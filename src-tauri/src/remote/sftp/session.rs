//! The SSH side of an SFTP connection: transport, host key verification, authentication and
//! the SFTP subsystem channels.
//!
//! Host keys are checked inside russh's `check_server_key` callback. The callback cannot return
//! an `AppError` through russh's `bool`, so the handler's own error type carries the trust
//! decision (`HostKeyUnknown`/`HostKeyChanged` with the key to show) out of the handshake;
//! russh aborts the connection before any authentication happens.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, AuthResult, Handle};
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::{AgentClient, AgentStream};
use russh::keys::{HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh_sftp::client::RawSftpSession;
use russh_sftp::client::rawsession::Limits;

use super::common::{fingerprint_sha256, tcp_connect};
use super::error::describe_ssh;
use crate::error::{AppError, AppResult};
use crate::remote::{Auth, ConnectParams, HostDecision, HostVerifier, Logger, host_key_info};

/// OpenSSH extensions Gem uses when the server announces them in its SFTP `VERSION` packet.
#[derive(Debug, Clone, Copy, Default)]
pub struct Extensions {
    /// `posix-rename@openssh.com`: rename over an existing file in one step.
    pub posix_rename: bool,
    /// `fsync@openssh.com`: flush a written file before it is renamed into place.
    pub fsync: bool,
    /// Largest read and write request (from `limits@openssh.com`, capped at [`MAX_REQUEST`]).
    pub read_len: u32,
    pub write_len: u32,
}

/// Size of one pipelined SFTP read or write request. OpenSSH accepts up to ~255 KiB; 64 KiB
/// requests with 16 in flight keep 1 MiB on the wire, enough to fill a fast link.
pub const MAX_REQUEST: u32 = 64 * 1024;

/// Where `Auth::Agent` finds the agent. Production uses the environment; tests pass the socket
/// of an agent they started (the environment is process-wide and tests run in parallel).
#[derive(Debug, Clone, Default)]
pub enum AgentSource {
    /// `SSH_AUTH_SOCK` on Unix; the OpenSSH named pipe, then Pageant, on Windows.
    #[default]
    Environment,
    #[cfg_attr(not(test), allow(dead_code))]
    Socket(PathBuf),
}

/// Failure inside the russh handler: a transport error, or a refused host key.
#[derive(Debug)]
pub enum HandlerError {
    Ssh(russh::Error),
    Trust(AppError),
}

impl From<russh::Error> for HandlerError {
    fn from(error: russh::Error) -> Self {
        Self::Ssh(error)
    }
}

/// russh client callbacks: host key verification and session details for the log.
pub struct ClientHandler {
    /// Connection parameters without the secret, for `HostKeyInfo`.
    params: ConnectParams,
    verifier: Arc<dyn HostVerifier>,
    logger: Logger,
    server_id: Arc<Mutex<Option<String>>>,
}

impl client::Handler for ClientHandler {
    type Error = HandlerError;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let key = match server_public_key {
            PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
            // A host certificate is trusted through the key it certifies.
            PublicKeyOrCertificate::Certificate(certificate) => {
                PublicKey::from(certificate.public_key().clone())
            }
        };
        let key_type = key.algorithm().as_str().to_owned();
        let blob = key
            .to_bytes()
            .map_err(|error| HandlerError::Trust(AppError::Internal(error.to_string())))?;
        let fingerprint = fingerprint_sha256(&blob);
        let decision =
            self.verifier
                .check(&self.params.host, self.params.port, &key_type, &fingerprint);
        match decision {
            HostDecision::Trusted => {
                self.logger
                    .status(format!("Host key {key_type} {fingerprint} is trusted"));
                Ok(true)
            }
            HostDecision::Unknown => {
                self.logger.error(format!(
                    "Host key {key_type} {fingerprint} is not trusted yet; connection stopped"
                ));
                Err(HandlerError::Trust(AppError::HostKeyUnknown(
                    host_key_info(&self.params, &key_type, &fingerprint, None),
                )))
            }
            HostDecision::Changed {
                previous_fingerprint,
            } => {
                self.logger.error(format!(
                    "Host key changed: the server now presents {key_type} {fingerprint}, \
                     the stored key is {previous_fingerprint}; connection refused"
                ));
                Err(HandlerError::Trust(AppError::HostKeyChanged(
                    host_key_info(
                        &self.params,
                        &key_type,
                        &fingerprint,
                        Some(previous_fingerprint),
                    ),
                )))
            }
        }
    }

    async fn kex_done(
        &mut self,
        _shared_secret: Option<&[u8]>,
        names: &russh::Names,
        session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        let id = String::from_utf8_lossy(session.remote_sshid())
            .trim()
            .to_owned();
        if let Ok(mut slot) = self.server_id.lock()
            && slot.is_none()
        {
            self.logger.status(format!("Server: {id}"));
            *slot = Some(id);
        }
        self.logger.status(format!(
            "Key exchange {}, cipher {}",
            names.kex.as_ref(),
            names.cipher.as_ref()
        ));
        Ok(())
    }

    async fn auth_banner(
        &mut self,
        banner: &str,
        _session: &mut client::Session,
    ) -> Result<(), Self::Error> {
        for line in banner.lines().filter(|line| !line.trim().is_empty()) {
            self.logger.response(line.to_owned());
        }
        Ok(())
    }
}

