//! Live sessions: one per connection tab (docs/adr/0010-session-and-event-model.md).
//!
//! A [`Session`] owns the authenticated [`Connection`], the primary [`RemoteFs`] used for
//! browsing and the viewer, its transfer queue and the event sink of its tab. Everything the
//! session has to tell the tab goes through that one sink as [`SessionEvent`]s: log lines
//! (batched every ~16 ms), state changes and transfer progress.
//!
//! Sessions survive a dropped connection: the state becomes `disconnected` (operations then
//! fail with `Disconnected`) and `reconnect` reopens the same session id with the same sink,
//! so open file tabs and the log keep working. Only `disconnect` (closing the tab) removes it.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use async_trait::async_trait;
use tokio::task::JoinHandle;

use super::blocking;
use super::commands::Runs;
use super::known_hosts::KnownHosts;
use super::secrets::SecretStore;
use super::transfers::TransferQueue;
use super::{settings, sites};
use crate::domain::log::LogLine;
use crate::domain::session::{
    Capabilities, ConnectTarget, SessionEvent, SessionInfo, SessionState, TestResult,
};
use crate::domain::site::{AuthMethod, SiteInput};
use crate::error::{AppError, AppResult, ErrorPayload};
use crate::remote::{
    self, Auth, ConnectParams, Connection, HostVerifier, Logger, RemoteFs, RemoteShell,
};
use crate::storage::Database;

/// Where a session's events go (the tab's `tauri::ipc::Channel` in the app).
pub type EventSink = Arc<dyn Fn(SessionEvent) + Send + Sync>;

/// Seconds before a connection attempt gives up.
pub const CONNECT_TIMEOUT_SECS: u64 = 20;

/// Log lines are sent to the UI in batches at most this often.
const LOG_BATCH: Duration = Duration::from_millis(16);

/// Opens connections. The app uses [`RemoteConnector`]; tests use the in-memory remote.
#[async_trait]
pub trait Connector: Send + Sync {
    async fn connect(
        &self,
        params: &ConnectParams,
        verifier: Arc<dyn HostVerifier>,
        logger: Logger,
    ) -> AppResult<Arc<dyn Connection>>;
}

/// Connects through the real protocol implementations ([`remote::connect`]).
pub struct RemoteConnector;

#[async_trait]
impl Connector for RemoteConnector {
    async fn connect(
        &self,
        params: &ConnectParams,
        verifier: Arc<dyn HostVerifier>,
        logger: Logger,
    ) -> AppResult<Arc<dyn Connection>> {
        remote::connect(params, verifier, logger).await
    }
}

/// A logger whose lines reach `events` as `SessionEvent::Log` batches.
fn batched_logger(events: EventSink) -> Logger {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<LogLine>();
    tokio::spawn(async move {
        while let Some(first) = rx.recv().await {
            tokio::time::sleep(LOG_BATCH).await;
            let mut lines = vec![first];
            while let Ok(line) = rx.try_recv() {
                lines.push(line);
            }
            events(SessionEvent::Log { lines });
        }
    });
    Logger::new(move |line| {
        let _ = tx.send(line);
    })
}

/// A logger that keeps its lines (for "Test connection").
fn collecting_logger() -> (Logger, Arc<Mutex<Vec<LogLine>>>) {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&lines);
    let logger = Logger::new(move |line| {
        sink.lock().unwrap_or_else(|e| e.into_inner()).push(line);
    });
    (logger, lines)
}

/// What a session needs to connect: the site as it is now and its id, if saved.
#[derive(Clone)]
struct Target {
    site_id: Option<String>,
    /// The keyring holds a secret for this site (only then is it read).
    has_secret: bool,
    input: SiteInput,
}

/// The secret that authenticated a session, kept in memory for the session's lifetime so a
/// reconnect does not ask again. Never logged, serialised or written anywhere; dropped on
/// disconnect and after a failed authentication.
#[derive(Clone)]
struct SessionSecret(String);

impl std::fmt::Debug for SessionSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionSecret(<redacted>)")
    }
}

fn label(target: &Target) -> String {
    if target.site_id.is_some() {
        return target.input.name.clone();
    }
    if target.input.user.is_empty() {
        target.input.host.clone()
    } else {
        format!("{}@{}", target.input.user, target.input.host)
    }
}

/// Resolves a connect target to a validated site (re-read from the database for saved sites).
async fn resolve_target(db: &Arc<Database>, target: ConnectTarget) -> AppResult<Target> {
    match target {
        ConnectTarget::Site { id } => {
            let db = Arc::clone(db);
            let site = blocking(move || db.read(|c| sites::get(c, &id))).await?;
            Ok(Target {
                site_id: Some(site.id.clone()),
                has_secret: site.has_secret,
                input: site.input(),
            })
        }
        ConnectTarget::Adhoc { mut site } => {
            if site.name.trim().is_empty() {
                // Quick connect has no name; it is labelled `user@host`.
                site.name = label(&Target {
                    site_id: None,
                    has_secret: false,
                    input: site.clone(),
                });
            }
            Ok(Target {
                site_id: None,
                has_secret: false,
                input: sites::normalise(site)?,
            })
        }
    }
}

