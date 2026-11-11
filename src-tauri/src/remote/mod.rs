//! The protocol layer: one [`RemoteFs`] trait implemented by SFTP and FTP/FTPS.
//!
//! Services and the UI never branch on the protocol; optional features are reported through
//! [`Capabilities`]. Every implementation logs what it does on the wire through a [`Logger`],
//! redacting secrets at the source (see docs/architecture.md#connection-log).

pub mod ftp;
#[cfg(test)]
pub(crate) mod it;
pub mod logger;
pub mod memory;
pub mod path;
pub mod sftp;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::domain::command::{CommandExit, CommandStream};
use crate::domain::entry::Entry;
use crate::domain::session::{Capabilities, HostKeyInfo};
use crate::domain::site::{FtpsMode, Protocol};
use crate::error::AppResult;

pub use logger::Logger;

/// Called with the number of bytes transferred so far.
pub type Progress<'a> = &'a (dyn Fn(u64) + Send + Sync);

/// How a write ended up on the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteOutcome {
    /// Temp file + rename; false when the file was overwritten in place.
    pub atomic: bool,
}

/// File operations on one logical connection. Paths are absolute POSIX paths.
///
/// Implementations must be safe to call concurrently (`&self`); a protocol that cannot
/// pipeline commands (FTP) serialises internally.
#[async_trait]
pub trait RemoteFs: Send + Sync {
    fn capabilities(&self) -> Capabilities;

    /// The canonical home (initial working) directory.
    async fn home(&self) -> AppResult<String>;

    /// Resolves `.`/`..`/symlinks of a directory path to its canonical form.
    async fn canonicalize(&self, path: &str) -> AppResult<String>;

    /// Lists a directory, without `.` and `..`. Symlinks are resolved best-effort into
    /// `target_kind`.
    async fn list(&self, path: &str) -> AppResult<Vec<Entry>>;

    /// Metadata of a path, following symlinks. `NotFound` when it does not exist.
    async fn stat(&self, path: &str) -> AppResult<Entry>;

    /// Reads up to `len` bytes starting at `offset`; fewer only at end of file.
    async fn read_range(&self, path: &str, offset: u64, len: u64) -> AppResult<Vec<u8>>;

    /// Replaces (or creates) `path` with `data` without ever leaving it half-written:
    /// write a temp sibling, apply `mode`, then rename over the target. Falls back to an
    /// in-place overwrite only when `allow_in_place` is true and the directory is not writable.
    async fn write_atomic(
        &self,
        path: &str,
        data: &[u8],
        mode: Option<u32>,
        allow_in_place: bool,
    ) -> AppResult<WriteOutcome>;

    /// Creates an empty file; `Conflict` if it already exists.
    async fn create_file(&self, path: &str) -> AppResult<()>;

    async fn mkdir(&self, path: &str) -> AppResult<()>;

    /// Renames `from` to `to`; `Conflict` if `to` exists (never overwrites).
    async fn rename(&self, from: &str, to: &str) -> AppResult<()>;

    /// Moves a complete temp file over `target`, replacing it if it exists, with the same
    /// guarantees as the final step of [`RemoteFs::write_atomic`]: atomic where the protocol
    /// allows it, otherwise the backup dance that restores the original on failure. Used to
    /// finish uploads. Returns whether the replacement was atomic.
    async fn replace(&self, temp: &str, target: &str) -> AppResult<WriteOutcome>;

    async fn remove_file(&self, path: &str) -> AppResult<()>;

    /// Removes an empty directory.
    async fn remove_dir(&self, path: &str) -> AppResult<()>;

    async fn chmod(&self, path: &str, mode: u32) -> AppResult<()>;

    /// Streams `path` (from `offset`) into `sink`, calling `progress` with the running total.
    /// Returns the number of bytes written. Stops with `Cancelled` when `cancel` is set.
    async fn download(
        &self,
        path: &str,
        offset: u64,
        sink: &mut (dyn AsyncWrite + Send + Unpin),
        progress: Progress<'_>,
        cancel: &AtomicBool,
    ) -> AppResult<u64>;

    /// Streams `source` into `path` (created or truncated). Returns the bytes written.
    async fn upload(
        &self,
        source: &mut (dyn AsyncRead + Send + Unpin),
        path: &str,
        progress: Progress<'_>,
        cancel: &AtomicBool,
    ) -> AppResult<u64>;

