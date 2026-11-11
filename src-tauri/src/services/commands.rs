//! Remote commands (ADR 0014): one SSH `exec` channel per run, on the session's connection.
//!
//! [`run`] expands `{dir}`, starts the command in a task and returns at once with its run id.
//! Output reaches the run's sink as [`CommandEvent::Output`], decoded and cleaned by
//! [`text::terminal`](crate::text::terminal) and batched (~30 ms) so a chatty command cannot
//! flood the IPC; then exactly one terminal event: `Exit` or `Failed`.
//!
//! Runs belong to their session ([`Runs`]): closing the session stops them. A run that produces
//! more than [`OUTPUT_CAP`] bytes is stopped, with a note on stderr (the viewer also keeps only a
//! bounded amount of text). Commands and their output are never written to the app log; the
//! protocol layer logs the command line and its exit status to the connection log.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::sessions::Session;
use crate::domain::command::{CommandEvent, CommandStream, RunStarted};
use crate::error::{AppError, AppResult, ErrorPayload};
use crate::text::terminal::OutputDecoder;

/// Where a run's events go (its viewer tab's `tauri::ipc::Channel` in the app).
pub type CommandSink = Arc<dyn Fn(CommandEvent) + Send + Sync>;

/// Raw output after which a run is stopped.
pub const OUTPUT_CAP: u64 = 64 * 1024 * 1024;

/// Output is sent at most this often.
const BATCH: Duration = Duration::from_millis(30);

/// Longest command line accepted (after `{dir}` expansion).
const MAX_COMMAND: usize = 64 * 1024;

const PLACEHOLDER: &str = "{dir}";

/// Shown when the server refuses another channel on the connection.
pub const NO_MORE_CHANNELS: &str = "The server allows no more open channels on this connection \
     (MaxSessions). Stop a running command or wait for transfers to finish.";

/// Added on stderr when Stop had to close the channel of a command that ignored `TERM`.
pub const STILL_RUNNING_NOTE: &str = "\n[Stopped. Without a terminal, a program that ignores \
     TERM may still be running on the server.]\n";

/// Whether the server refused to open the channel (OpenSSH `MaxSessions`), as russh reports it.
fn channel_refused(error: &AppError) -> bool {
    matches!(error, AppError::Connection(message) if message.contains("Failed to open channel"))
}

/// Quotes `value` for a POSIX shell: `'…'`, with each `'` written as `'\''`.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Replaces every `{dir}` with the shell-quoted folder; `Validation` when there is none.
pub fn expand(command: &str, dir: Option<&str>) -> AppResult<String> {
    if command.trim().is_empty() {
        return Err(AppError::validation("the command is empty"));
    }
    if command.contains('\0') || dir.is_some_and(|d| d.contains('\0')) {
        return Err(AppError::validation(
            "commands cannot contain NUL characters",
        ));
    }
    let expanded = if command.contains(PLACEHOLDER) {
        let Some(dir) = dir.filter(|d| !d.is_empty()) else {
            return Err(AppError::validation(
                "this command uses {dir}, which needs a folder",
            ));
        };
        command.replace(PLACEHOLDER, &shell_quote(dir))
    } else {
        command.to_owned()
    };
    if expanded.len() > MAX_COMMAND {
        return Err(AppError::validation("the command is too long"));
    }
    Ok(expanded)
}