struct Live {
    state: SessionState,
    target: Target,
    connection: Option<Arc<dyn Connection>>,
    fs: Option<Arc<dyn RemoteFs>>,
    info: Option<SessionInfo>,
    keepalive: Option<JoinHandle<()>>,
    secret: Option<SessionSecret>,
}

/// One connection tab's session.
pub struct Session {
    pub id: String,
    events: EventSink,
    logger: Logger,
    live: Mutex<Live>,
    /// Serialises connect/reconnect of this session.
    connecting: tokio::sync::Mutex<()>,
    pub transfers: Arc<TransferQueue>,
    /// Remote commands running on this session's connection (ADR 0014).
    pub runs: Runs,
}

impl Session {
    fn lock(&self) -> MutexGuard<'_, Live> {
        self.live.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[cfg(test)]
    pub fn state(&self) -> SessionState {
        self.lock().state
    }

    /// The file system for browsing and the viewer; `Disconnected` unless connected.
    pub fn fs(&self) -> AppResult<Arc<dyn RemoteFs>> {
        let live = self.lock();
        match (&live.state, &live.fs) {
            (SessionState::Connected, Some(fs)) => Ok(Arc::clone(fs)),
            _ => Err(AppError::Disconnected),
        }
    }

    /// The connection's shell for remote commands: `Disconnected` unless connected,
    /// `Unsupported` on protocols without one (FTP).
    pub fn shell(&self) -> AppResult<Arc<dyn RemoteShell>> {
        let live = self.lock();
        let connection = match (&live.state, &live.connection) {
            (SessionState::Connected, Some(connection)) => connection,
            _ => return Err(AppError::Disconnected),
        };
        connection
            .shell()
            .ok_or_else(|| AppError::Unsupported("Commands need an SSH (SFTP) connection".into()))
    }

    /// The canonical home directory of the current connection.
    pub fn home(&self) -> Option<String> {
        self.lock().info.as_ref().map(|i| i.home.clone())
    }

    /// Passes `result` through, noticing when an operation failed because the connection
    /// dropped (the session then becomes disconnected and the tab is told).
    pub fn observe<T>(&self, result: AppResult<T>) -> AppResult<T> {
        if let Err(error @ (AppError::Connection(_) | AppError::Disconnected)) = &result {
            let dead = self
                .lock()
                .connection
                .as_ref()
                .is_some_and(|c| !c.is_alive());
            if dead {
                self.mark_disconnected(error.to_string());
            }
        }
        result
    }

    /// Marks the session disconnected if its connection died, for operations that report
    /// failures without returning an error (deleting a selection).
    pub fn check_alive(&self) {
        let dead = self
            .lock()
            .connection
            .as_ref()
            .is_some_and(|c| !c.is_alive());
        if dead {
            self.mark_disconnected("the connection was lost".into());
        }
    }

    fn emit_state(&self, state: SessionState, reason: Option<String>) {
        (self.events)(SessionEvent::State { state, reason });
    }

    /// The connection dropped: tell the tab, stop the keepalive, park the transfer queue.
    pub fn mark_disconnected(&self, reason: String) {
        let connection = {
            let mut live = self.lock();
            if live.state != SessionState::Connected {
                return;
            }
            live.state = SessionState::Disconnected;
            live.fs = None;
            if let Some(task) = live.keepalive.take() {
                task.abort();
            }
            live.connection.take()
        };
        self.logger.error(format!("Connection lost: {reason}"));
        self.transfers.set_connection(None);
        self.emit_state(SessionState::Disconnected, Some(reason));
        if let Some(connection) = connection {
            tokio::spawn(async move { connection.close().await });
        }
    }

    /// Closes the current connection, if any, without emitting anything.
    async fn close_connection(&self) {
        let connection = {
            let mut live = self.lock();
            live.fs = None;
            if let Some(task) = live.keepalive.take() {
                task.abort();
            }
            live.connection.take()
        };
        self.transfers.set_connection(None);
        if let Some(connection) = connection {
            connection.close().await;
        }
    }
}

/// All live sessions, managed by Tauri.
pub struct SessionManager {
    sessions: Mutex<HashMap<String, Arc<Session>>>,
    connector: Arc<dyn Connector>,
    secrets: Arc<dyn SecretStore>,
    /// Tests use a short keepalive period instead of the settings' seconds.
    keepalive_override: Option<Duration>,
}

