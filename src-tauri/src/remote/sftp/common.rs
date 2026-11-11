//! Helpers shared by the SFTP and FTP backends: identity fingerprints, the names of the temp
//! and backup siblings used by atomic writes, and the transfer chunk size.
//!
//! They live here (and the FTP backend imports them) because the protocol modules own no file
//! outside `remote/sftp` and `remote/ftp`.

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use sha2::{Digest, Sha256};
use tokio::net::TcpStream;

use crate::error::{AppError, AppResult};
use crate::remote::{Logger, path};

/// Size of one streamed chunk for downloads and uploads. Memory per transfer stays at a small
/// multiple of this, whatever the file size.
pub const CHUNK: usize = 256 * 1024;

/// Longest file name most servers accept (bytes).
const MAX_NAME: usize = 255;

/// OpenSSH-style fingerprint: `SHA256:` + unpadded base64 of the SHA-256 digest of `bytes`
/// (the SSH public key blob, or a certificate's DER).
pub fn fingerprint_sha256(bytes: &[u8]) -> String {
    format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(bytes)))
}

/// Eight random ASCII alphanumerics from the OS random source.
pub fn random_suffix() -> AppResult<String> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut out = String::with_capacity(8);
    let mut buf = [0u8; 32];
    while out.len() < 8 {
        getrandom::fill(&mut buf)
            .map_err(|error| AppError::Internal(format!("no random source: {error}")))?;
        // Rejection sampling keeps the distribution uniform (248 = 4 * 62).
        for byte in buf {
            if byte < 248 && out.len() < 8 {
                out.push(char::from(ALPHABET[usize::from(byte % 62)]));
            }
        }
    }
    Ok(out)
}

/// The temp sibling an atomic write goes through: `<dir>/.<name>.gem-<suffix>`.
pub fn temp_sibling(target: &str, suffix: &str) -> String {
    sibling(target, &format!(".gem-{suffix}"))
}

/// The backup sibling used when a server cannot rename over an existing file:
/// `<dir>/.<name>.gem-old-<suffix>`.
pub fn backup_sibling(target: &str, suffix: &str) -> String {
    sibling(target, &format!(".gem-old-{suffix}"))
}

/// Whether `name` looks like one of our temp or backup siblings (for tests and cleanup).
#[cfg(test)]
pub fn is_gem_sibling(name: &str) -> bool {
    name.starts_with('.') && name.contains(".gem-")
}

/// Gem's own temp and backup siblings older than this are left over from an interrupted save
/// and are removed the next time the same file is saved (ADR 0005).
pub const STALE_AFTER_SECS: i64 = 3600;

/// Whether `candidate` (a file name in the target's folder) is exactly one of Gem's temp or
/// backup siblings of `target`: `.<name>.gem-XXXXXXXX` or `.<name>.gem-old-XXXXXXXX`, with eight
/// ASCII alphanumerics. Nothing else ever matches.
pub fn is_own_sibling(candidate: &str, target: &str) -> bool {
    const PLACEHOLDER: &str = "00000000";
    [
        temp_sibling(target, PLACEHOLDER),
        backup_sibling(target, PLACEHOLDER),
    ]
    .iter()
    .any(|template| {
        let template = path::file_name(template);
        let prefix_len = template.len() - PLACEHOLDER.len();
        candidate.len() == template.len()
            && candidate
                .as_bytes()
                .starts_with(&template.as_bytes()[..prefix_len])
            && candidate.as_bytes()[prefix_len..]
                .iter()
                .all(u8::is_ascii_alphanumeric)
    })
}

/// Seconds since the Unix epoch, now.
pub fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

/// The connect timeout for `timeout_secs` (at least one second).
pub fn connect_timeout(timeout_secs: u64) -> Duration {
    Duration::from_secs(timeout_secs.max(1))
}