/// An SSH connection that passed host key verification, with the server's identification.
pub struct Transport {
    pub handle: Handle<ClientHandler>,
    pub server_id: Option<String>,
}

/// TCP connect, SSH handshake and host key check, within the connect timeout.
pub async fn open_transport(
    params: &ConnectParams,
    verifier: Arc<dyn HostVerifier>,
    logger: &Logger,
    timeout: Duration,
) -> AppResult<Transport> {
    let stream = tcp_connect(&params.host, params.port, timeout, logger).await?;
    let config = Arc::new(client::Config {
        // Gem sends its own keepalives (`RemoteFs::keepalive`); no hidden timers here.
        inactivity_timeout: None,
        keepalive_interval: None,
        nodelay: true,
        ..client::Config::default()
    });
    let server_id = Arc::new(Mutex::new(None));
    let handler = ClientHandler {
        params: ConnectParams {
            auth: Auth::Agent,
            ..params.clone()
        },
        verifier,
        logger: logger.clone(),
        server_id: Arc::clone(&server_id),
    };
    let handshake = client::connect_stream(config, stream, handler);
    let handle = match tokio::time::timeout(timeout, handshake).await {
        Ok(Ok(handle)) => handle,
        Ok(Err(HandlerError::Trust(error))) => return Err(error),
        Ok(Err(HandlerError::Ssh(error))) => {
            let message = describe_ssh(&error);
            logger.error(message.clone());
            return Err(AppError::Connection(message));
        }
        Err(_) => {
            let message = format!("the SSH handshake timed out after {} s", timeout.as_secs());
            logger.error(message.clone());
            return Err(AppError::Connection(message));
        }
    };
    let server_id = server_id.lock().ok().and_then(|slot| slot.clone());
    Ok(Transport { handle, server_id })
}

/// Authenticates with the method in `params.auth`, within `timeout`.
pub async fn authenticate(
    handle: &mut Handle<ClientHandler>,
    params: &ConnectParams,
    agent: &AgentSource,
    logger: &Logger,
    timeout: Duration,
) -> AppResult<()> {
    let user = params.user.as_str();
    let attempt = async {
        match &params.auth {
            Auth::Password(password) => {
                logger.status(format!("Authenticating as {user} with a password"));
                let result = handle
                    .authenticate_password(user, password.as_str())
                    .await
                    .map_err(|error| AppError::Connection(describe_ssh(&error)))?;
                check_result(result, || {
                    format!("the server rejected the password for {user}")
                })
            }
            Auth::Key { path, passphrase } => {
                logger.status(format!("Authenticating as {user} with the key {path}"));
                let key = load_key(path, passphrase.as_deref()).await?;
                let hash = rsa_hash(handle, &key.public_key().clone()).await;
                let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
                let result = handle
                    .authenticate_publickey(user, key)
                    .await
                    .map_err(|error| AppError::Connection(describe_ssh(&error)))?;
                check_result(result, || {
                    format!("the server rejected the key {path} for {user}")
                })
            }
            Auth::Agent => {
                logger.status(format!("Authenticating as {user} with the SSH agent"));
                authenticate_with_agent(handle, user, agent, logger).await
            }
        }
    };
    let result = match tokio::time::timeout(timeout, attempt).await {
        Ok(result) => result,
        Err(_) => Err(AppError::Connection(format!(
            "authentication timed out after {} s",
            timeout.as_secs()
        ))),
    };
    match &result {
        Ok(()) => logger.status(format!("Authenticated as {user}")),
        Err(error) => logger.error(error.to_string()),
    }
    result
}

