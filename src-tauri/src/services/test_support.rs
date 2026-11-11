//! Helpers for service tests: an in-memory database, secret store and server, a connector
//! that authenticates against them, and an event recorder.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use super::secrets::{MemoryStore, SecretStore};
use super::sessions::{Connector, EventSink, SessionManager};
use crate::domain::session::SessionEvent;
use crate::domain::site::{AuthMethod, FtpsMode, Protocol, Site, SiteInput};
use crate::error::{AppError, AppResult};
use crate::remote::memory::MemoryServer;
use crate::remote::{Auth, ConnectParams, Connection, HostDecision, HostVerifier, Logger};
use crate::storage::Database;

/// The password the test server accepts.
pub const PASSWORD: &str = "hunter2-secret";

pub fn sftp_input() -> SiteInput {
    SiteInput {
        name: "prod".into(),
        protocol: Protocol::Sftp,
        host: "example.test".into(),
        port: 22,
        user: "deploy".into(),
        auth: AuthMethod::Password,
        key_path: None,
        ftps_mode: FtpsMode::Explicit,
        passive: true,
        initial_path: None,
        color: None,
    }
}

struct ConnectorState {
    password: String,
    passphrase: Option<String>,
    check_host_keys: bool,
    fingerprint: String,
    secrets_used: Vec<Option<String>>,
}

/// Connects to a [`MemoryServer`], checking the password (or key passphrase) and, when asked
/// to, the host key against the verifier, like the real protocols do.
pub struct MemoryConnector {
    server: Arc<MemoryServer>,
    state: Mutex<ConnectorState>,
}

