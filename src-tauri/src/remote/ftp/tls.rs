//! FTPS certificate trust (ADR 0003).
//!
//! The server certificate is first verified like a browser would, against the Mozilla roots
//! (`webpki-roots`). When that fails (self-signed, private CA, wrong name, expired), Gem falls
//! back to trust on first use: the SHA-256 fingerprint of the leaf certificate's DER is looked
//! up in the [`HostVerifier`] with key type `x509`. A pinned fingerprint is accepted; anything
//! else stops the handshake and surfaces as `CertificateUntrusted` with the certificate to show.
//! Handshake signatures are always verified, pinned or not.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use suppaftp::tokio::{AsyncRustlsStream, AsyncTlsConnector};
use suppaftp::tokio_rustls::TlsConnector;
use suppaftp::tokio_rustls::rustls::client::WebPkiServerVerifier;
use suppaftp::tokio_rustls::rustls::client::danger::{
    HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier,
};
use suppaftp::tokio_rustls::rustls::crypto::{self, CryptoProvider};
use suppaftp::tokio_rustls::rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use suppaftp::tokio_rustls::rustls::{
    self, ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme,
};
use suppaftp::{FtpError, FtpResult};
use tokio::net::TcpStream;

use crate::error::{AppError, AppResult};
use crate::remote::sftp::common::fingerprint_sha256;
use crate::remote::{Auth, ConnectParams, HostDecision, HostVerifier, Logger, host_key_info};

/// Key type reported to the [`HostVerifier`] for TLS certificates.
pub const CERT_KEY_TYPE: &str = "x509";

/// Where the verifier leaves the reason it refused a certificate, for the connect error.
pub type TrustSlot = Arc<Mutex<Option<AppError>>>;

struct GemVerifier {
    webpki: Arc<WebPkiServerVerifier>,
    provider: Arc<CryptoProvider>,
    verifier: Arc<dyn HostVerifier>,
    /// Connection parameters without the secret, for `HostKeyInfo`.
    params: ConnectParams,
    logger: Logger,
    refused: TrustSlot,
    /// Log the trust decision once (data connections repeat the handshake).
    announced: AtomicBool,
}

impl std::fmt::Debug for GemVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GemVerifier")
    }
}

impl GemVerifier {
    fn announce(&self, text: impl FnOnce() -> String) {
        if !self.announced.swap(true, Ordering::AcqRel) {
            self.logger.status(text());
        }
    }

    fn refuse(&self, error: AppError) {
        if let Ok(mut slot) = self.refused.lock() {
            *slot = Some(error);
        }
    }
}

impl ServerCertVerifier for GemVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fingerprint = fingerprint_sha256(end_entity.as_ref());
        let public = self.webpki.verify_server_cert(
            end_entity,
            intermediates,
            server_name,
            ocsp_response,
            now,
        );
        let reason = match public {
            Ok(verified) => {
                self.announce(|| format!("Certificate {fingerprint} verified by a trusted CA"));
                return Ok(verified);
            }
            Err(error) => error,
        };
        match self.verifier.check(
            &self.params.host,
            self.params.port,
            CERT_KEY_TYPE,
            &fingerprint,
        ) {
            HostDecision::Trusted => {
                self.announce(|| {
                    format!("Certificate {fingerprint} is not CA-signed ({reason}) but trusted")
                });
                Ok(ServerCertVerified::assertion())
            }
            HostDecision::Unknown => {
                self.logger.error(format!(
                    "Certificate {fingerprint} is not trusted ({reason}); connection stopped"
                ));
                self.refuse(AppError::CertificateUntrusted(host_key_info(
                    &self.params,
                    CERT_KEY_TYPE,
                    &fingerprint,
                    None,
                )));
                Err(reason)
            }
            HostDecision::Changed {
                previous_fingerprint,
            } => {
                self.logger.error(format!(
                    "Certificate changed: the server now presents {fingerprint}, the trusted \
                     one is {previous_fingerprint}; connection refused"
                ));
                self.refuse(AppError::CertificateUntrusted(host_key_info(
                    &self.params,
                    CERT_KEY_TYPE,
                    &fingerprint,
                    Some(previous_fingerprint),
                )));
                Err(reason)
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// Builds the rustls client configuration for one control connection and its data connections.
/// Session resumption stays on (the rustls default): servers such as vsftpd and ProFTPD require
/// data connections to resume the control connection's session. `tls13` allows TLS 1.3; by
/// default only TLS 1.2 is offered, whose resumption reuses the control session every time.
pub fn client_config(
    params: &ConnectParams,
    verifier: Arc<dyn HostVerifier>,
    logger: &Logger,
    refused: TrustSlot,
    tls13: bool,
) -> AppResult<Arc<ClientConfig>> {
    let provider = Arc::new(crypto::ring::default_provider());
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let webpki =
        WebPkiServerVerifier::builder_with_provider(Arc::new(roots), Arc::clone(&provider))
            .build()
            .map_err(|error| AppError::Internal(format!("TLS setup failed: {error}")))?;
    let verifier = GemVerifier {
        webpki,
        provider: Arc::clone(&provider),
        verifier,
        params: ConnectParams {
            auth: Auth::Agent,
            ..params.clone()
        },
        logger: logger.clone(),
        refused,
        announced: AtomicBool::new(false),
    };
    let versions: &[&rustls::SupportedProtocolVersion] = if tls13 {
        &[&rustls::version::TLS13, &rustls::version::TLS12]
    } else {
        &[&rustls::version::TLS12]
    };
    let config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(versions)
        .map_err(|error| AppError::Internal(format!("TLS setup failed: {error}")))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    Ok(Arc::new(config))
}

/// suppaftp TLS connector that logs the negotiated protocol of the control connection.
/// Every handshake (control and data connections) is bounded by `timeout`: suppaftp sets none,
/// and a server that never answers would otherwise hold the session until its own data
/// timeout (300 s on vsftpd).
pub struct Connector {
    inner: TlsConnector,
    logger: Logger,
    announce: AtomicBool,
    timeout: Duration,
}

impl Connector {
    pub fn new(config: Arc<ClientConfig>, logger: Logger, timeout: Duration) -> Self {
        Self {
            inner: TlsConnector::from(config),
            logger,
            announce: AtomicBool::new(true),
            timeout,
        }
    }
}

impl std::fmt::Debug for Connector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Connector")
    }
}

#[async_trait]
impl AsyncTlsConnector for Connector {
    type Stream = AsyncRustlsStream;

    async fn connect(&self, domain: &str, stream: TcpStream) -> FtpResult<Self::Stream> {
        let name = ServerName::try_from(domain.to_owned())
            .map_err(|error| FtpError::SecureError(error.to_string()))?;
        let tls = tokio::time::timeout(self.timeout, self.inner.connect(name, stream))
            .await
            .map_err(|_| {
                FtpError::SecureError(format!(
                    "TLS handshake timed out after {} s",
                    self.timeout.as_secs()
                ))
            })?
            .map_err(|error| FtpError::SecureError(error.to_string()))?;
        if self.announce.swap(false, Ordering::AcqRel) {
            let (_, connection) = tls.get_ref();
            let version = connection
                .protocol_version()
                .and_then(|version| version.as_str())
                .unwrap_or("TLS");
            let suite = connection
                .negotiated_cipher_suite()
                .and_then(|suite| suite.suite().as_str())
                .unwrap_or("unknown cipher");
            self.logger
                .status(format!("TLS established ({version}, {suite})"));
        }
        Ok(AsyncRustlsStream::from(tls))
    }
}
