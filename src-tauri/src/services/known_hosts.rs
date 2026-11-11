//! Trusted server identities (SSH host keys and FTPS certificates), trust on first use.
//!
//! One identity per host and port. A different fingerprint for a known host is reported as
//! changed and the connection is refused; there is no one-click override: the user removes
//! the stored key in the site's settings first (plan §3.6).

use std::sync::Arc;

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::domain::session::HostKeyInfo;
use crate::error::{AppError, AppResult};
use crate::remote::{HostDecision, HostVerifier};
use crate::storage::Database;

/// Host names are case-insensitive.
fn host_key(host: &str) -> String {
    host.trim().to_ascii_lowercase()
}

/// The stored `(key_type, fingerprint)` for a host and port.
pub fn get(conn: &Connection, host: &str, port: u16) -> AppResult<Option<(String, String)>> {
    Ok(conn
        .query_row(
            "SELECT key_type, fingerprint FROM known_hosts WHERE host = ?1 AND port = ?2",
            params![host_key(host), port],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}

pub fn check(
    conn: &Connection,
    host: &str,
    port: u16,
    fingerprint: &str,
) -> AppResult<HostDecision> {
    Ok(match get(conn, host, port)? {
        None => HostDecision::Unknown,
        Some((_, stored)) if stored == fingerprint => HostDecision::Trusted,
        Some((_, stored)) => HostDecision::Changed {
            previous_fingerprint: stored,
        },
    })
}

/// Stores an identity the user accepted. Trusting the stored key again is a no-op; a
/// different key is never overwritten.
pub fn trust(tx: &Transaction<'_>, key: &HostKeyInfo) -> AppResult<()> {
    if key.fingerprint.trim().is_empty() || key.host.trim().is_empty() || key.port == 0 {
        return Err(AppError::validation("incomplete host key"));
    }
    match get(tx, &key.host, key.port)? {
        Some((_, stored)) if stored == key.fingerprint => Ok(()),
        Some(_) => Err(AppError::Conflict(format!(
            "A different key is already trusted for {}:{}. Remove it in the site's settings \
             first if the server's key really changed.",
            key.host, key.port
        ))),
        None => {
            tx.execute(
                "INSERT INTO known_hosts (host, port, key_type, fingerprint, added_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    host_key(&key.host),
                    key.port,
                    key.key_type,
                    key.fingerprint,
                    Utc::now().to_rfc3339()
                ],
            )?;
            Ok(())
        }
    }
}

/// Removes the trusted identity of a host. True if one was stored.
pub fn forget(tx: &Transaction<'_>, host: &str, port: u16) -> AppResult<bool> {
    let removed = tx.execute(
        "DELETE FROM known_hosts WHERE host = ?1 AND port = ?2",
        params![host_key(host), port],
    )?;
    Ok(removed > 0)
}

/// The [`HostVerifier`] handed to the remote layer.
pub struct KnownHosts {
    db: Arc<Database>,
}

impl KnownHosts {
    pub fn new(db: Arc<Database>) -> Self {
        Self { db }
    }
}

impl HostVerifier for KnownHosts {
    fn check(&self, host: &str, port: u16, _key_type: &str, fingerprint: &str) -> HostDecision {
        match self.db.read(|c| check(c, host, port, fingerprint)) {
            Ok(decision) => decision,
            Err(error) => {
                // Unknown makes the UI ask; trusting will then report the storage error.
                log::error!("could not read the known hosts: {error}");
                HostDecision::Unknown
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(fingerprint: &str) -> HostKeyInfo {
        HostKeyInfo {
            host: "Example.TEST".into(),
            port: 22,
            key_type: "ssh-ed25519".into(),
            fingerprint: fingerprint.into(),
            previous_fingerprint: None,
        }
    }

    #[test]
    fn trust_on_first_use_then_changed() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let verifier = KnownHosts::new(Arc::clone(&db));
        assert_eq!(
            verifier.check("example.test", 22, "ssh-ed25519", "SHA256:a"),
            HostDecision::Unknown
        );
        db.write(|tx| trust(tx, &key("SHA256:a"))).unwrap();
        assert_eq!(
            verifier.check("example.test", 22, "ssh-ed25519", "SHA256:a"),
            HostDecision::Trusted
        );
        // Another key type with another fingerprint is still a change (one identity per host).
        assert_eq!(
            verifier.check("EXAMPLE.test", 22, "ssh-rsa", "SHA256:b"),
            HostDecision::Changed {
                previous_fingerprint: "SHA256:a".into()
            }
        );
        // Another port is another identity.
        assert_eq!(
            verifier.check("example.test", 2222, "ssh-ed25519", "SHA256:b"),
            HostDecision::Unknown
        );
    }

    #[test]
    fn trust_never_overwrites_a_different_key() {
        let db = Database::open_in_memory().unwrap();
        db.write(|tx| trust(tx, &key("SHA256:a"))).unwrap();
        db.write(|tx| trust(tx, &key("SHA256:a"))).unwrap();
        let error = db.write(|tx| trust(tx, &key("SHA256:b"))).unwrap_err();
        assert_eq!(error.code(), crate::error::ErrorCode::Conflict);
        assert!(error.to_string().contains("site's settings"));
        assert!(db.write(|tx| forget(tx, "example.test", 22)).unwrap());
        assert!(!db.write(|tx| forget(tx, "example.test", 22)).unwrap());
        db.write(|tx| trust(tx, &key("SHA256:b"))).unwrap();
        assert_eq!(
            db.read(|c| get(c, "example.test", 22)).unwrap(),
            Some(("ssh-ed25519".into(), "SHA256:b".into()))
        );
    }
}