    /// A cheap round trip that keeps the connection alive and detects drops.
    async fn keepalive(&self) -> AppResult<()>;

    /// Closes the connection politely. Further calls fail with `Disconnected`.
    async fn close(&self);
}

/// A live, authenticated connection to a server.
#[async_trait]
pub trait Connection: Send + Sync {
    /// The file system used for browsing and the viewer.
    fn fs(&self) -> Arc<dyn RemoteFs>;

    /// An independent file system for a transfer, so transfers never block browsing:
    /// a new SFTP channel on the same SSH connection, or a new FTP control connection.
    async fn open_secondary(&self) -> AppResult<Arc<dyn RemoteFs>>;

    /// Server identification (SSH banner or FTP welcome line).
    fn server_info(&self) -> Option<String>;

    /// Whether the underlying transport is still up (best effort, no I/O).
    fn is_alive(&self) -> bool;

    /// Runs commands on the server (SSH only). `None` when the protocol has no shell (FTP).
    fn shell(&self) -> Option<Arc<dyn RemoteShell>> {
        None
    }

    /// Closes every channel and the transport.
    async fn close(&self);
}

/// Called with each chunk of command output as it arrives.
pub type OutputSink<'a> = &'a (dyn Fn(CommandStream, &[u8]) + Send + Sync);

/// Remote command execution over an existing, authenticated connection (ADR 0014).
#[async_trait]
pub trait RemoteShell: Send + Sync {
    /// Runs `command` through the user's login shell on a new channel without a PTY, streaming
    /// stdout and stderr to `output` in arrival order. Returns when the command ends. When
    /// `cancel` is set, asks the server to stop it (signal `TERM`, then closes the channel) and
    /// returns `CommandExit { cancelled: true, .. }`. Logs the command and its exit status to the
    /// connection log (never its output).
    async fn exec(
        &self,
        command: &str,
        output: OutputSink<'_>,
        cancel: &AtomicBool,
    ) -> AppResult<CommandExit>;
}

/// Credentials for one connection attempt. Never logged, never serialised.
#[derive(Clone)]
pub enum Auth {
    /// FTP/FTPS with an empty user means anonymous.
    Password(String),
    Key {
        path: String,
        passphrase: Option<String>,
    },
    Agent,
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Password(_) => f.write_str("Password(<redacted>)"),
            Self::Key { path, .. } => write!(f, "Key({path})"),
            Self::Agent => f.write_str("Agent"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConnectParams {
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: Auth,
    pub ftps_mode: FtpsMode,
    pub passive: bool,
    /// Seconds before a connection attempt gives up.
    pub timeout_secs: u64,
}

/// What the trust store says about a server identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostDecision {
    Trusted,
    Unknown,
    /// A different key is stored for this host and port.
    Changed {
        previous_fingerprint: String,
    },
}

/// Looks up SSH host keys and TLS certificate fingerprints the user trusted (TOFU).
pub trait HostVerifier: Send + Sync {
    fn check(&self, host: &str, port: u16, key_type: &str, fingerprint: &str) -> HostDecision;
}

/// Opens and authenticates a connection. Errors: `HostKeyUnknown`/`HostKeyChanged`/
/// `CertificateUntrusted` (with the key to show), `AuthFailed`, `SecretRequired` (encrypted key
/// without passphrase), `Connection`.
pub async fn connect(
    params: &ConnectParams,
    verifier: Arc<dyn HostVerifier>,
    logger: Logger,
) -> AppResult<Arc<dyn Connection>> {
    match params.protocol {
        Protocol::Sftp => sftp::connect(params, verifier, logger).await,
        Protocol::Ftp | Protocol::Ftps => ftp::connect(params, verifier, logger).await,
    }
}

/// Builds the `HostKeyInfo` returned with trust errors.
pub fn host_key_info(
    params: &ConnectParams,
    key_type: &str,
    fingerprint: &str,
    previous: Option<String>,
) -> HostKeyInfo {
    HostKeyInfo {
        host: params.host.clone(),
        port: params.port,
        key_type: key_type.to_owned(),
        fingerprint: fingerprint.to_owned(),
        previous_fingerprint: previous,
    }
}
