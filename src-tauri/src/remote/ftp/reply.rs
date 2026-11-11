//! FTP reply codes and failures mapped to [`AppError`].
//!
//! `550` is overloaded: servers use it for "no such file", "permission denied" and generic
//! failures, and some (vsftpd) never say which. The text is inspected first; when it is vague,
//! the kind of operation decides: a lookup or read reports `NotFound`, a write or creation
//! reports `PermissionDenied`. See ADR 0004.

use suppaftp::{FtpError, FtpResult, Status};

use crate::domain::log::LogKind;
use crate::error::AppError;

/// What the failed command was trying to do (only used to read vague `550` replies).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Reading or looking up an existing path (RETR, SIZE, MDTM, MLST, CWD, RNFR, DELE, RMD).
    Read,
    /// Creating or writing (STOR, MKD, RNTO, SITE CHMOD).
    Write,
}

/// The three-digit code of a reply. suppaftp maps codes it does not know (e.g. 554) to
/// `Status::Unknown`, so the code is read from the reply text itself.
pub fn code(response: &suppaftp::types::Response) -> u32 {
    response
        .body
        .get(..3)
        .and_then(|digits| std::str::from_utf8(digits).ok())
        .and_then(|digits| digits.parse().ok())
        .unwrap_or_else(|| response.status.code())
}

/// The reply as one line of text (`550 Failed to open file.`).
pub fn text(response: &suppaftp::types::Response) -> String {
    let text = String::from_utf8_lossy(&response.body);
    let mut lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    // Multi-line replies repeat the code; the last line carries the outcome.
    if lines.len() > 2 {
        lines.drain(..lines.len() - 1);
    }
    lines.join(" ")
}

/// Whether a reply means "this command is unknown or not implemented".
pub fn is_unsupported(code: u32) -> bool {
    matches!(code, 500 | 502 | 504)
}

const PERMISSION_WORDS: [&str; 6] = [
    "permission denied",
    "access denied",
    "not permitted",
    "could not create",
    "read-only",
    "not allowed",
];
const MISSING_WORDS: [&str; 5] = [
    "no such file",
    "not found",
    "does not exist",
    "doesn't exist",
    "cannot find",
];

/// Maps a negative reply (`code`, one-line `text`) for `path`.
pub fn from_reply(code: u32, text: &str, path: &str, op: Op) -> AppError {
    let lower = text.to_ascii_lowercase();
    let says_permission = PERMISSION_WORDS.iter().any(|word| lower.contains(word));
    let says_missing = MISSING_WORDS.iter().any(|word| lower.contains(word));
    match code {
        530 | 532 => AppError::AuthFailed(text.to_owned()),
        421 => AppError::Disconnected,
        450 | 550 | 553 if says_permission => AppError::PermissionDenied(path.to_owned()),
        450 | 550 if says_missing => AppError::not_found(path),
        // 553: "file name not allowed"; vsftpd answers it for every refused STOR.
        553 => AppError::PermissionDenied(path.to_owned()),
        550 if op == Op::Read => AppError::not_found(path),
        550 if op == Op::Write => AppError::PermissionDenied(path.to_owned()),
        code if is_unsupported(code) => {
            AppError::Unsupported(format!("the server does not support this command ({text})"))
        }
        _ => AppError::Remote(text.to_owned()),
    }
}

/// Maps a suppaftp failure on `path`. Broken control connections are `Disconnected`.
pub fn from_ftp(error: &FtpError, path: &str, op: Op) -> AppError {
    match error {
        FtpError::UnexpectedResponse(response) => {
            from_reply(code(response), &text(response), path, op)
        }
        FtpError::ConnectionError(io) => from_io(io),
        FtpError::SecureError(message) => AppError::Connection(format!("TLS error: {message}")),
        FtpError::BadResponse => AppError::Remote("the server sent an invalid reply".into()),
        FtpError::InvalidAddress(_) => {
            AppError::Remote("the server announced an invalid data connection address".into())
        }
        FtpError::DataConnectionAlreadyOpen => {
            AppError::Internal("an FTP data connection is already open".into())
        }
    }
}

/// Maps an I/O failure on the control or data connection.
pub fn from_io(io: &std::io::Error) -> AppError {
    use std::io::ErrorKind;
    match io.kind() {
        ErrorKind::UnexpectedEof
        | ErrorKind::ConnectionReset
        | ErrorKind::ConnectionAborted
        | ErrorKind::BrokenPipe
        | ErrorKind::NotConnected => AppError::Disconnected,
        ErrorKind::TimedOut => AppError::Connection("the server did not answer in time".into()),
        ErrorKind::ConnectionRefused => {
            AppError::Connection("the server refused the data connection".into())
        }
        _ => AppError::Connection(io.to_string()),
    }
}

/// Whether a failure leaves the control connection unusable.
pub fn is_fatal(error: &FtpError) -> bool {
    match error {
        // Refused: only the data connection's TCP connect can be refused mid-session.
        FtpError::ConnectionError(io) => io.kind() != std::io::ErrorKind::ConnectionRefused,
        FtpError::BadResponse => true,
        FtpError::UnexpectedResponse(response) => code(response) == 421,
        _ => false,
    }
}

