//! Remote commands over SSH (ADR 0014): one `exec` channel per run, on the connection the
//! SFTP channels already use (no new handshake, no new host key check).
//!
//! No PTY is requested: the server runs the command with the user's shell (`$SHELL -c`), exactly
//! like `ssh host command`. Standard input is closed at once, so a command that reads it sees
//! end of file instead of waiting. Output is forwarded chunk by chunk (one SSH packet, at most
//! 32 KiB) as it arrives and never buffered. Stop sends `TERM` and closes the channel if the
//! command has not ended within [`STOP_GRACE`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use russh::{ChannelMsg, Sig};

use super::error::describe_ssh;
use super::fs::Shared;
use crate::domain::command::{CommandExit, CommandStream};
use crate::error::{AppError, AppResult};
use crate::remote::{Logger, OutputSink, RemoteShell};

/// How long a stopped command gets to end after `TERM` before its channel is closed.
const STOP_GRACE: Duration = Duration::from_secs(1);

/// How often a running command checks its cancel flag.
const CANCEL_POLL: Duration = Duration::from_millis(50);

/// `SSH_EXTENDED_DATA_STDERR` (RFC 4254 §5.2).
const STDERR: u32 = 1;

/// Runs commands on the SSH connection of an [`super::SftpConnection`].
pub struct SftpShell {
    shared: Arc<Shared>,
    logger: Logger,
}

impl SftpShell {
    pub fn new(shared: Arc<Shared>, logger: Logger) -> Self {
        Self { shared, logger }
    }

    /// A failure on the channel: `Disconnected` once the transport is gone.
    fn fail(&self, error: &russh::Error) -> AppError {
        let error = if self.shared.is_alive() {
            AppError::Connection(describe_ssh(error))
        } else {
            AppError::Disconnected
        };
        self.logger.error(error.to_string());
        error
    }
}

/// The SSH name of a signal (`TERM`, `KILL`, …), as `CommandExit::signal` reports it.
pub fn signal_name(signal: &Sig) -> String {
    match signal {
        Sig::ABRT => "ABRT".into(),
        Sig::ALRM => "ALRM".into(),
        Sig::FPE => "FPE".into(),
        Sig::HUP => "HUP".into(),
        Sig::ILL => "ILL".into(),
        Sig::INT => "INT".into(),
        Sig::KILL => "KILL".into(),
        Sig::PIPE => "PIPE".into(),
        Sig::QUIT => "QUIT".into(),
        Sig::SEGV => "SEGV".into(),
        Sig::TERM => "TERM".into(),
        Sig::USR1 => "USR1".into(),
        Sig::Custom(name) => name.clone(),
    }
}

/// The connection-log line for how a command ended.
pub fn describe_exit(exit: &CommandExit) -> String {
    match (exit.cancelled, &exit.signal, exit.code) {
        (true, _, _) => "cancelled".into(),
        (false, Some(signal), _) => format!("signal {signal}"),
        (false, None, Some(code)) => format!("exit {code}"),
        (false, None, None) => "ended without an exit status".into(),
    }
}

#[async_trait]
impl RemoteShell for SftpShell {
    async fn exec(
        &self,
        command: &str,
        output: OutputSink<'_>,
        cancel: &AtomicBool,
    ) -> AppResult<CommandExit> {
        if !self.shared.is_alive() {
            return Err(AppError::Disconnected);
        }
        // The command is user-typed and shown in the UI; its output is never logged.
        self.logger.command(format!("exec {command}"));
        let mut channel = self
            .shared
            .handle
            .channel_open_session()
            .await
            .map_err(|error| self.fail(&error))?;
        channel
            .exec(true, command)
            .await
            .map_err(|error| self.fail(&error))?;
        // No input: a command reading stdin gets end of file.
        let _ = channel.eof().await;

        let mut exit = CommandExit {
            code: None,
            signal: None,
            cancelled: false,
        };
        let mut stop_deadline: Option<tokio::time::Instant> = None;
        let mut ticker = tokio::time::interval(CANCEL_POLL);
        let ended = loop {
            tokio::select! {
                message = channel.wait() => match message {
                    Some(ChannelMsg::Data { data }) => output(CommandStream::Stdout, &data),
                    Some(ChannelMsg::ExtendedData { data, ext }) => {
                        let stream = if ext == STDERR {
                            CommandStream::Stderr
                        } else {
                            CommandStream::Stdout
                        };
                        output(stream, &data);
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => exit.code = Some(exit_status),
                    Some(ChannelMsg::ExitSignal { signal_name: signal, .. }) => {
                        exit.signal = Some(signal_name(&signal));
                    }
                    Some(ChannelMsg::Failure) => {
                        let error =
                            AppError::Remote("the server refused to run the command".into());
                        self.logger.error(error.to_string());
                        let _ = channel.close().await;
                        return Err(error);
                    }
                    // EOF comes before the exit status; the channel's end is `Close`/`None`.
                    Some(ChannelMsg::Close) | None => break true,
                    Some(_) => {}
                },
                _ = ticker.tick() => {
                    if stop_deadline.is_none() && cancel.load(Ordering::Acquire) {
                        exit.cancelled = true;
                        self.logger.status("Stopping the command (TERM)");
                        let _ = channel.signal(Sig::TERM).await;
                        stop_deadline = Some(tokio::time::Instant::now() + STOP_GRACE);
                    }
                    if stop_deadline.is_some_and(|deadline| tokio::time::Instant::now() >= deadline) {
                        break false;
                    }
                }
            }
        };
        if !ended {
            // The command ignored TERM (or the server ignores signal requests): close.
            let _ = channel.close().await;
        }
        if !exit.cancelled
            && exit.code.is_none()
            && exit.signal.is_none()
            && !self.shared.is_alive()
        {
            let error = AppError::Disconnected;
            self.logger.error(error.to_string());
            return Err(error);
        }
        self.logger.response(describe_exit(&exit));
        Ok(exit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exits_are_described_for_the_log() {
        let exit = |code, signal: Option<&str>, cancelled| CommandExit {
            code,
            signal: signal.map(str::to_owned),
            cancelled,
        };
        assert_eq!(describe_exit(&exit(Some(3), None, false)), "exit 3");
        assert_eq!(
            describe_exit(&exit(None, Some("TERM"), false)),
            "signal TERM"
        );
        assert_eq!(describe_exit(&exit(None, Some("TERM"), true)), "cancelled");
        assert_eq!(
            describe_exit(&exit(None, None, false)),
            "ended without an exit status"
        );
        assert_eq!(signal_name(&Sig::Custom("WINCH".into())), "WINCH");
        assert_eq!(signal_name(&Sig::KILL), "KILL");
    }
}
