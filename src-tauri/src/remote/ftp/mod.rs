//! FTP and FTPS (`suppaftp` on tokio with rustls/`ring`).
//!
//! - **FTP**: plain control and data connections (credentials travel in clear text).
//! - **FTPS explicit**: `AUTH TLS` on the control connection, then `PBSZ 0` + `PROT P` so data
//!   connections are encrypted too.
//! - **FTPS implicit**: TLS from the first byte (port 990), through suppaftp's
//!   `connect_secure_implicit` (ADR 0006), then `PBSZ 0` + `PROT P`.
//!
//! Certificates are verified against the Mozilla roots, else trusted by fingerprint through the
//! [`HostVerifier`] (see [`tls`]). After login Gem switches to binary mode (`TYPE I`), reads
//! `FEAT` (MLSD/MLST support) and `SYST`, and remembers `PWD` as the home directory. Passive
//! mode connects data channels to the control connection's address whatever the `PASV` reply
//! says (servers behind NAT announce private addresses); `EPSV` is used over IPv6.
//!
//! A control connection is sequential, so [`Connection::open_secondary`] opens a whole new,
//! authenticated control connection for each transfer.

mod fs;
mod listing;
mod reply;
mod tls;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use suppaftp::tokio::AsyncRustlsFtpStream;
use suppaftp::{FtpError, Mode, Status};

use self::fs::{Features, FtpFs};
use self::reply::Op;
use self::tls::{Connector, TrustSlot};
use super::sftp::common::{connect_timeout, tcp_connect};
use super::{Auth, ConnectParams, Connection, HostVerifier, Logger, RemoteFs};
use crate::domain::site::{FtpsMode, Protocol};
use crate::error::{AppError, AppResult};

/// How long one control-connection operation may take.
fn op_timeout(timeout_secs: u64) -> Duration {
    Duration::from_secs(timeout_secs.saturating_mul(2).max(30))
}

/// Opens an FTP or FTPS connection (see the module docs). Errors: `CertificateUntrusted`,
/// `AuthFailed`, `Connection`, `Unsupported` (key or agent authentication).
pub async fn connect(
    params: &ConnectParams,
    verifier: Arc<dyn HostVerifier>,
    logger: Logger,
) -> AppResult<Arc<dyn Connection>> {
    let (primary, welcome) = open(params, &verifier, &logger).await?;
    Ok(Arc::new(FtpConnection {
        params: params.clone(),
        verifier,
        logger,
        welcome,
        primary: Arc::new(primary),
        secondaries: Mutex::new(Vec::new()),
    }))
}

/// One authenticated control connection as an [`FtpFs`], and the welcome line.
async fn open(
    params: &ConnectParams,
    verifier: &Arc<dyn HostVerifier>,
    logger: &Logger,
) -> AppResult<(FtpFs, Option<String>)> {
    let opened = open_control(params, verifier, logger).await?;
    let fs = FtpFs::new(
        opened.ftp,
        opened.features,
        opened.home,
        logger.clone(),
        op_timeout(params.timeout_secs),
        opened.kill_switch,
        Reopen {
            params: params.clone(),
            verifier: Arc::clone(verifier),
        },
    );
    Ok((fs, opened.welcome))
}

/// What reopening a control connection needs (the same site, the same trust store).
pub struct Reopen {
    pub params: ConnectParams,
    pub verifier: Arc<dyn HostVerifier>,
}

/// A freshly authenticated control connection.
pub struct Opened {
    pub ftp: AsyncRustlsFtpStream,
    pub features: Features,
    pub home: String,
    pub welcome: Option<String>,
    pub kill_switch: Option<std::net::TcpStream>,
}

/// Connects, negotiates TLS, logs in and prepares one control connection.
///
/// FTPS uses TLS 1.2 unless the server only speaks TLS 1.3: servers that require data
/// connections to resume the control connection's TLS session (vsftpd, ProFTPD) accept
/// rustls's TLS 1.3 resumption only for the first data connections, because each TLS 1.3
/// ticket is used once and later tickets come from data connections (ADR 0003).
pub async fn open_control(
    params: &ConnectParams,
    verifier: &Arc<dyn HostVerifier>,
    logger: &Logger,
) -> AppResult<Opened> {
    match open_control_with(params, verifier, logger, false).await {
        Err(AppError::Connection(message))
            if params.protocol == Protocol::Ftps && message.contains("ProtocolVersion") =>
        {
            logger.status("The server requires TLS 1.3; connecting again with TLS 1.3");
            open_control_with(params, verifier, logger, true).await
        }
        other => other,
    }
}