/// The running commands of one session, by run id.
#[derive(Default)]
pub struct Runs {
    runs: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl Runs {
    fn lock(&self) -> MutexGuard<'_, HashMap<String, Arc<AtomicBool>>> {
        self.runs.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[cfg(test)]
    pub fn contains(&self, run_id: &str) -> bool {
        self.lock().contains_key(run_id)
    }

    /// Asks a run to stop. False when there is no such run (anymore).
    pub fn cancel(&self, run_id: &str) -> bool {
        match self.lock().get(run_id) {
            Some(cancel) => {
                cancel.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    /// Stops every run and waits (up to `wait`) for them to end, so they can report it.
    pub async fn stop_all(&self, wait: Duration) {
        for cancel in self.lock().values() {
            cancel.store(true, Ordering::SeqCst);
        }
        let deadline = Instant::now() + wait;
        while !self.lock().is_empty() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    fn insert(&self, run_id: &str, cancel: Arc<AtomicBool>) {
        self.lock().insert(run_id.to_owned(), cancel);
    }

    fn remove(&self, run_id: &str) {
        self.lock().remove(run_id);
    }
}

/// Starts `command` (with `{dir}` expanded) on the session's shell.
pub fn run(
    session: &Arc<Session>,
    command: &str,
    dir: Option<&str>,
    sink: CommandSink,
) -> AppResult<RunStarted> {
    run_capped(session, command, dir, sink, OUTPUT_CAP)
}

pub(crate) fn run_capped(
    session: &Arc<Session>,
    command: &str,
    dir: Option<&str>,
    sink: CommandSink,
    cap: u64,
) -> AppResult<RunStarted> {
    let command = expand(command, dir)?;
    let shell = session.shell()?;
    let run_id = uuid::Uuid::now_v7().to_string();
    let cancel = Arc::new(AtomicBool::new(false));
    session.runs.insert(&run_id, Arc::clone(&cancel));
    let pipe = Arc::new(Pipe::new(Arc::clone(&sink), Arc::clone(&cancel), cap));

    let ticker = {
        let pipe = Arc::clone(&pipe);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(BATCH);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                if !pipe.tick() {
                    break;
                }
            }
        })
    };
    let task = {
        let session = Arc::clone(session);
        let command = command.clone();
        let run_id = run_id.clone();
        async move {
            let started = Instant::now();
            let output = |stream: CommandStream, bytes: &[u8]| pipe.push(stream, bytes);
            let result = shell.exec(&command, &output, &cancel).await;
            // Stopped, and the server reported neither an exit status nor a signal: the
            // channel was closed on a command that ignored TERM.
            let forced = matches!(
                &result,
                Ok(exit) if exit.cancelled && exit.code.is_none() && exit.signal.is_none()
            );
            pipe.finish(forced.then_some(STILL_RUNNING_NOTE));
            ticker.abort();
            let result = match result {
                Err(error) if channel_refused(&error) => {
                    Err(AppError::Remote(NO_MORE_CHANNELS.into()))
                }
                other => other,
            };
            session.runs.remove(&run_id);
            let event = match &result {
                Ok(exit) => CommandEvent::Exit {
                    exit: exit.clone(),
                    duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                },
                Err(error) => CommandEvent::Failed {
                    error: ErrorPayload::from(error),
                },
            };
            sink(event);
            // A broken connection is the session's business too.
            let _ = session.observe(result);
        }
    };
    tokio::spawn(task);
    Ok(RunStarted { run_id, command })
}

struct PipeState {
    stdout: OutputDecoder,
    stderr: OutputDecoder,
    /// Text not sent yet, in arrival order (consecutive text of one stream merged).
    pending: Vec<(CommandStream, String)>,
    bytes: u64,
    capped: bool,
    closed: bool,
}

impl PipeState {
    fn add(&mut self, stream: CommandStream, text: String) {
        if text.is_empty() {
            return;
        }
        match self.pending.last_mut() {
            Some((last, buffer)) if *last == stream => buffer.push_str(&text),
            _ => self.pending.push((stream, text)),
        }
    }

    fn decoder(&mut self, stream: CommandStream) -> &mut OutputDecoder {
        match stream {
            CommandStream::Stdout => &mut self.stdout,
            CommandStream::Stderr => &mut self.stderr,
        }
    }
}

/// Decodes, bounds and batches one run's output. Events are sent under the lock, so the
/// terminal event (sent after `finish`) always comes last.
struct Pipe {
    state: Mutex<PipeState>,
    sink: CommandSink,
    cancel: Arc<AtomicBool>,
    cap: u64,
}

impl Pipe {
    fn new(sink: CommandSink, cancel: Arc<AtomicBool>, cap: u64) -> Self {
        Self {
            state: Mutex::new(PipeState {
                stdout: OutputDecoder::new(),
                stderr: OutputDecoder::new(),
                pending: Vec::new(),
                bytes: 0,
                capped: false,
                closed: false,
            }),
            sink,
            cancel,
            cap,
        }
    }

    fn lock(&self) -> MutexGuard<'_, PipeState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn push(&self, stream: CommandStream, bytes: &[u8]) {
        let mut state = self.lock();
        if state.closed || state.capped {
            return;
        }
        state.bytes = state.bytes.saturating_add(bytes.len() as u64);
        if state.bytes > self.cap {
            state.capped = true;
            self.cancel.store(true, Ordering::SeqCst);
            let mib = self.cap / (1024 * 1024);
            state.add(
                CommandStream::Stderr,
                format!("\n[Gem stopped this command: it printed more than {mib} MiB of output]\n"),
            );
            return;
        }
        let text = state.decoder(stream).push(bytes);
        state.add(stream, text);
    }

    /// A batch flush; false once the run is over.
    fn tick(&self) -> bool {
        let mut state = self.lock();
        if state.closed {
            return false;
        }
        if !state.capped {
            for stream in [CommandStream::Stdout, CommandStream::Stderr] {
                let text = state.decoder(stream).take_partial();
                state.add(stream, text);
            }
        }
        self.send(&mut state);
        true
    }

    /// The command ended: sends everything left, then `note` (on stderr).
    fn finish(&self, note: Option<&str>) {
        let mut state = self.lock();
        if state.closed {
            return;
        }
        state.closed = true;
        if !state.capped {
            for stream in [CommandStream::Stdout, CommandStream::Stderr] {
                let text = state.decoder(stream).finish();
                state.add(stream, text);
            }
        }
        if let Some(note) = note {
            state.add(CommandStream::Stderr, note.to_owned());
        }
        self.send(&mut state);
    }

    fn send(&self, state: &mut PipeState) {
        for (stream, text) in std::mem::take(&mut state.pending) {
            (self.sink)(CommandEvent::Output { stream, text });
        }
    }
}

#[cfg(test)]
mod tests;
