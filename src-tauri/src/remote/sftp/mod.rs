//! SFTP over SSH (`russh` + `russh-sftp`, pure Rust, `ring` crypto; ADR 0002).
//!
//! One [`SftpConnection`] owns one SSH connection. Browsing and the viewer use its primary SFTP
//! channel; [`Connection::open_secondary`] opens more channels on the same, already
//! authenticated connection for transfers (no new handshake, no new host key check).
//!
//! Connecting: TCP (with the site's timeout) → SSH handshake → host key check against the
//! [`HostVerifier`] (unknown or changed keys abort before authentication) → password, key file
//! or agent authentication → SFTP subsystem. Every step is written to the connection log;
//! secrets never are.

pub mod common;
mod error;
mod fs;
mod session;
mod shell;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;

use self::common::connect_timeout;
use self::fs::{SftpFs, Shared};
pub(crate) use self::session::AgentSource;
use self::session::{authenticate, open_sftp, open_transport};
use self::shell::SftpShell;
use super::{ConnectParams, Connection, HostVerifier, Logger, RemoteFs, RemoteShell};
use crate::error::AppResult;

/// How long one SFTP request may wait for its reply.
fn request_timeout(timeout_secs: u64) -> Duration {
    Duration::from_secs(timeout_secs.saturating_mul(2).max(30))
}

/// Opens an SFTP connection (see the module docs for the steps and errors).
pub async fn connect(
    params: &ConnectParams,
    verifier: Arc<dyn HostVerifier>,
    logger: Logger,
) -> AppResult<Arc<dyn Connection>> {
    connect_with(params, verifier, logger, AgentSource::Environment).await
}

/// [`connect`] with an explicit agent location (tests start their own agent).
pub(crate) async fn connect_with(
    params: &ConnectParams,
    verifier: Arc<dyn HostVerifier>,
    logger: Logger,
    agent: AgentSource,
) -> AppResult<Arc<dyn Connection>> {
    let timeout = connect_timeout(params.timeout_secs);
    logger.status(format!(
        "Connecting to {}:{} (SFTP)…",
        params.host, params.port
    ));
    let transport = open_transport(params, verifier, &logger, timeout).await?;
    let mut handle = transport.handle;
    authenticate(&mut handle, params, &agent, &logger, timeout).await?;
    let shared = Arc::new(Shared {
        handle,
        closed: Default::default(),
    });
    let request_timeout = request_timeout(params.timeout_secs);
    let primary = open_channel(&shared, &logger, request_timeout).await?;
    Ok(Arc::new(SftpConnection {
        shared,
        primary,
        secondaries: Mutex::new(Vec::new()),
        server_id: transport.server_id,
        logger,
        request_timeout,
    }))
}

async fn open_channel(
    shared: &Arc<Shared>,
    logger: &Logger,
    request_timeout: Duration,
) -> AppResult<Arc<SftpFs>> {
    let (sftp, extensions) = match open_sftp(&shared.handle, request_timeout).await {
        Ok(opened) => opened,
        Err(error) => {
            logger.error(error.to_string());
            return Err(error);
        }
    };
    let mut names = Vec::new();
    if extensions.posix_rename {
        names.push("posix-rename");
    }
    if extensions.fsync {
        names.push("fsync");
    }
    logger.status(if names.is_empty() {
        "SFTP channel ready".to_owned()
    } else {
        format!("SFTP channel ready ({})", names.join(", "))
    });
    Ok(Arc::new(SftpFs::new(
        sftp,
        extensions,
        Arc::clone(shared),
        logger.clone(),
    )))
}

/// A live SFTP connection: one SSH transport, a primary channel and transfer channels.
pub struct SftpConnection {
    shared: Arc<Shared>,
    primary: Arc<SftpFs>,
    secondaries: Mutex<Vec<Weak<SftpFs>>>,
    server_id: Option<String>,
    logger: Logger,
    request_timeout: Duration,
}

#[async_trait]
impl Connection for SftpConnection {
    fn fs(&self) -> Arc<dyn RemoteFs> {
        self.primary.clone()
    }

    async fn open_secondary(&self) -> AppResult<Arc<dyn RemoteFs>> {
        if !self.shared.is_alive() {
            return Err(crate::error::AppError::Disconnected);
        }
        let channel = open_channel(&self.shared, &self.logger, self.request_timeout).await?;
        if let Ok(mut secondaries) = self.secondaries.lock() {
            secondaries.retain(|weak| weak.strong_count() > 0);
            secondaries.push(Arc::downgrade(&channel));
        }
        Ok(channel)
    }

    fn server_info(&self) -> Option<String> {
        self.server_id.clone()
    }

    fn shell(&self) -> Option<Arc<dyn RemoteShell>> {
        Some(Arc::new(SftpShell::new(
            Arc::clone(&self.shared),
            self.logger.clone(),
        )))
    }

    fn is_alive(&self) -> bool {
        self.shared.is_alive()
    }

    async fn close(&self) {
        if self.shared.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let secondaries: Vec<Arc<SftpFs>> = self
            .secondaries
            .lock()
            .map(|list| list.iter().filter_map(Weak::upgrade).collect())
            .unwrap_or_default();
        for channel in secondaries {
            channel.close_channel();
        }
        self.primary.close_channel();
        let _ = self
            .shared
            .handle
            .disconnect(russh::Disconnect::ByApplication, "closed by the user", "en")
            .await;
        self.logger.status("Disconnected");
    }
}

/// Test support: sets the modification time of `paths` over one short-lived connection (the
/// test servers share one tree, so this also ages files seen over FTP).
#[cfg(test)]
pub(crate) async fn set_mtime(
    params: &ConnectParams,
    verifier: Arc<dyn HostVerifier>,
    paths: &[&str],
    mtime: u32,
) -> AppResult<()> {
    let logger = Logger::noop();
    let timeout = connect_timeout(params.timeout_secs);
    let transport = open_transport(params, verifier, &logger, timeout).await?;
    let mut handle = transport.handle;
    authenticate(
        &mut handle,
        params,
        &AgentSource::Environment,
        &logger,
        timeout,
    )
    .await?;
    let (sftp, _) = open_sftp(&handle, request_timeout(params.timeout_secs)).await?;
    let attrs = russh_sftp::protocol::FileAttributes {
        atime: Some(mtime),
        mtime: Some(mtime),
        ..russh_sftp::protocol::FileAttributes::empty()
    };
    let mut result = Ok(());
    for path in paths {
        if let Err(error) = sftp.setstat(*path, attrs.clone()).await {
            result = Err(crate::error::AppError::Remote(error.to_string()));
        }
    }
    let _ = handle
        .disconnect(russh::Disconnect::ByApplication, "", "en")
        .await;
    result
}