async fn open_control_with(
    params: &ConnectParams,
    verifier: &Arc<dyn HostVerifier>,
    logger: &Logger,
    tls13: bool,
) -> AppResult<Opened> {
    let password = match &params.auth {
        Auth::Password(password) => password.clone(),
        Auth::Key { .. } | Auth::Agent => {
            return Err(AppError::Unsupported(
                "FTP and FTPS only support password authentication".into(),
            ));
        }
    };
    let timeout = connect_timeout(params.timeout_secs);
    let secure = params.protocol == Protocol::Ftps;
    let label = match (secure, params.ftps_mode) {
        (false, _) => "FTP",
        (true, FtpsMode::Explicit) => "FTPS, explicit TLS",
        (true, FtpsMode::Implicit) => "FTPS, implicit TLS",
    };
    logger.status(format!(
        "Connecting to {}:{} ({label})…",
        params.host, params.port
    ));
    let refused: TrustSlot = Arc::default();
    let attempt = async {
        let mut ftp = match (secure, params.ftps_mode) {
            (false, _) => {
                let stream = tcp_connect(&params.host, params.port, timeout, logger).await?;
                let ftp = AsyncRustlsFtpStream::connect_with_stream(stream)
                    .await
                    .map_err(|error| connect_error(&error, &refused))?;
                log_welcome(&ftp, logger);
                ftp
            }
            (true, FtpsMode::Explicit) => {
                let stream = tcp_connect(&params.host, params.port, timeout, logger).await?;
                let ftp = AsyncRustlsFtpStream::connect_with_stream(stream)
                    .await
                    .map_err(|error| connect_error(&error, &refused))?;
                log_welcome(&ftp, logger);
                let config = tls::client_config(
                    params,
                    Arc::clone(verifier),
                    logger,
                    Arc::clone(&refused),
                    tls13,
                )?;
                logger.command("AUTH TLS");
                let ftp = ftp
                    .into_secure(Connector::new(config, logger.clone()), &params.host)
                    .await
                    .map_err(|error| match error {
                        FtpError::UnexpectedResponse(response) => {
                            let text = reply::text(&response);
                            logger.error(text.clone());
                            AppError::Connection(format!(
                                "the server does not offer FTPS (AUTH TLS): {text}"
                            ))
                        }
                        other => connect_error(&other, &refused),
                    })?;
                logger.command("PBSZ 0");
                logger.command("PROT P");
                ftp
            }
            (true, FtpsMode::Implicit) => {
                let address = resolve(&params.host, params.port).await?;
                let config = tls::client_config(
                    params,
                    Arc::clone(verifier),
                    logger,
                    Arc::clone(&refused),
                    tls13,
                )?;
                let mut ftp = AsyncRustlsFtpStream::connect_secure_implicit(
                    address,
                    Connector::new(config, logger.clone()),
                    &params.host,
                )
                .await
                .map_err(|error| connect_error(&error, &refused))?;
                log_welcome(&ftp, logger);
                for line in ["PBSZ 0", "PROT P"] {
                    logger.command(line);
                    match ftp.custom_command(line, &[Status::CommandOk]).await {
                        Ok(response) => logger.response(reply::text(&response)),
                        Err(error) => {
                            logger.error(error.to_string());
                            return Err(reply::from_ftp(&error, "", Op::Read));
                        }
                    }
                }
                ftp
            }
        };
        let welcome = ftp.get_welcome_msg().map(welcome_line);
        login(&mut ftp, &params.user, &password, logger).await?;
        let (features, home) = prepare(&mut ftp, logger).await?;
        let ftp = select_mode(ftp, params.passive, logger).await?;
        Ok::<_, AppError>((ftp, features, home, welcome))
    };
    let (ftp, features, home, welcome) = match tokio::time::timeout(timeout, attempt).await {
        Ok(Ok(opened)) => opened,
        Ok(Err(error)) => {
            if !matches!(error, AppError::CertificateUntrusted(_)) {
                logger.error(error.to_string());
            }
            return Err(error);
        }
        Err(_) => {
            let error = AppError::Connection(format!(
                "connecting to {}:{} timed out after {} s",
                params.host,
                params.port,
                timeout.as_secs()
            ));
            logger.error(error.to_string());
            return Err(error);
        }
    };
    let kill_switch = duplicate_socket(&ftp).await;
    Ok(Opened {
        ftp,
        features,
        home,
        welcome,
        kill_switch,
    })
}

/// A second handle on the control socket (same OS socket), used only to shut it down.
async fn duplicate_socket(ftp: &AsyncRustlsFtpStream) -> Option<std::net::TcpStream> {
    let socket = ftp.get_ref().await;
    #[cfg(unix)]
    let duplicate = {
        use std::os::fd::AsFd;
        socket
            .as_fd()
            .try_clone_to_owned()
            .ok()
            .map(std::net::TcpStream::from)
    };
    #[cfg(windows)]
    let duplicate = {
        use std::os::windows::io::AsSocket;
        socket
            .as_socket()
            .try_clone_to_owned()
            .ok()
            .map(std::net::TcpStream::from)
    };
    #[cfg(not(any(unix, windows)))]
    let duplicate = None;
    duplicate
}