impl SessionManager {
    pub fn new(connector: Arc<dyn Connector>, secrets: Arc<dyn SecretStore>) -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            connector,
            secrets,
            keepalive_override: None,
        }
    }

    #[cfg(test)]
    pub fn with_keepalive_period(mut self, period: Duration) -> Self {
        self.keepalive_override = Some(period);
        self
    }

    fn map(&self) -> MutexGuard<'_, HashMap<String, Arc<Session>>> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn secrets(&self) -> Arc<dyn SecretStore> {
        Arc::clone(&self.secrets)
    }

    pub fn get(&self, id: &str) -> AppResult<Arc<Session>> {
        self.map()
            .get(id)
            .cloned()
            .ok_or_else(|| AppError::not_found("session"))
    }

    /// The session whose queue holds the transfer `id`.
    pub fn session_of_transfer(&self, id: &str) -> AppResult<Arc<Session>> {
        self.map()
            .values()
            .find(|s| s.transfers.contains(id))
            .cloned()
            .ok_or_else(|| AppError::not_found("transfer"))
    }

    /// Stops a remote command, wherever it runs. Unknown or finished runs are fine.
    pub fn cancel_run(&self, run_id: &str) {
        for session in self.map().values() {
            if session.runs.cancel(run_id) {
                return;
            }
        }
    }

    /// Opens a new session. `secret` overrides the stored one; with `remember`, it is stored
    /// in the keyring once authentication succeeded (saved sites only).
    pub async fn connect(
        &self,
        db: Arc<Database>,
        target: ConnectTarget,
        secret: Option<String>,
        remember: bool,
        events: EventSink,
    ) -> AppResult<SessionInfo> {
        let target = resolve_target(&db, target).await?;
        let id = uuid::Uuid::now_v7().to_string();
        let logger = batched_logger(Arc::clone(&events));
        let session = Arc::new(Session {
            id: id.clone(),
            transfers: TransferQueue::new(id.clone(), Arc::clone(&events), logger.clone()),
            logger,
            events,
            live: Mutex::new(Live {
                state: SessionState::Connecting,
                target,
                connection: None,
                fs: None,
                info: None,
                keepalive: None,
                secret: None,
            }),
            connecting: tokio::sync::Mutex::new(()),
            runs: Runs::default(),
        });
        let info = self.establish(&session, &db, secret, remember).await?;
        self.map().insert(id, session);
        Ok(info)
    }

    /// Reopens a session (dropped or not) with the same id, site and event sink.
    pub async fn reconnect(
        &self,
        db: Arc<Database>,
        id: &str,
        secret: Option<String>,
    ) -> AppResult<SessionInfo> {
        let session = self.get(id)?;
        let site_id = session.lock().target.site_id.clone();
        if let Some(site_id) = site_id {
            // Pick up edits made to the site since the tab was opened.
            match resolve_target(&db, ConnectTarget::Site { id: site_id }).await {
                Ok(target) => session.lock().target = target,
                Err(error) => log::warn!("reconnecting with the previous site settings: {error}"),
            }
        }
        self.establish(&session, &db, secret, false).await
    }

    /// Closes a session: cancels its transfers, closes the connection and forgets it.
    pub async fn disconnect(&self, id: &str) -> AppResult<()> {
        let Some(session) = self.map().remove(id) else {
            return Ok(());
        };
        session.runs.stop_all(Duration::from_secs(2)).await;
        session.transfers.shutdown().await;
        session.close_connection().await;
        let was_connected = {
            let mut live = session.lock();
            live.secret = None;
            std::mem::replace(&mut live.state, SessionState::Disconnected)
                == SessionState::Connected
        };
        if was_connected {
            session.logger.status("Disconnected");
        }
        session.emit_state(SessionState::Disconnected, None);
        Ok(())
    }

    /// Closes every session (app exit).
    pub async fn close_all(&self) {
        let ids: Vec<String> = self.map().keys().cloned().collect();
        for id in ids {
            let _ = self.disconnect(&id).await;
        }
    }

    /// Connects, reads the home directory and disconnects; returns the log of the attempt.
    /// Connection-level failures (authentication, trust, network, missing secret) are results
    /// with `ok == false`, so the dialog can show the conversation; only invalid input and
    /// storage errors are errors.
    pub async fn test_connection(
        &self,
        db: Arc<Database>,
        site: SiteInput,
        secret: Option<String>,
        site_id: Option<String>,
    ) -> AppResult<TestResult> {
        let mut site = site;
        if site.name.trim().is_empty() {
            site.name = site.host.clone();
        }
        let input = sites::normalise(site)?;
        let (logger, lines) = collecting_logger();
        let attempt = self
            .try_connection(&db, site_id.as_deref(), &input, secret, &logger)
            .await;
        let (server_info, home, error) = match attempt {
            Ok((server_info, home)) => {
                logger.status(format!("Test succeeded · home folder {home}"));
                (server_info, Some(home), None)
            }
            Err(error @ (AppError::Storage(_) | AppError::Internal(_))) => return Err(error),
            Err(error) => {
                logger.error(format!("Test failed: {error}"));
                (None, None, Some(ErrorPayload::from(&error)))
            }
        };
        let log = lines.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Ok(TestResult {
            ok: error.is_none(),
            server_info,
            home,
            error,
            log,
        })
    }

    async fn try_connection(
        &self,
        db: &Arc<Database>,
        site_id: Option<&str>,
        input: &SiteInput,
        secret: Option<String>,
        logger: &Logger,
    ) -> AppResult<(Option<String>, String)> {
        let has_secret = match site_id {
            Some(id) => {
                let (db, id) = (Arc::clone(db), id.to_owned());
                blocking(move || db.read(|c| sites::get(c, &id)))
                    .await
                    .is_ok_and(|site| site.has_secret)
            }
            None => false,
        };
        let stored = if has_secret { site_id } else { None };
        let secret = self
            .resolve_secret(stored, input, secret, None, logger)
            .await?;
        let params = connect_params(input, secret);
        let verifier: Arc<dyn HostVerifier> = Arc::new(KnownHosts::new(Arc::clone(db)));
        let connection = self
            .connector
            .connect(&params, verifier, logger.clone())
            .await?;
        let home = connection.fs().home().await;
        let server_info = connection.server_info();
        connection.close().await;
        Ok((server_info, home?))
    }

    /// Password or passphrase: the argument, then the secret that authenticated this session
    /// before (reconnect), then the keyring (only for a saved site that has a secret there),
    /// else `SecretRequired` when the method needs one. A key may or may not be encrypted: the
    /// remote layer says.
    async fn resolve_secret(
        &self,
        stored_for: Option<&str>,
        input: &SiteInput,
        secret: Option<String>,
        in_memory: Option<SessionSecret>,
        logger: &Logger,
    ) -> AppResult<Option<String>> {
        if secret.is_some() {
            return Ok(secret);
        }
        if let Some(SessionSecret(remembered)) = in_memory {
            return Ok(Some(remembered));
        }
        if let Some(site_id) = stored_for {
            let store = Arc::clone(&self.secrets);
            let site_id = site_id.to_owned();
            match blocking(move || store.get(&site_id)).await {
                Ok(Some(stored)) => return Ok(Some(stored)),
                Ok(None) => {}
                Err(error) => {
                    log::warn!("could not read a saved secret: {error}");
                    logger.status(
                        "The saved password could not be read from the system keyring; \
                         enter it to connect",
                    );
                }
            }
        }
        if input.needs_password() {
            Err(AppError::SecretRequired)
        } else {
            Ok(None)
        }
    }

    async fn establish(
        &self,
        session: &Arc<Session>,
        db: &Arc<Database>,
        secret: Option<String>,
        remember: bool,
    ) -> AppResult<SessionInfo> {
        let _guard = session.connecting.lock().await;
        session.close_connection().await;
        let target = {
            let mut live = session.lock();
            live.state = SessionState::Connecting;
            live.target.clone()
        };
        session.emit_state(SessionState::Connecting, None);
        match self.open(session, db, &target, secret, remember).await {
            Ok(info) => Ok(info),
            Err(error) => {
                let mut live = session.lock();
                live.state = SessionState::Disconnected;
                if matches!(error, AppError::AuthFailed(_) | AppError::SecretRequired) {
                    // What worked before no longer does: ask the user next time.
                    live.secret = None;
                }
                drop(live);
                session.emit_state(SessionState::Disconnected, Some(error.to_string()));
                Err(error)
            }
        }
    }

    async fn open(
        &self,
        session: &Arc<Session>,
        db: &Arc<Database>,
        target: &Target,
        secret: Option<String>,
        remember: bool,
    ) -> AppResult<SessionInfo> {
        let logger = session.logger.clone();
        let settings = {
            let db = Arc::clone(db);
            blocking(move || db.read(settings::get))
                .await
                .unwrap_or_default()
        };
        let typed = secret.is_some();
        let in_memory = session.lock().secret.clone();
        let stored_for = target.site_id.as_deref().filter(|_| target.has_secret);
        let secret = self
            .resolve_secret(stored_for, &target.input, secret, in_memory, &logger)
            .await?;
        let params = connect_params(&target.input, secret.clone());
        let verifier: Arc<dyn HostVerifier> = Arc::new(KnownHosts::new(Arc::clone(db)));
        let connection = match self
            .connector
            .connect(&params, verifier, logger.clone())
            .await
        {
            Ok(connection) => connection,
            Err(error) => {
                if error.host_key().is_none() && !matches!(error, AppError::SecretRequired) {
                    logger.error(format!("Could not connect: {error}"));
                }
                return Err(error);
            }
        };
        let fs = connection.fs();
        let home = match fs.home().await {
            Ok(home) => home,
            Err(error) => {
                logger.error(format!("Could not read the home folder: {error}"));
                connection.close().await;
                return Err(error);
            }
        };
        let start_path = match &target.input.initial_path {
            None => home.clone(),
            Some(path) => match fs.canonicalize(path).await {
                Ok(path) => path,
                Err(error) => {
                    logger.status(format!(
                        "The initial folder {path} is not available ({error}); opening the \
                         home folder"
                    ));
                    home.clone()
                }
            },
        };
        let used = secret.clone();
        if remember
            && typed
            && let (Some(site_id), Some(secret)) = (&target.site_id, secret)
        {
            self.remember(db, site_id, secret, &logger).await;
        }
        let info = SessionInfo {
            id: session.id.clone(),
            site_id: target.site_id.clone(),
            label: label(target),
            protocol: target.input.protocol,
            host: target.input.host.clone(),
            port: target.input.port,
            user: target.input.user.clone(),
            home,
            start_path,
            insecure: target.input.protocol.is_insecure(),
            server_info: connection.server_info(),
            capabilities: Capabilities {
                exec: connection.shell().is_some(),
                ..fs.capabilities()
            },
        };
        let keepalive = self.start_keepalive(session, &connection, &fs, settings.keepalive_secs);
        {
            let mut live = session.lock();
            live.state = SessionState::Connected;
            live.connection = Some(Arc::clone(&connection));
            live.fs = Some(fs);
            live.info = Some(info.clone());
            live.keepalive = keepalive;
            live.secret = used.map(SessionSecret);
        }
        session.transfers.set_connection(Some(connection));
        session.emit_state(SessionState::Connected, None);
        Ok(info)
    }

    /// Stores a secret that just authenticated. Failures are logged, never fatal.
    async fn remember(&self, db: &Arc<Database>, site_id: &str, secret: String, logger: &Logger) {
        let store = Arc::clone(&self.secrets);
        let db = Arc::clone(db);
        let id = site_id.to_owned();
        let stored = blocking(move || {
            store.set(&id, &secret)?;
            db.write(|tx| sites::set_has_secret(tx, &id, true))
        })
        .await;
        match stored {
            Ok(()) => logger.status("Password saved in the system keyring"),
            Err(error) => logger.error(format!("Could not save the password: {error}")),
        }
    }

    fn start_keepalive(
        &self,
        session: &Arc<Session>,
        connection: &Arc<dyn Connection>,
        fs: &Arc<dyn RemoteFs>,
        secs: u32,
    ) -> Option<JoinHandle<()>> {
        let period = match self.keepalive_override {
            Some(period) => period,
            None if secs == 0 => return None,
            None => Duration::from_secs(u64::from(secs)),
        };
        let weak: Weak<Session> = Arc::downgrade(session);
        let connection = Arc::clone(connection);
        let fs = Arc::clone(fs);
        Some(tokio::spawn(async move {
            loop {
                tokio::time::sleep(period).await;
                let result = if connection.is_alive() {
                    fs.keepalive().await
                } else {
                    Err(AppError::Connection("the connection was closed".into()))
                };
                let Some(session) = weak.upgrade() else {
                    break;
                };
                if let Err(error) = result {
                    session.mark_disconnected(error.to_string());
                    break;
                }
            }
        }))
    }
}