fn check_result(result: AuthResult, rejected: impl FnOnce() -> String) -> AppResult<()> {
    match result {
        AuthResult::Success => Ok(()),
        AuthResult::Failure {
            partial_success: true,
            ..
        } => Err(AppError::AuthFailed(
            "the server asks for a second authentication step, which Gem does not support".into(),
        )),
        AuthResult::Failure { .. } => Err(AppError::AuthFailed(rejected())),
    }
}

/// Loads a private key file. An encrypted key without a passphrase is `SecretRequired`; a wrong
/// passphrase is `AuthFailed`.
async fn load_key(path: &str, passphrase: Option<&str>) -> AppResult<PrivateKey> {
    let text = tokio::fs::read_to_string(path).await.map_err(|error| {
        AppError::Validation(format!("the key file {path} could not be read: {error}"))
    })?;
    let passphrase = passphrase.filter(|value| !value.is_empty());
    match russh::keys::decode_secret_key(&text, passphrase) {
        Ok(key) => Ok(key),
        Err(russh::keys::Error::KeyIsEncrypted) => Err(AppError::SecretRequired),
        Err(_) if passphrase.is_none() && is_encrypted_ppk(&text) => Err(AppError::SecretRequired),
        Err(error) => {
            // Decoding again without the passphrase tells "wrong passphrase" apart from a
            // corrupt or unsupported key.
            let encrypted = is_encrypted_ppk(&text)
                || matches!(
                    russh::keys::decode_secret_key(&text, None),
                    Err(russh::keys::Error::KeyIsEncrypted)
                );
            if passphrase.is_some() && encrypted {
                Err(AppError::AuthFailed(format!(
                    "wrong passphrase for the key {path}"
                )))
            } else {
                Err(AppError::AuthFailed(format!(
                    "the key {path} could not be loaded: {error}"
                )))
            }
        }
    }
}

/// Whether the text is a PuTTY key file with an encrypted private part. russh reports an
/// encrypted PPK without a passphrase as a parse error rather than `KeyIsEncrypted`, so the
/// header is read here.
fn is_encrypted_ppk(text: &str) -> bool {
    text.trim_start().starts_with("PuTTY-User-Key-File-")
        && text.lines().any(|line| {
            line.strip_prefix("Encryption:")
                .is_some_and(|value| value.trim() != "none")
        })
}

/// The signature hash for RSA keys: the best one the server announces (`server-sig-algs`),
/// SHA-256 when it announces nothing. `None` for other key types.
async fn rsa_hash(handle: &Handle<ClientHandler>, key: &PublicKey) -> Option<HashAlg> {
    if !key.algorithm().is_rsa() {
        return None;
    }
    match handle.best_supported_rsa_hash().await {
        Ok(Some(hash)) => hash,
        Ok(None) | Err(_) => Some(HashAlg::Sha256),
    }
}

type DynAgent = AgentClient<Box<dyn AgentStream + Send + Unpin>>;

