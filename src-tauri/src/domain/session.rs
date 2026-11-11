//! Live connections (one per connection tab) and the events they stream to the UI.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::log::LogLine;
use super::site::{Protocol, SiteInput};
use super::transfer::Transfer;
use crate::error::ErrorPayload;

/// A server identity the user is asked to trust: an SSH host key or a TLS certificate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct HostKeyInfo {
    pub host: String,
    pub port: u16,
    /// `ssh-ed25519`, `ecdsa-sha2-nistp256`, `ssh-rsa`, … or `x509` for FTPS certificates.
    pub key_type: String,
    /// `SHA256:<base64>` as printed by OpenSSH (certificates: same format over the DER).
    pub fingerprint: String,
    /// The stored fingerprint when the key changed.
    pub previous_fingerprint: Option<String>,
}

/// What `connect` opens: a saved site or an unsaved quick-connect target.
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "bindings/")]
pub enum ConnectTarget {
    Site { id: String },
    Adhoc { site: SiteInput },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "bindings/")]
pub enum SessionState {
    Connecting,
    Connected,
    Disconnected,
}

/// Optional protocol features, so the UI never branches on the protocol itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct Capabilities {
    /// Rename over an existing file in one step (`posix-rename@openssh.com`, FTP `RNTO`).
    pub atomic_rename: bool,
    pub chmod: bool,
    /// Listings report owner and group names.
    pub owner: bool,
    pub symlinks: bool,
    /// Remote commands can run on this connection (SSH exec channels; never FTP). Set by the
    /// session from `Connection::shell`, not by the file system.
    pub exec: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct SessionInfo {
    pub id: String,
    pub site_id: Option<String>,
    /// Site name, or `user@host` for quick connect.
    pub label: String,
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub user: String,
    /// The server's home directory (canonical).
    pub home: String,
    /// Where the file list starts: the site's initial path, or `home`.
    pub start_path: String,
    /// Plain FTP: credentials and data travel unencrypted.
    pub insecure: bool,
    /// Server identification (SSH banner / FTP welcome), for the status bar.
    pub server_info: Option<String>,
    pub capabilities: Capabilities,
}

/// Everything a session pushes to its tab, through one `tauri::ipc::Channel` per tab.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export, export_to = "bindings/")]
pub enum SessionEvent {
    Log {
        lines: Vec<LogLine>,
    },
    State {
        state: SessionState,
        reason: Option<String>,
    },
    Transfers {
        transfers: Vec<Transfer>,
    },
}

/// Result of "Test connection" in the site dialog. A failed connection is a result too
/// (`ok == false` with `error`), so the dialog can show the server conversation that led to it.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct TestResult {
    pub ok: bool,
    pub server_info: Option<String>,
    /// The home folder, when the test succeeded.
    pub home: Option<String>,
    /// Why the test failed (`host_key_unknown` / `certificate_untrusted` carry `hostKey`, so the
    /// dialog can offer to trust it).
    pub error: Option<ErrorPayload>,
    pub log: Vec<LogLine>,
}