async fn resolve(host: &str, port: u16) -> AppResult<SocketAddr> {
    tokio::net::lookup_host((host, port))
        .await
        .map_err(|error| AppError::Connection(format!("could not resolve {host}: {error}")))?
        .next()
        .ok_or_else(|| AppError::Connection(format!("could not resolve {host}")))
}

/// A failure while connecting or negotiating TLS; an untrusted certificate wins.
fn connect_error(error: &FtpError, refused: &TrustSlot) -> AppError {
    if let Some(trust) = refused.lock().ok().and_then(|mut slot| slot.take()) {
        return trust;
    }
    match error {
        FtpError::SecureError(message) => {
            AppError::Connection(format!("TLS handshake failed: {message}"))
        }
        FtpError::ConnectionError(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => {
            AppError::Connection("the server refused the connection".into())
        }
        FtpError::ConnectionError(io) => {
            AppError::Connection(format!("the connection failed: {io}"))
        }
        FtpError::UnexpectedResponse(response) => AppError::Connection(format!(
            "the server is not ready: {}",
            reply::text(response)
        )),
        other => AppError::Connection(other.to_string()),
    }
}

fn log_welcome(ftp: &AsyncRustlsFtpStream, logger: &Logger) {
    if let Some(welcome) = ftp.get_welcome_msg() {
        for line in welcome.lines().filter(|line| !line.trim().is_empty()) {
            logger.response(line.trim().to_owned());
        }
    }
}

/// The first welcome line without its `220` code, for the status bar.
fn welcome_line(welcome: &str) -> String {
    let first = welcome.lines().next().unwrap_or_default().trim();
    first
        .strip_prefix("220-")
        .or_else(|| first.strip_prefix("220 "))
        .unwrap_or(first)
        .trim()
        .to_owned()
}

/// `USER`/`PASS`. An empty user logs in as `anonymous`. The password is never logged.
async fn login(
    ftp: &mut AsyncRustlsFtpStream,
    user: &str,
    password: &str,
    logger: &Logger,
) -> AppResult<()> {
    let (user, password) = if user.is_empty() {
        (
            "anonymous",
            if password.is_empty() {
                "anonymous@"
            } else {
                password
            },
        )
    } else {
        (user, password)
    };
    let rejected = |response: &suppaftp::types::Response| {
        let text = reply::text(response);
        logger.error(text.clone());
        match reply::code(response) {
            530 => {
                AppError::AuthFailed(format!("the server rejected the login for {user}: {text}"))
            }
            _ => AppError::AuthFailed(text),
        }
    };
    logger.command(format!("USER {user}"));
    let response = match ftp
        .custom_command(
            format!("USER {user}"),
            &[Status::LoggedIn, Status::NeedPassword],
        )
        .await
    {
        Ok(response) => response,
        Err(FtpError::UnexpectedResponse(response)) => return Err(rejected(&response)),
        Err(error) => return Err(reply::from_ftp(&error, "", Op::Read)),
    };
    logger.response(reply::text(&response));
    if response.status == Status::NeedPassword {
        // Logged as a fixed string: the secret never reaches the logger.
        logger.command("PASS ****");
        match ftp
            .custom_command(format!("PASS {password}"), &[Status::LoggedIn])
            .await
        {
            Ok(response) => logger.response(reply::text(&response)),
            Err(FtpError::UnexpectedResponse(response)) => return Err(rejected(&response)),
            Err(error) => return Err(reply::from_ftp(&error, "", Op::Read)),
        }
    }
    logger.status(format!("Logged in as {user}"));
    Ok(())
}

/// `TYPE I`, `FEAT`, `OPTS UTF8 ON`, `SYST` and `PWD`.
async fn prepare(ftp: &mut AsyncRustlsFtpStream, logger: &Logger) -> AppResult<(Features, String)> {
    async fn command(
        ftp: &mut AsyncRustlsFtpStream,
        logger: &Logger,
        line: &str,
        expect: &[Status],
    ) -> Result<suppaftp::types::Response, FtpError> {
        logger.command(line);
        let result = ftp.custom_command(line, expect).await;
        match &result {
            Ok(response) => logger.response(reply::text(response)),
            Err(FtpError::UnexpectedResponse(response)) => logger.error(reply::text(response)),
            Err(error) => logger.error(error.to_string()),
        }
        result
    }
    // Binary mode: bytes are transferred unchanged (ASCII mode rewrites line endings).
    command(ftp, logger, "TYPE I", &[Status::CommandOk])
        .await
        .map_err(|error| reply::from_ftp(&error, "", Op::Read))?;
    logger.command("FEAT");
    let features = match ftp.feat().await {
        Ok(features) => {
            let mut names: Vec<&str> = features.keys().map(String::as_str).collect();
            names.sort_unstable();
            logger.response(format!("211 features: {}", names.join(", ")));
            features
        }
        Err(error) => {
            match &error {
                FtpError::UnexpectedResponse(response) => logger.error(reply::text(response)),
                other => logger.error(other.to_string()),
            }
            if reply::is_fatal(&error) {
                return Err(reply::from_ftp(&error, "", Op::Read));
            }
            Default::default()
        }
    };
    let feature = |name: &str| {
        features
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone().unwrap_or_default())
    };
    if feature("UTF8").is_some() {
        let _ = command(ftp, logger, "OPTS UTF8 ON", &[Status::CommandOk]).await;
    }
    let mlst_facts = feature("MLST");
    let unix = match command(ftp, logger, "SYST", &[Status::Name]).await {
        Ok(response) => reply::text(&response).to_ascii_uppercase().contains("UNIX"),
        Err(error) if reply::is_fatal(&error) => {
            return Err(reply::from_ftp(&error, "", Op::Read));
        }
        // Unknown: most servers are Unix-like.
        Err(_) => true,
    };
    let owner = match &mlst_facts {
        Some(facts) => facts.to_ascii_lowercase().contains("unix.owner"),
        None => unix,
    };
    logger.command("PWD");
    let home = ftp.pwd().await.map_err(|error| {
        logger.error(error.to_string());
        reply::from_ftp(&error, "", Op::Read)
    })?;
    logger.response(format!("257 \"{home}\""));
    Ok((
        Features {
            mlst: mlst_facts.is_some(),
            owner,
            unix,
            rang: feature("RANG").is_some(),
        },
        home,
    ))
}