async fn authenticate_with_agent(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    source: &AgentSource,
    logger: &Logger,
) -> AppResult<()> {
    let mut agent = connect_agent(source, logger).await?;
    let identities = agent.request_identities().await.map_err(|error| {
        AppError::AuthFailed(format!("the SSH agent did not list its keys: {error}"))
    })?;
    let keys: Vec<(PublicKey, String)> = identities
        .into_iter()
        .filter_map(|identity| match identity {
            AgentIdentity::PublicKey { key, comment } => Some((key, comment)),
            // OpenSSH certificates in the agent are not offered (no certificate support).
            AgentIdentity::Certificate { .. } => None,
        })
        .collect();
    if keys.is_empty() {
        return Err(AppError::AuthFailed("the SSH agent has no keys".into()));
    }
    for (key, comment) in keys {
        let fingerprint = key
            .to_bytes()
            .map(|blob| fingerprint_sha256(&blob))
            .unwrap_or_default();
        logger.status(format!(
            "Offering agent key {} {fingerprint} {comment}",
            key.algorithm().as_str()
        ));
        let hash = rsa_hash(handle, &key).await;
        match handle
            .authenticate_publickey_with(user, key, hash, &mut agent)
            .await
        {
            Ok(result @ AuthResult::Success) => return check_result(result, String::new),
            Ok(
                result @ AuthResult::Failure {
                    partial_success: true,
                    ..
                },
            ) => return check_result(result, String::new),
            Ok(AuthResult::Failure { .. }) => {}
            Err(error) => logger.error(format!("the SSH agent could not sign: {error:?}")),
        }
    }
    Err(AppError::AuthFailed(format!(
        "the server rejected every key in the SSH agent for {user}"
    )))
}

#[cfg(unix)]
async fn connect_agent(source: &AgentSource, _logger: &Logger) -> AppResult<DynAgent> {
    let socket = match source {
        AgentSource::Socket(path) => path.clone(),
        AgentSource::Environment => match std::env::var_os("SSH_AUTH_SOCK") {
            Some(value) if !value.is_empty() => PathBuf::from(value),
            _ => {
                return Err(AppError::AuthFailed(
                    "no SSH agent is running (SSH_AUTH_SOCK is not set)".into(),
                ));
            }
        },
    };
    AgentClient::connect_uds(&socket)
        .await
        .map(AgentClient::dynamic)
        .map_err(|error| {
            AppError::AuthFailed(format!(
                "could not reach the SSH agent at {}: {error}",
                socket.display()
            ))
        })
}

/// Windows: the OpenSSH agent's named pipe (or `SSH_AUTH_SOCK` when it names a pipe), then
/// Pageant. Not exercised by the Linux test suite; see ADR 0007.
#[cfg(windows)]
async fn connect_agent(source: &AgentSource, logger: &Logger) -> AppResult<DynAgent> {
    const OPENSSH_PIPE: &str = r"\\.\pipe\openssh-ssh-agent";
    let pipe: std::ffi::OsString = match source {
        AgentSource::Socket(path) => path.clone().into_os_string(),
        AgentSource::Environment => std::env::var_os("SSH_AUTH_SOCK")
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| OPENSSH_PIPE.into()),
    };
    match AgentClient::connect_named_pipe(&pipe).await {
        Ok(agent) => return Ok(agent.dynamic()),
        Err(error) => logger.status(format!(
            "OpenSSH agent pipe {} not available ({error}); trying Pageant",
            pipe.to_string_lossy()
        )),
    }
    AgentClient::connect_pageant()
        .await
        .map(AgentClient::dynamic)
        .map_err(|error| {
            AppError::AuthFailed(format!(
                "no SSH agent found: neither the OpenSSH agent nor Pageant answered ({error})"
            ))
        })
}

#[cfg(not(any(unix, windows)))]
async fn connect_agent(_source: &AgentSource, _logger: &Logger) -> AppResult<DynAgent> {
    Err(AppError::Unsupported(
        "SSH agent authentication is not supported on this platform".into(),
    ))
}

