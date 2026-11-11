//! The connection log sink handed to every protocol implementation.
//!
//! Lines are sent to the session (which batches them to the UI). Callers must never pass
//! secrets: log `PASS ****`, never the password; never key material. [`redact_command`] masks
//! the FTP commands that carry one.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::domain::log::{LogKind, LogLine};
use crate::domain::now_ms;

type Sink = dyn Fn(LogLine) + Send + Sync;

#[derive(Clone)]
pub struct Logger {
    sink: Arc<Sink>,
    seq: Arc<AtomicU64>,
}

impl std::fmt::Debug for Logger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Logger")
    }
}

/// Longest line kept; server banners and listings can be huge.
const MAX_LINE: usize = 2000;

impl Logger {
    pub fn new(sink: impl Fn(LogLine) + Send + Sync + 'static) -> Self {
        Self {
            sink: Arc::new(sink),
            seq: Arc::new(AtomicU64::new(0)),
        }
    }

    /// A logger that drops everything (tests).
    #[cfg(test)]
    pub fn noop() -> Self {
        Self::new(|_| {})
    }

    pub fn log(&self, kind: LogKind, text: impl Into<String>) {
        let mut text: String = text.into();
        if text.len() > MAX_LINE {
            let mut cut = MAX_LINE;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
            text.push('…');
        }
        let line = LogLine {
            seq: self.seq.fetch_add(1, Ordering::Relaxed) + 1,
            ts: now_ms(),
            kind,
            text: text.replace(['\r', '\n'], " ").trim_end().to_owned(),
        };
        (self.sink)(line);
    }

    pub fn status(&self, text: impl Into<String>) {
        self.log(LogKind::Status, text);
    }

    pub fn command(&self, text: impl Into<String>) {
        self.log(LogKind::Command, redact_command(&text.into()));
    }

    pub fn response(&self, text: impl Into<String>) {
        self.log(LogKind::Response, text);
    }

    pub fn error(&self, text: impl Into<String>) {
        self.log(LogKind::Error, text);
    }
}

/// Masks the argument of FTP commands that carry secrets (`PASS`, `ACCT`).
pub fn redact_command(command: &str) -> String {
    let upper = command.trim_start().to_ascii_uppercase();
    for verb in ["PASS", "ACCT"] {
        if upper == verb || upper.starts_with(&format!("{verb} ")) {
            return format!("{verb} ****");
        }
    }
    command.to_owned()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    fn capture() -> (Logger, Arc<Mutex<Vec<LogLine>>>) {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&lines);
        let logger = Logger::new(move |line| sink.lock().unwrap().push(line));
        (logger, lines)
    }

    #[test]
    fn passwords_are_masked() {
        assert_eq!(redact_command("PASS hunter2"), "PASS ****");
        assert_eq!(redact_command("pass hunter2"), "PASS ****");
        assert_eq!(redact_command("ACCT secret"), "ACCT ****");
        assert_eq!(redact_command("PASSIVE"), "PASSIVE");
        assert_eq!(redact_command("USER bob"), "USER bob");
    }

    #[test]
    fn lines_are_numbered_single_line_and_bounded() {
        let (logger, lines) = capture();
        logger.command("PASS hunter2");
        logger.response("230 ok\r\n");
        logger.status("x".repeat(5000));
        let lines = lines.lock().unwrap();
        assert_eq!(lines[0].text, "PASS ****");
        assert_eq!(lines[0].seq, 1);
        assert_eq!(lines[1].text, "230 ok");
        assert_eq!(lines[1].kind, LogKind::Response);
        assert!(lines[2].text.chars().count() <= MAX_LINE + 1);
    }
}