/// Passive mode (EPSV over IPv6) with the NAT workaround, or active mode.
async fn select_mode(
    mut ftp: AsyncRustlsFtpStream,
    passive: bool,
    logger: &Logger,
) -> AppResult<AsyncRustlsFtpStream> {
    if !passive {
        logger.status("Active mode: the server connects back for data transfers");
        return Ok(ftp.active_mode(Duration::from_secs(30)));
    }
    let ipv6 = ftp
        .get_ref()
        .await
        .peer_addr()
        .map(|address| address.is_ipv6())
        .unwrap_or(false);
    if ipv6 {
        ftp.set_mode(Mode::ExtendedPassive);
    } else {
        ftp.set_mode(Mode::Passive);
        // Use the control connection's address for data, whatever PASV announces.
        ftp.set_passive_nat_workaround(true);
    }
    Ok(ftp)
}

/// A live FTP connection: the primary control connection plus one per transfer.
pub struct FtpConnection {
    params: ConnectParams,
    verifier: Arc<dyn HostVerifier>,
    logger: Logger,
    welcome: Option<String>,
    primary: Arc<FtpFs>,
    secondaries: Mutex<Vec<Weak<FtpFs>>>,
}

#[async_trait]
impl Connection for FtpConnection {
    fn fs(&self) -> Arc<dyn RemoteFs> {
        self.primary.clone()
    }

    async fn open_secondary(&self) -> AppResult<Arc<dyn RemoteFs>> {
        if !self.primary.is_alive() {
            return Err(AppError::Disconnected);
        }
        self.logger
            .status("Opening another control connection for a transfer");
        let (fs, _) = open(&self.params, &self.verifier, &self.logger).await?;
        let fs = Arc::new(fs);
        if let Ok(mut secondaries) = self.secondaries.lock() {
            secondaries.retain(|weak| weak.strong_count() > 0);
            secondaries.push(Arc::downgrade(&fs));
        }
        Ok(fs)
    }

    fn server_info(&self) -> Option<String> {
        self.welcome.clone()
    }

    fn is_alive(&self) -> bool {
        self.primary.is_alive()
    }

    async fn close(&self) {
        let secondaries: Vec<Arc<FtpFs>> = self
            .secondaries
            .lock()
            .map(|list| list.iter().filter_map(Weak::upgrade).collect())
            .unwrap_or_default();
        for fs in secondaries {
            fs.shutdown().await;
        }
        self.primary.shutdown().await;
        self.logger.status("Disconnected");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn welcome_lines_lose_their_code() {
        assert_eq!(
            welcome_line("220 ProFTPD Server (Gem test ProFTPD) [127.0.0.1]"),
            "ProFTPD Server (Gem test ProFTPD) [127.0.0.1]"
        );
        assert_eq!(
            welcome_line("220-Welcome to Pure-FTPd.\n220 Bye"),
            "Welcome to Pure-FTPd."
        );
    }
}