/// Opens a session channel with the SFTP subsystem and initialises the protocol.
pub async fn open_sftp(
    handle: &Handle<ClientHandler>,
    request_timeout: Duration,
) -> AppResult<(RawSftpSession, Extensions)> {
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|error| AppError::Connection(describe_ssh(&error)))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|error| AppError::Connection(describe_ssh(&error)))?;
    let config = russh_sftp::client::Config {
        request_timeout_secs: request_timeout.as_secs().max(1),
        ..russh_sftp::client::Config::default()
    };
    let mut sftp = RawSftpSession::new_with_config(channel.into_stream(), config);
    let version = match tokio::time::timeout(request_timeout, sftp.init()).await {
        Ok(Ok(version)) => version,
        Ok(Err(error)) => {
            return Err(AppError::Connection(format!(
                "the server did not start SFTP: {error}"
            )));
        }
        Err(_) => {
            return Err(AppError::Connection(
                "the server did not start SFTP in time".into(),
            ));
        }
    };
    let has = |name: &str| {
        version
            .extensions
            .get(name)
            .is_some_and(|value| value == "1")
    };
    let mut extensions = Extensions {
        posix_rename: has("posix-rename@openssh.com"),
        fsync: has("fsync@openssh.com"),
        read_len: MAX_REQUEST,
        write_len: MAX_REQUEST,
    };
    if has("limits@openssh.com")
        && let Ok(limits) = sftp.limits().await
    {
        let cap = |value: u64| {
            if value == 0 {
                MAX_REQUEST
            } else {
                u32::try_from(value).unwrap_or(MAX_REQUEST).min(MAX_REQUEST)
            }
        };
        extensions.read_len = cap(limits.max_read_len);
        extensions.write_len = cap(limits.max_write_len);
        sftp.set_limits(Limits::from(limits));
    }
    Ok((sftp, extensions))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PuTTY test keys from the `ssh-key` crate; the encrypted one's passphrase is `123`.
    const PPK: &str = "PuTTY-User-Key-File-3: ssh-ed25519
Encryption: none
Comment: user@example.com
Public-Lines: 2
AAAAC3NzaC1lZDI1NTE5AAAAILM+rvN+ot98qgEN796jTiQfZfG1KaT0PtFDJ/XF
Sqti
Private-Lines: 1
AAAAILYGwiLRDBba4WxwpNRRc0cuxhfgXGVpINJuVsCPtZHt
Private-MAC: 94140d0344fad6aa1bf7b71e9c93db11ccac8a232f8a51e11c024869d608c82d
";
    const PPK_ENCRYPTED: &str = "PuTTY-User-Key-File-3: ssh-ed25519
Encryption: aes256-cbc
Comment: user@example.com
Public-Lines: 2
AAAAC3NzaC1lZDI1NTE5AAAAILM+rvN+ot98qgEN796jTiQfZfG1KaT0PtFDJ/XF
Sqti
Key-Derivation: Argon2id
Argon2-Memory: 8192
Argon2-Passes: 34
Argon2-Parallelism: 1
Argon2-Salt: 63d1d43f7bf7700720496646a2f5ec17
Private-Lines: 1
DyWtExZ3dxFutnb12tIwXBC6kWdozrvP+r6faHKBGDb4+qEar9XBiC0BmGySMHUi
Private-MAC: 52fd00d4ef47ebc506e4e709486c0c6bc0606e24fe2c6cb1b3d168f4da238a66
";

    async fn load(contents: &str, passphrase: Option<&str>) -> AppResult<PrivateKey> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("key.ppk");
        std::fs::write(&path, contents).unwrap();
        load_key(path.to_str().unwrap(), passphrase).await
    }

    #[tokio::test]
    async fn loads_a_putty_key() {
        let key = load(PPK, None).await.unwrap();
        assert!(matches!(key.algorithm(), russh::keys::Algorithm::Ed25519));
    }

    #[tokio::test]
    async fn asks_for_the_passphrase_of_an_encrypted_putty_key() {
        assert!(matches!(
            load(PPK_ENCRYPTED, None).await,
            Err(AppError::SecretRequired)
        ));
        assert!(matches!(
            load(PPK_ENCRYPTED, Some("")).await,
            Err(AppError::SecretRequired)
        ));
        let windows = PPK_ENCRYPTED.replace('\n', "\r\n");
        assert!(matches!(
            load(&windows, None).await,
            Err(AppError::SecretRequired)
        ));
    }

    #[tokio::test]
    async fn opens_an_encrypted_putty_key_with_its_passphrase() {
        assert!(load(PPK_ENCRYPTED, Some("123")).await.is_ok());
        match load(PPK_ENCRYPTED, Some("wrong")).await {
            Err(AppError::AuthFailed(message)) => assert!(message.contains("wrong passphrase")),
            other => panic!("expected a wrong passphrase error, got {other:?}"),
        }
    }
}
