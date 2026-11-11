//! Mapping of SSH and SFTP failures to [`AppError`].

use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::StatusCode;

use crate::error::AppError;

/// Maps an SFTP request failure on `path` to the app's error categories.
///
/// Status codes map one to one (`NoSuchFile` → `NotFound`, `PermissionDenied` →
/// `PermissionDenied`, `Failure` → `Remote`); a closed channel or transport is `Disconnected`.
pub fn from_sftp(error: SftpError, path: &str) -> AppError {
    match error {
        SftpError::Status(status) => {
            let message = status.error_message.trim().to_owned();
            match status.status_code {
                StatusCode::NoSuchFile => AppError::not_found(path),
                StatusCode::PermissionDenied => AppError::PermissionDenied(path.to_owned()),
                StatusCode::NoConnection | StatusCode::ConnectionLost => AppError::Disconnected,
                StatusCode::OpUnsupported => AppError::Unsupported(format!(
                    "the server does not support this operation on {path}"
                )),
                StatusCode::Failure if message.is_empty() || message == "Failure" => {
                    AppError::Remote(format!(
                        "the server could not complete the request on {path}"
                    ))
                }
                StatusCode::Ok | StatusCode::Eof | StatusCode::Failure | StatusCode::BadMessage => {
                    AppError::Remote(format!("{path}: {message}"))
                }
            }
        }
        SftpError::Timeout => AppError::Connection("the server did not answer in time".into()),
        // The request could not be sent or its reply never came: the channel is gone.
        SftpError::IO(_) | SftpError::UnexpectedBehavior(_) => AppError::Disconnected,
        SftpError::Limited(message) => AppError::Remote(format!("server limit: {message}")),
        SftpError::UnexpectedPacket => {
            AppError::Remote("the server sent an unexpected SFTP reply".into())
        }
    }
}

/// Whether an SFTP failure is a status `code` (used to branch on "does not exist").
pub fn is_status(error: &SftpError, code: StatusCode) -> bool {
    matches!(error, SftpError::Status(status) if status.status_code == code)
}

/// A human message for a transport-level SSH failure while connecting.
pub fn describe_ssh(error: &russh::Error) -> String {
    match error {
        russh::Error::Disconnect | russh::Error::HUP => {
            "the server closed the connection during the SSH handshake".into()
        }
        russh::Error::ConnectionTimeout => "the SSH handshake timed out".into(),
        russh::Error::NoCommonAlgo { kind, .. } => {
            format!("no SSH algorithm in common with the server ({kind:?})")
        }
        russh::Error::Version => "the server did not answer with an SSH identification".into(),
        russh::Error::IO(io) => format!("SSH connection failed: {io}"),
        other => format!("SSH error: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use russh_sftp::protocol::Status;

    use super::*;
    use crate::error::ErrorCode;

    fn status(code: StatusCode, message: &str) -> SftpError {
        SftpError::Status(Status {
            id: 1,
            status_code: code,
            error_message: message.into(),
            language_tag: "en".into(),
        })
    }

    #[test]
    fn maps_status_codes() {
        let cases = [
            (StatusCode::NoSuchFile, ErrorCode::NotFound),
            (StatusCode::PermissionDenied, ErrorCode::PermissionDenied),
            (StatusCode::Failure, ErrorCode::Remote),
            (StatusCode::ConnectionLost, ErrorCode::Disconnected),
            (StatusCode::NoConnection, ErrorCode::Disconnected),
            (StatusCode::OpUnsupported, ErrorCode::Unsupported),
        ];
        for (code, expected) in cases {
            assert_eq!(
                from_sftp(status(code, "x"), "/a").code(),
                expected,
                "{code:?}"
            );
        }
        assert_eq!(
            from_sftp(status(StatusCode::PermissionDenied, ""), "/etc/x").to_string(),
            "permission denied: /etc/x"
        );
    }

    #[test]
    fn closed_channels_are_disconnected() {
        let closed = SftpError::UnexpectedBehavior("session closed".into());
        assert_eq!(from_sftp(closed, "/a").code(), ErrorCode::Disconnected);
        assert_eq!(
            from_sftp(SftpError::Timeout, "/a").code(),
            ErrorCode::Connection
        );
        assert!(is_status(
            &status(StatusCode::NoSuchFile, ""),
            StatusCode::NoSuchFile
        ));
    }
}