fn connect_params(input: &SiteInput, secret: Option<String>) -> ConnectParams {
    let auth = match input.auth {
        AuthMethod::Password => Auth::Password(secret.unwrap_or_default()),
        AuthMethod::Key => Auth::Key {
            path: input.key_path.clone().unwrap_or_default(),
            passphrase: secret,
        },
        AuthMethod::Agent => Auth::Agent,
    };
    ConnectParams {
        protocol: input.protocol,
        host: input.host.clone(),
        port: input.port,
        user: input.user.clone(),
        auth,
        ftps_mode: input.ftps_mode,
        passive: input.passive,
        timeout_secs: CONNECT_TIMEOUT_SECS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::site::Protocol;
    use crate::error::ErrorCode;
    use crate::services::test_support::*;

    #[tokio::test]
    async fn connects_with_the_stored_secret_and_streams_log_and_state() {
        let env = Env::new();
        let site = env.site_with_secret(Some(PASSWORD));
        let (events, sink) = recorder();
        let info = env
            .manager
            .connect(
                env.db(),
                ConnectTarget::Site {
                    id: site.id.clone(),
                },
                None,
                false,
                sink,
            )
            .await
            .unwrap();
        assert_eq!(info.site_id.as_deref(), Some(site.id.as_str()));
        assert_eq!(info.label, "prod");
        assert_eq!(info.home, "/home/user");
        assert_eq!(info.start_path, "/home/user");
        assert!(!info.insecure);
        assert_eq!(info.server_info.as_deref(), Some("MemoryFS 1.0"));
        assert_eq!(env.connector.secrets_used(), [Some(PASSWORD.to_owned())]);
        let events = wait_for(&events, |e| has_log(e, "Connecting to")).await;
        assert!(matches!(
            events.first(),
            Some(SessionEvent::State {
                state: SessionState::Connecting,
                ..
            })
        ));
        assert!(events.iter().any(|e| matches!(
            e,
            SessionEvent::State {
                state: SessionState::Connected,
                ..
            }
        )));
        // The log never shows the password.
        assert!(
            !log_text(&events).contains(PASSWORD),
            "{}",
            log_text(&events)
        );
    }

    #[tokio::test]
    async fn the_typed_secret_wins_and_is_remembered_only_after_success() {
        let env = Env::new();
        let site = env.site_with_secret(None);
        let target = || ConnectTarget::Site {
            id: site.id.clone(),
        };
        // No stored secret and password auth: ask the user.
        let error = env
            .manager
            .connect(env.db(), target(), None, true, null_sink())
            .await
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::SecretRequired);
        // A wrong password with "remember": nothing is stored.
        let error = env
            .manager
            .connect(env.db(), target(), Some("wrong".into()), true, null_sink())
            .await
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::AuthFailed);
        assert_eq!(env.store.get(&site.id).unwrap(), None);
        assert!(!env.site(&site.id).has_secret);
        // The right one is stored and flagged.
        env.manager
            .connect(env.db(), target(), Some(PASSWORD.into()), true, null_sink())
            .await
            .unwrap();
        assert_eq!(env.store.get(&site.id).unwrap().as_deref(), Some(PASSWORD));
        assert!(env.site(&site.id).has_secret);
        // A typed secret overrides the stored one.
        env.store.set(&site.id, "stale").unwrap();
        env.manager
            .connect(
                env.db(),
                target(),
                Some(PASSWORD.into()),
                false,
                null_sink(),
            )
            .await
            .unwrap();
        assert_eq!(
            env.connector.secrets_used().last().unwrap().as_deref(),
            Some(PASSWORD)
        );
        assert_eq!(env.store.get(&site.id).unwrap().as_deref(), Some("stale"));
    }

    #[tokio::test]
    async fn a_broken_keyring_falls_back_to_asking_with_a_status_line() {
        let env = Env::new();
        let site = env.site_with_secret(Some(PASSWORD));
        env.store.set_broken(true);
        let (events, sink) = recorder();
        let error = env
            .manager
            .connect(
                env.db(),
                ConnectTarget::Site { id: site.id },
                None,
                false,
                sink,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::SecretRequired);
        let seen = wait_for(&events, |e| {
            has_log(e, "could not be read from the system keyring")
        })
        .await;
        let lines: Vec<&LogLine> = seen
            .iter()
            .filter_map(|e| match e {
                SessionEvent::Log { lines } => Some(lines.iter()),
                _ => None,
            })
            .flatten()
            .collect();
        assert!(
            lines
                .iter()
                .all(|l| l.kind != crate::domain::log::LogKind::Error),
            "{lines:?}"
        );
    }

    #[tokio::test]
    async fn sites_without_a_saved_secret_never_touch_the_keyring() {
        let env = Env::new();
        let site = env.site_with_secret(None);
        env.store.set_broken(true);
        let (events, sink) = recorder();
        let error = env
            .manager
            .connect(
                env.db(),
                ConnectTarget::Site {
                    id: site.id.clone(),
                },
                None,
                false,
                sink,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::SecretRequired);
        // Typed secret: connects, and nothing is read either.
        let (_, sink) = recorder();
        env.manager
            .connect(
                env.db(),
                ConnectTarget::Site {
                    id: site.id.clone(),
                },
                Some(PASSWORD.into()),
                false,
                sink,
            )
            .await
            .unwrap();
        let tested = env
            .manager
            .test_connection(env.db(), sftp_input(), None, Some(site.id))
            .await
            .unwrap();
        assert_eq!(tested.error.unwrap().code, ErrorCode::SecretRequired);
        assert_eq!(env.store.reads(), 0);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!log_text(&events.lock().unwrap()).contains("keyring"));
    }

    #[tokio::test]
    async fn reconnect_reuses_the_secret_of_the_session_without_storing_it() {
        let env = Env::new();
        let site = env.site_with_secret(None);
        let info = env
            .manager
            .connect(
                env.db(),
                ConnectTarget::Site {
                    id: site.id.clone(),
                },
                Some(PASSWORD.into()),
                false,
                null_sink(),
            )
            .await
            .unwrap();
        let session = env.manager.get(&info.id).unwrap();
        assert!(!format!("{:?}", session.lock().secret).contains(PASSWORD));
        env.server.drop_connection();
        let _ = session.observe(session.fs().unwrap().list("/").await);
        assert_eq!(session.state(), SessionState::Disconnected);
        env.server.restore();
        env.manager
            .reconnect(env.db(), &info.id, None)
            .await
            .unwrap();
        assert_eq!(
            env.connector.secrets_used().last().unwrap().as_deref(),
            Some(PASSWORD)
        );
        assert_eq!(env.store.get(&site.id).unwrap(), None, "memory only");
        assert!(!env.site(&site.id).has_secret);

        // The password changed on the server: the kept secret fails once, then is forgotten.
        env.connector.set_password("rotated-password");
        assert_eq!(
            env.manager
                .reconnect(env.db(), &info.id, None)
                .await
                .unwrap_err()
                .code(),
            ErrorCode::AuthFailed
        );
        assert!(session.lock().secret.is_none());
        assert_eq!(
            env.manager
                .reconnect(env.db(), &info.id, None)
                .await
                .unwrap_err()
                .code(),
            ErrorCode::SecretRequired
        );
        env.manager
            .reconnect(env.db(), &info.id, Some("rotated-password".into()))
            .await
            .unwrap();
        // Disconnecting forgets it.
        env.manager.disconnect(&info.id).await.unwrap();
        assert!(session.lock().secret.is_none());
    }

    #[test]
    fn session_secrets_are_redacted() {
        let secret = SessionSecret("hunter2-secret".into());
        assert_eq!(format!("{secret:?}"), "SessionSecret(<redacted>)");
    }

    #[tokio::test]
    async fn key_auth_tries_without_passphrase_and_propagates_secret_required() {
        let env = Env::new();
        env.connector.require_passphrase("phrase");
        let input = SiteInput {
            auth: AuthMethod::Key,
            key_path: Some("/keys/id_ed25519".into()),
            ..sftp_input()
        };
        let target = || ConnectTarget::Adhoc {
            site: input.clone(),
        };
        let error = env
            .manager
            .connect(env.db(), target(), None, false, null_sink())
            .await
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::SecretRequired);
        assert_eq!(env.connector.secrets_used(), [None]);
        env.manager
            .connect(
                env.db(),
                target(),
                Some("phrase".into()),
                false,
                null_sink(),
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn adhoc_targets_need_no_name_and_ftp_is_insecure() {
        let env = Env::new();
        let info = env
            .manager
            .connect(
                env.db(),
                ConnectTarget::Adhoc {
                    site: SiteInput {
                        name: String::new(),
                        protocol: Protocol::Ftp,
                        port: 21,
                        ..sftp_input()
                    },
                },
                Some(PASSWORD.into()),
                true,
                null_sink(),
            )
            .await
            .unwrap();
        assert_eq!(info.label, "deploy@example.test");
        assert!(info.insecure);
        assert_eq!(info.site_id, None);
    }

    #[tokio::test]
    async fn host_key_errors_propagate_with_the_key_until_trusted() {
        let env = Env::new();
        env.connector.check_host_keys();
        let target = || ConnectTarget::Adhoc { site: sftp_input() };
        let error = env
            .manager
            .connect(
                env.db(),
                target(),
                Some(PASSWORD.into()),
                false,
                null_sink(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::HostKeyUnknown);
        let key = error.host_key().unwrap().clone();
        env.db()
            .write(|tx| crate::services::known_hosts::trust(tx, &key))
            .unwrap();
        env.manager
            .connect(
                env.db(),
                target(),
                Some(PASSWORD.into()),
                false,
                null_sink(),
            )
            .await
            .unwrap();
        // The server's key changes: refused, with both fingerprints.
        env.connector.set_fingerprint("SHA256:other");
        let error = env
            .manager
            .connect(
                env.db(),
                target(),
                Some(PASSWORD.into()),
                false,
                null_sink(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::HostKeyChanged);
        let info = error.host_key().unwrap();
        assert_eq!(info.fingerprint, "SHA256:other");
        assert_eq!(
            info.previous_fingerprint.as_deref(),
            Some(key.fingerprint.as_str())
        );
    }

    #[tokio::test]
    async fn an_unavailable_initial_path_falls_back_to_home() {
        let env = Env::new();
        env.server.add_dir("/var");
        env.server.add_dir("/var/www");
        let connect = |path: &str| {
            env.manager.connect(
                env.db(),
                ConnectTarget::Adhoc {
                    site: SiteInput {
                        initial_path: Some(path.into()),
                        ..sftp_input()
                    },
                },
                Some(PASSWORD.into()),
                false,
                null_sink(),
            )
        };
        assert_eq!(connect("/var/www/").await.unwrap().start_path, "/var/www");
        let (events, sink) = recorder();
        let info = env
            .manager
            .connect(
                env.db(),
                ConnectTarget::Adhoc {
                    site: SiteInput {
                        initial_path: Some("/missing".into()),
                        ..sftp_input()
                    },
                },
                Some(PASSWORD.into()),
                false,
                sink,
            )
            .await
            .unwrap();
        assert_eq!(info.start_path, "/home/user");
        wait_for(&events, |e| has_log(e, "/missing is not available")).await;
    }

    #[tokio::test]
    async fn keepalive_failure_disconnects_and_reconnect_reuses_the_session() {
        let env = Env::with_keepalive(Duration::from_millis(20));
        let (events, sink) = recorder();
        let info = env
            .manager
            .connect(
                env.db(),
                ConnectTarget::Adhoc { site: sftp_input() },
                Some(PASSWORD.into()),
                false,
                sink,
            )
            .await
            .unwrap();
        let session = env.manager.get(&info.id).unwrap();
        env.server.drop_connection();
        let seen = wait_for(&events, |e| {
            matches!(
                e,
                SessionEvent::State {
                    state: SessionState::Disconnected,
                    reason: Some(_)
                }
            )
        })
        .await;
        assert!(seen.iter().any(|e| matches!(
            e,
            SessionEvent::State {
                state: SessionState::Connected,
                ..
            }
        )));
        wait_for(&events, |e| has_log(e, "Connection lost")).await;
        assert_eq!(session.state(), SessionState::Disconnected);
        assert_eq!(
            session.fs().err().map(|e| e.code()),
            Some(ErrorCode::Disconnected)
        );

        env.server.restore();
        let again = env
            .manager
            .reconnect(env.db(), &info.id, Some(PASSWORD.into()))
            .await
            .unwrap();
        assert_eq!(again.id, info.id);
        assert_eq!(session.state(), SessionState::Connected);
        session.fs().unwrap().list("/home/user").await.unwrap();
        env.manager.disconnect(&info.id).await.unwrap();
        assert_eq!(
            env.manager.get(&info.id).err().map(|e| e.code()),
            Some(ErrorCode::NotFound)
        );
    }

    #[tokio::test]
    async fn a_failed_operation_on_a_dead_connection_marks_the_session() {
        let env = Env::new();
        let (events, sink) = recorder();
        let info = env
            .manager
            .connect(
                env.db(),
                ConnectTarget::Adhoc { site: sftp_input() },
                Some(PASSWORD.into()),
                false,
                sink,
            )
            .await
            .unwrap();
        let session = env.manager.get(&info.id).unwrap();
        env.server.drop_connection();
        let fs = session.fs().unwrap();
        let result = session.observe(fs.list("/").await);
        assert_eq!(result.unwrap_err().code(), ErrorCode::Connection);
        assert_eq!(session.state(), SessionState::Disconnected);
        wait_for(&events, |e| {
            matches!(
                e,
                SessionEvent::State {
                    state: SessionState::Disconnected,
                    ..
                }
            )
        })
        .await;
    }

    #[tokio::test]
    async fn test_connection_returns_the_log_and_closes() {
        let env = Env::new();
        let result = env
            .manager
            .test_connection(env.db(), sftp_input(), Some(PASSWORD.into()), None)
            .await
            .unwrap();
        assert!(result.ok);
        assert!(result.error.is_none());
        assert_eq!(result.home.as_deref(), Some("/home/user"));
        assert_eq!(result.server_info.as_deref(), Some("MemoryFS 1.0"));
        assert!(result.log.iter().any(|l| l.text.contains("Connecting to")));
        assert!(result.log.iter().all(|l| !l.text.contains(PASSWORD)));
    }

    #[tokio::test]
    async fn failed_tests_are_results_with_the_log() {
        let env = Env::new();
        let wrong = env
            .manager
            .test_connection(env.db(), sftp_input(), Some("wrong-password".into()), None)
            .await
            .unwrap();
        assert!(!wrong.ok);
        assert_eq!(wrong.home, None);
        assert_eq!(wrong.error.as_ref().unwrap().code, ErrorCode::AuthFailed);
        assert!(
            wrong
                .log
                .iter()
                .any(|l| l.text.contains("530 Login incorrect"))
        );
        assert!(wrong.log.iter().any(|l| l.text.starts_with("Test failed")));
        assert!(wrong.log.iter().all(|l| !l.text.contains("wrong-password")));

        let missing = env
            .manager
            .test_connection(env.db(), sftp_input(), None, None)
            .await
            .unwrap();
        assert_eq!(missing.error.unwrap().code, ErrorCode::SecretRequired);

        env.connector.check_host_keys();
        let unknown = env
            .manager
            .test_connection(env.db(), sftp_input(), Some(PASSWORD.into()), None)
            .await
            .unwrap();
        let error = unknown.error.unwrap();
        assert_eq!(error.code, ErrorCode::HostKeyUnknown);
        assert_eq!(
            error.host_key.unwrap().fingerprint,
            "SHA256:memory-host-key"
        );

        env.server.drop_connection();
        let down = env
            .manager
            .test_connection(env.db(), sftp_input(), Some(PASSWORD.into()), None)
            .await
            .unwrap();
        assert_eq!(down.error.unwrap().code, ErrorCode::Connection);

        // Invalid input is still an error.
        let invalid = SiteInput {
            port: 0,
            ..sftp_input()
        };
        assert_eq!(
            env.manager
                .test_connection(env.db(), invalid, None, None)
                .await
                .unwrap_err()
                .code(),
            ErrorCode::Validation
        );
    }
}