/// Resolves `host` and opens a TCP connection to the first address that answers, within
/// `timeout` overall. Errors are `Connection` with a message for people (DNS failure, refused,
/// timed out).
pub async fn tcp_connect(
    host: &str,
    port: u16,
    timeout: Duration,
    logger: &Logger,
) -> AppResult<TcpStream> {
    let secs = timeout.as_secs();
    let attempt = async {
        let addresses: Vec<_> = tokio::net::lookup_host((host, port))
            .await
            .map_err(|error| AppError::Connection(format!("could not resolve {host}: {error}")))?
            .collect();
        if addresses.is_empty() {
            return Err(AppError::Connection(format!("could not resolve {host}")));
        }
        let mut last = None;
        for address in addresses {
            match TcpStream::connect(address).await {
                Ok(stream) => {
                    // Interactive protocols: send small commands immediately.
                    let _ = stream.set_nodelay(true);
                    logger.status(format!("Connected to {address}"));
                    return Ok(stream);
                }
                Err(error) => last = Some((address, error)),
            }
        }
        Err(match last {
            Some((address, error)) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                AppError::Connection(format!("{address} refused the connection"))
            }
            Some((address, error)) => {
                AppError::Connection(format!("could not connect to {address}: {error}"))
            }
            None => AppError::Connection(format!("could not connect to {host}:{port}")),
        })
    };
    match tokio::time::timeout(timeout, attempt).await {
        Ok(result) => result,
        Err(_) => Err(AppError::Connection(format!(
            "connecting to {host}:{port} timed out after {secs} s"
        ))),
    }
}

fn sibling(target: &str, tail: &str) -> String {
    let dir = path::parent(target);
    let name = path::file_name(target);
    // Keep the result within the usual 255-byte limit by shortening the original name.
    let budget = MAX_NAME.saturating_sub(1 + tail.len());
    let mut cut = name.len().min(budget);
    while !name.is_char_boundary(cut) {
        cut -= 1;
    }
    path::join(&dir, &format!(".{}{tail}", &name[..cut]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprints_like_openssh() {
        // dev/servers/keys/id_ed25519.pub; `ssh-keygen -lf` prints the same fingerprint.
        let blob = base64::engine::general_purpose::STANDARD
            .decode("AAAAC3NzaC1lZDI1NTE5AAAAIERR660SYq9V4J0pXmtEFNOGT2J2I4idapoOiXRzC27F")
            .unwrap();
        assert_eq!(
            fingerprint_sha256(&blob),
            "SHA256:xmc6ycGgWPVsT4DH5xSnqwmkuRNIaLtpBQmNrOCP8hE"
        );
        // Unpadded, like OpenSSH.
        assert_eq!(
            fingerprint_sha256(b""),
            "SHA256:47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU"
        );
    }

    #[test]
    fn random_suffixes_are_alphanumeric() {
        let a = random_suffix().unwrap();
        let b = random_suffix().unwrap();
        assert_eq!(a.len(), 8);
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(a, b);
    }

    #[test]
    fn temp_names_are_hidden_siblings() {
        assert_eq!(
            temp_sibling("/var/www/config.yaml", "Ab3dEf9h"),
            "/var/www/.config.yaml.gem-Ab3dEf9h"
        );
        assert_eq!(
            backup_sibling("/config.yaml", "Ab3dEf9h"),
            "/.config.yaml.gem-old-Ab3dEf9h"
        );
        assert!(is_gem_sibling(".config.yaml.gem-Ab3dEf9h"));
        assert!(!is_gem_sibling("config.yaml"));
    }

    #[test]
    fn recognises_only_own_siblings_of_the_same_file() {
        let target = "/var/www/config.yaml";
        assert!(is_own_sibling(".config.yaml.gem-Ab3dEf9h", target));
        assert!(is_own_sibling(".config.yaml.gem-old-Ab3dEf9h", target));
        for other in [
            "config.yaml",
            ".config.yaml.gem-Ab3dEf9",
            ".config.yaml.gem-Ab3dEf9hX",
            ".config.yaml.gem-Ab3d-f9h",
            ".config.yaml.gem-old-Ab3dEf9",
            ".config.yaml.gem-part",
            ".other.yaml.gem-Ab3dEf9h",
            ".config.yaml.gem-Ab3dEf9é",
            "x.config.yaml.gem-Ab3dEf9h",
        ] {
            assert!(!is_own_sibling(other, target), "{other}");
        }
        // Names shortened to fit 255 bytes still match their own target only.
        let long = format!("/d/{}", "é".repeat(200));
        let temp = temp_sibling(&long, "Zz9Zz9Zz");
        assert!(is_own_sibling(path::file_name(&temp), &long));
        assert!(!is_own_sibling(path::file_name(&temp), "/d/short"));
    }

    #[test]
    fn temp_names_stay_within_255_bytes() {
        let long = format!("/d/{}", "é".repeat(200));
        let temp = temp_sibling(&long, "Ab3dEf9h");
        assert!(path::file_name(&temp).len() <= 255);
        assert!(temp.starts_with("/d/.é"));
        assert!(temp.ends_with(".gem-Ab3dEf9h"));
    }
}
