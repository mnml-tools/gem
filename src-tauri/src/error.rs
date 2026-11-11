//! Application error type shared by the remote layer, services and Tauri commands.
//!
//! Errors are serialised to the UI as `{ code, message, hostKey }`. Messages never include
//! passwords, passphrases or file contents, so they are safe to show and to log.

use serde::Serialize;
use ts_rs::TS;

use crate::domain::session::HostKeyInfo;

/// Stable, machine-readable error category exposed to the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "bindings/")]
pub enum ErrorCode {
    NotFound,
    Validation,
    /// The remote file changed since it was opened (editor save) or a target already exists.
    Conflict,
    Storage,
    Platform,
    Internal,
    /// The server could not be reached, or the connection broke during an operation.
    Connection,
    AuthFailed,
    /// The site needs a password or passphrase that is not stored; ask the user.
    SecretRequired,
    HostKeyUnknown,
    HostKeyChanged,
    CertificateUntrusted,
    PermissionDenied,
    /// Any other error reported by the server (message carries the server's text).
    Remote,
    Cancelled,
    /// The session is closed or was dropped; reconnect first.
    Disconnected,
    Unsupported,
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{entity} not found")]
    NotFound { entity: String },
    #[error("{0}")]
    Validation(String),
    #[error("{0}")]
    Conflict(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("{0}")]
    Platform(String),
    #[error("internal error: {0}")]
    Internal(String),
    #[error("{0}")]
    Connection(String),
    #[error("{0}")]
    AuthFailed(String),
    #[error("a password or passphrase is required")]
    SecretRequired,
    #[error("the server's host key is not trusted yet")]
    HostKeyUnknown(HostKeyInfo),
    #[error("the server's host key has changed; the connection was refused")]
    HostKeyChanged(HostKeyInfo),
    #[error("the server's certificate is not trusted")]
    CertificateUntrusted(HostKeyInfo),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("{0}")]
    Remote(String),
    #[error("cancelled")]
    Cancelled,
    #[error("disconnected")]
    Disconnected,
    #[error("{0}")]
    Unsupported(String),
}

impl AppError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::NotFound { .. } => ErrorCode::NotFound,
            Self::Validation(_) => ErrorCode::Validation,
            Self::Conflict(_) => ErrorCode::Conflict,
            Self::Storage(_) => ErrorCode::Storage,
            Self::Platform(_) => ErrorCode::Platform,
            Self::Internal(_) => ErrorCode::Internal,
            Self::Connection(_) => ErrorCode::Connection,
            Self::AuthFailed(_) => ErrorCode::AuthFailed,
            Self::SecretRequired => ErrorCode::SecretRequired,
            Self::HostKeyUnknown(_) => ErrorCode::HostKeyUnknown,
            Self::HostKeyChanged(_) => ErrorCode::HostKeyChanged,
            Self::CertificateUntrusted(_) => ErrorCode::CertificateUntrusted,
            Self::PermissionDenied(_) => ErrorCode::PermissionDenied,
            Self::Remote(_) => ErrorCode::Remote,
            Self::Cancelled => ErrorCode::Cancelled,
            Self::Disconnected => ErrorCode::Disconnected,
            Self::Unsupported(_) => ErrorCode::Unsupported,
        }
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::Validation(message.into())
    }

    pub fn not_found(entity: impl Into<String>) -> Self {
        Self::NotFound {
            entity: entity.into(),
        }
    }

    pub fn host_key(&self) -> Option<&HostKeyInfo> {
        match self {
            Self::HostKeyUnknown(info)
            | Self::HostKeyChanged(info)
            | Self::CertificateUntrusted(info) => Some(info),
            _ => None,
        }
    }
}

impl From<tauri::Error> for AppError {
    fn from(error: tauri::Error) -> Self {
        Self::Platform(error.to_string())
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(error: rusqlite::Error) -> Self {
        match error {
            rusqlite::Error::QueryReturnedNoRows => Self::not_found("record"),
            other => Self::Storage(other.to_string()),
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(error: std::io::Error) -> Self {
        Self::Platform(error.to_string())
    }
}

/// Wire format of an error as seen by the UI.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/", rename = "AppError")]
pub struct ErrorPayload {
    pub code: ErrorCode,
    pub message: String,
    /// Set for `host_key_unknown`, `host_key_changed` and `certificate_untrusted`.
    pub host_key: Option<HostKeyInfo>,
}

impl From<&AppError> for ErrorPayload {
    fn from(error: &AppError) -> Self {
        Self {
            code: error.code(),
            message: error.to_string(),
            host_key: error.host_key().cloned(),
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        ErrorPayload::from(self).serialize(serializer)
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialises_as_code_and_message() {
        let json = serde_json::to_value(AppError::Conflict("changed".into())).unwrap();
        assert_eq!(json["code"], "conflict");
        assert_eq!(json["message"], "changed");
        assert!(json["hostKey"].is_null());
    }

    #[test]
    fn host_key_errors_carry_the_key() {
        let info = HostKeyInfo {
            host: "example.test".into(),
            port: 22,
            key_type: "ssh-ed25519".into(),
            fingerprint: "SHA256:abc".into(),
            previous_fingerprint: Some("SHA256:old".into()),
        };
        let json = serde_json::to_value(AppError::HostKeyChanged(info)).unwrap();
        assert_eq!(json["code"], "host_key_changed");
        assert_eq!(json["hostKey"]["fingerprint"], "SHA256:abc");
        assert_eq!(json["hostKey"]["previousFingerprint"], "SHA256:old");
    }

    #[test]
    fn missing_row_maps_to_not_found() {
        let error: AppError = rusqlite::Error::QueryReturnedNoRows.into();
        assert_eq!(error.code(), ErrorCode::NotFound);
    }
}