/// Whether a data command failed because the data connection could not be opened, before any
/// byte moved: a `425` reply (to `PASV`/`EPSV` or to the transfer command), or the passive port
/// refusing the TCP connection. Busy servers answer this transiently (passive ports in
/// `TIME_WAIT`), so it is worth one retry.
pub fn is_data_refusal(error: &FtpError) -> bool {
    match error {
        FtpError::UnexpectedResponse(response) => code(response) == 425,
        FtpError::ConnectionError(io) => io.kind() == std::io::ErrorKind::ConnectionRefused,
        _ => false,
    }
}

/// How to log the outcome of a probe (a command sent to find out what a path is, such as
/// `SIZE` or `CWD` on a symlink): any reply, negative ones included, is an expected answer and
/// logged as a response; only a broken connection is an error.
pub fn probe_log_kind<T>(result: &FtpResult<T>) -> LogKind {
    match result {
        Ok(_) | Err(FtpError::UnexpectedResponse(_)) => LogKind::Response,
        Err(_) => LogKind::Error,
    }
}

/// The size in a `213 <size>` reply to `SIZE`.
pub fn size_reply(response: &suppaftp::types::Response) -> Option<u64> {
    text(response)
        .strip_prefix("213")
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|size| size.parse().ok())
}

/// Reply codes that end an aborted or early-closed transfer.
pub fn is_transfer_end(status: Status) -> bool {
    matches!(
        status,
        Status::ClosingDataConnection
            | Status::RequestedFileActionOk
            | Status::TransferAborted
            | Status::ActionAborted
            | Status::DataConnectionOpen
    )
}

#[cfg(test)]
mod tests {
    use suppaftp::types::Response;

    use super::*;
    use crate::error::ErrorCode;

    fn reply(body: &str) -> FtpError {
        FtpError::UnexpectedResponse(Response::new(
            Status::from(body[..3].parse::<u32>().unwrap()),
            body.as_bytes().to_vec(),
        ))
    }

    #[test]
    fn reads_codes_and_text() {
        let FtpError::UnexpectedResponse(response) = reply("554 Invalid REST parameter.\r\n")
        else {
            unreachable!()
        };
        assert_eq!(response.status, Status::Unknown);
        assert_eq!(code(&response), 554);
        assert_eq!(text(&response), "554 Invalid REST parameter.");
    }

    #[test]
    fn maps_reply_codes() {
        let cases = [
            ("530 Login incorrect.", Op::Read, ErrorCode::AuthFailed),
            ("421 Timeout.", Op::Read, ErrorCode::Disconnected),
            // ProFTPD says what happened.
            (
                "550 /x: No such file or directory",
                Op::Write,
                ErrorCode::NotFound,
            ),
            (
                "550 /x: Permission denied",
                Op::Read,
                ErrorCode::PermissionDenied,
            ),
            // vsftpd does not: the operation decides.
            ("550 Failed to open file.", Op::Read, ErrorCode::NotFound),
            (
                "550 Create directory operation failed.",
                Op::Write,
                ErrorCode::PermissionDenied,
            ),
            (
                "553 Could not create file.",
                Op::Write,
                ErrorCode::PermissionDenied,
            ),
            ("500 Unknown command.", Op::Read, ErrorCode::Unsupported),
            ("552 Disk full", Op::Write, ErrorCode::Remote),
        ];
        for (body, op, expected) in cases {
            assert_eq!(from_ftp(&reply(body), "/x", op).code(), expected, "{body}");
        }
    }

    #[test]
    fn broken_connections_are_disconnected() {
        let eof = FtpError::ConnectionError(std::io::ErrorKind::UnexpectedEof.into());
        assert_eq!(
            from_ftp(&eof, "/", Op::Read).code(),
            ErrorCode::Disconnected
        );
        assert!(is_fatal(&eof));
        assert!(is_fatal(&reply("421 Service not available")));
        assert!(!is_fatal(&reply("550 Failed to open file.")));
        assert!(is_transfer_end(Status::TransferAborted));
    }

    #[test]
    fn probe_failures_are_responses_not_errors() {
        let ok: FtpResult<()> = Ok(());
        assert_eq!(probe_log_kind(&ok), LogKind::Response);
        let missing: FtpResult<()> = Err(reply("550 Could not get file size."));
        assert_eq!(probe_log_kind(&missing), LogKind::Response);
        let broken: FtpResult<()> = Err(FtpError::ConnectionError(
            std::io::ErrorKind::UnexpectedEof.into(),
        ));
        assert_eq!(probe_log_kind(&broken), LogKind::Error);
    }

    #[test]
    fn reads_sizes_from_size_replies() {
        let FtpError::UnexpectedResponse(response) = reply("213 727\r\n") else {
            unreachable!()
        };
        assert_eq!(size_reply(&response), Some(727));
        let FtpError::UnexpectedResponse(response) = reply("550 Could not get file size.") else {
            unreachable!()
        };
        assert_eq!(size_reply(&response), None);
    }

    #[test]
    fn data_refusals_are_retryable_and_not_fatal() {
        assert!(is_data_refusal(&reply("425 PASV: Address in use")));
        assert!(is_data_refusal(&reply(
            "425 Unable to build data connection: Connection refused"
        )));
        let refused = FtpError::ConnectionError(std::io::ErrorKind::ConnectionRefused.into());
        assert!(is_data_refusal(&refused));
        assert!(!is_fatal(&refused));
        for other in [
            reply("426 Transfer aborted."),
            reply("550 Failed to open file."),
            reply("421 Timeout."),
            FtpError::ConnectionError(std::io::ErrorKind::ConnectionReset.into()),
            FtpError::BadResponse,
        ] {
            assert!(!is_data_refusal(&other), "{other}");
        }
    }
}