impl MemoryConnector {
    pub fn new(server: Arc<MemoryServer>) -> Self {
        Self {
            server,
            state: Mutex::new(ConnectorState {
                password: PASSWORD.into(),
                passphrase: None,
                check_host_keys: false,
                fingerprint: "SHA256:memory-host-key".into(),
                secrets_used: Vec::new(),
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ConnectorState> {
        self.state.lock().unwrap()
    }

    /// Key files need this passphrase (`SecretRequired` without one).
    pub fn require_passphrase(&self, passphrase: &str) {
        self.lock().passphrase = Some(passphrase.into());
    }

    /// Changes the password the server accepts (someone changed it on the server).
    pub fn set_password(&self, password: &str) {
        self.lock().password = password.into();
    }

    pub fn check_host_keys(&self) {
        self.lock().check_host_keys = true;
    }

    pub fn set_fingerprint(&self, fingerprint: &str) {
        self.lock().fingerprint = fingerprint.into();
    }

    /// The password or passphrase of every attempt, in order.
    pub fn secrets_used(&self) -> Vec<Option<String>> {
        self.lock().secrets_used.clone()
    }
}

#[async_trait]
impl Connector for MemoryConnector {
    async fn connect(
        &self,
        params: &ConnectParams,
        verifier: Arc<dyn HostVerifier>,
        logger: Logger,
    ) -> AppResult<Arc<dyn Connection>> {
        logger.status(format!("Connecting to {}:{}", params.host, params.port));
        if !self.server.is_up() {
            return Err(AppError::Connection("connection refused".into()));
        }
        let (check, fingerprint, passphrase, accepted) = {
            let state = self.lock();
            (
                state.check_host_keys,
                state.fingerprint.clone(),
                state.passphrase.clone(),
                state.password.clone(),
            )
        };
        if check {
            match verifier.check(&params.host, params.port, "ssh-ed25519", &fingerprint) {
                HostDecision::Trusted => {}
                HostDecision::Unknown => {
                    return Err(AppError::HostKeyUnknown(crate::remote::host_key_info(
                        params,
                        "ssh-ed25519",
                        &fingerprint,
                        None,
                    )));
                }
                HostDecision::Changed {
                    previous_fingerprint,
                } => {
                    return Err(AppError::HostKeyChanged(crate::remote::host_key_info(
                        params,
                        "ssh-ed25519",
                        &fingerprint,
                        Some(previous_fingerprint),
                    )));
                }
            }
        }
        match &params.auth {
            Auth::Password(password) => {
                self.lock().secrets_used.push(Some(password.clone()));
                logger.command(format!("USER {}", params.user));
                logger.command(format!("PASS {password}"));
                if *password != accepted {
                    logger.response("530 Login incorrect");
                    return Err(AppError::AuthFailed("Login incorrect".into()));
                }
            }
            Auth::Key {
                passphrase: given, ..
            } => {
                self.lock().secrets_used.push(given.clone());
                if let Some(expected) = passphrase {
                    match given {
                        None => return Err(AppError::SecretRequired),
                        Some(given) if *given != expected => {
                            return Err(AppError::AuthFailed("wrong passphrase".into()));
                        }
                        Some(_) => {}
                    }
                }
            }
            Auth::Agent => self.lock().secrets_used.push(None),
        }
        logger.response("230 Logged in");
        Ok(self.server.connect() as Arc<dyn Connection>)
    }
}

/// Everything a session test needs.
pub struct Env {
    db: Arc<Database>,
    pub store: Arc<MemoryStore>,
    pub server: Arc<MemoryServer>,
    pub connector: Arc<MemoryConnector>,
    pub manager: SessionManager,
}

impl Env {
    pub fn new() -> Self {
        Self::build(None)
    }

    pub fn with_keepalive(period: Duration) -> Self {
        Self::build(Some(period))
    }

    fn build(keepalive: Option<Duration>) -> Self {
        let server = MemoryServer::new();
        let store = Arc::new(MemoryStore::default());
        let connector = Arc::new(MemoryConnector::new(Arc::clone(&server)));
        let mut manager = SessionManager::new(
            Arc::clone(&connector) as Arc<dyn Connector>,
            Arc::clone(&store) as Arc<dyn SecretStore>,
        );
        if let Some(period) = keepalive {
            manager = manager.with_keepalive_period(period);
        }
        Self {
            db: Arc::new(Database::open_in_memory().unwrap()),
            store,
            server,
            connector,
            manager,
        }
    }

    pub fn db(&self) -> Arc<Database> {
        Arc::clone(&self.db)
    }

    /// A saved SFTP password site named "prod", with `secret` in the keyring.
    pub fn site_with_secret(&self, secret: Option<&str>) -> Site {
        super::sites::create(
            &self.db,
            self.store.as_ref(),
            sftp_input(),
            secret.map(str::to_owned),
        )
        .unwrap()
    }

    pub fn site(&self, id: &str) -> Site {
        self.db.read(|c| super::sites::get(c, id)).unwrap()
    }
}

pub type Events = Arc<Mutex<Vec<SessionEvent>>>;

/// An event sink that records everything.
pub fn recorder() -> (Events, EventSink) {
    let events: Events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    (
        events,
        Arc::new(move |event| sink.lock().unwrap().push(event)),
    )
}

pub fn null_sink() -> EventSink {
    Arc::new(|_| {})
}

/// Waits (up to 5 s) until an event matches; returns every event seen so far.
pub async fn wait_for(
    events: &Events,
    matches: impl Fn(&SessionEvent) -> bool,
) -> Vec<SessionEvent> {
    for _ in 0..1000 {
        {
            let seen = events.lock().unwrap();
            if seen.iter().any(&matches) {
                return seen.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("event not seen; got {:?}", events.lock().unwrap());
}

/// Waits (up to 5 s) for a condition.
pub async fn eventually(what: &str, condition: impl Fn() -> bool) {
    for _ in 0..1000 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("timed out waiting for {what}");
}

pub fn has_log(event: &SessionEvent, needle: &str) -> bool {
    matches!(event, SessionEvent::Log { lines } if lines.iter().any(|l| l.text.contains(needle)))
}

pub fn log_text(events: &[SessionEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::Log { lines } => Some(
                lines
                    .iter()
                    .map(|l| l.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}
