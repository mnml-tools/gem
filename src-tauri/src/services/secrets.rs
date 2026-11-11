//! Site secrets (passwords and key passphrases) in the OS keyring.
//!
//! Secrets never touch SQLite, JSON files or logs. There is no plain-text fallback: when the
//! keyring is unavailable (no Secret Service on a minimal Linux), saving fails with a clear
//! `Platform` error and the user types the secret at each connection instead
//! (docs/adr/0012-secret-store.md). Calls may block (D-Bus, Keychain): run them off the async
//! runtime.

use std::sync::Arc;

use crate::error::{AppError, AppResult};

/// Keyring service name: the app identifier (tauri.conf.json).
pub const SERVICE: &str = "app.gem.Gem";

/// Account looked up (never written) to find out whether the keyring answers.
const PROBE_ACCOUNT: &str = "gem-keyring-probe";

/// What saving a site with a secret says when the keyring is missing.
pub const REMEMBER_UNAVAILABLE: &str = "The system keyring isn't available, so Gem can't \
     remember this password. Untick \"Remember\" to save the site; Gem will ask for the \
     password when connecting.";

/// Where secrets are kept, keyed by site id.
pub trait SecretStore: Send + Sync {
    fn get(&self, site_id: &str) -> AppResult<Option<String>>;
    fn set(&self, site_id: &str, secret: &str) -> AppResult<()>;
    /// Removes the secret; succeeds when there was none.
    fn delete(&self, site_id: &str) -> AppResult<()>;
    /// Whether the store answers at all (a read of an entry that does not exist; nothing is
    /// ever created).
    fn available(&self) -> bool;
}

/// Turns a failure to store a secret into the actionable message when the keyring is missing.
pub fn remember_error(store: &dyn SecretStore, error: AppError) -> AppError {
    if store.available() {
        error
    } else {
        AppError::Platform(REMEMBER_UNAVAILABLE.into())
    }
}

/// The keyring probe, run once off the main thread at startup (Tauri-managed state) and
/// reported by `app_status`.
#[derive(Default)]
pub struct KeyringProbe(tokio::sync::OnceCell<bool>);

impl KeyringProbe {
    pub async fn available(&self, store: Arc<dyn SecretStore>) -> bool {
        *self
            .0
            .get_or_init(|| async move {
                super::blocking(move || Ok(store.available()))
                    .await
                    .unwrap_or(false)
            })
            .await
    }
}

/// The OS keyring (Secret Service, macOS Keychain, Windows Credential Manager).
pub struct KeyringStore {
    service: String,
}

impl KeyringStore {
    pub fn new() -> Self {
        Self {
            service: SERVICE.to_owned(),
        }
    }

    fn entry(&self, site_id: &str) -> AppResult<keyring::Entry> {
        keyring::Entry::new(&self.service, site_id).map_err(keyring_error)
    }
}

/// Keyring errors never contain the secret; the message says what to do.
fn keyring_error(error: keyring::Error) -> AppError {
    match error {
        keyring::Error::NoStorageAccess(_) | keyring::Error::PlatformFailure(_) => {
            AppError::Platform(format!(
                "The system keyring is not available ({error}). Passwords cannot be saved; \
                 Gem will ask for them when connecting."
            ))
        }
        // Its message lists the matching credentials: keep them out of the UI and the logs.
        keyring::Error::Ambiguous(_) => AppError::Platform(
            "The system keyring holds several entries for this site; remove them in the \
             keyring manager"
                .into(),
        ),
        other => AppError::Platform(format!("The system keyring failed: {other}")),
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, site_id: &str) -> AppResult<Option<String>> {
        match self.entry(site_id)?.get_password() {
            Ok(secret) => Ok(Some(secret)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(keyring_error(error)),
        }
    }

    fn set(&self, site_id: &str, secret: &str) -> AppResult<()> {
        self.entry(site_id)?
            .set_password(secret)
            .map_err(keyring_error)
    }

    fn delete(&self, site_id: &str) -> AppResult<()> {
        match self.entry(site_id)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(keyring_error(error)),
        }
    }

    fn available(&self) -> bool {
        let Ok(entry) = keyring::Entry::new(&self.service, PROBE_ACCOUNT) else {
            return false;
        };
        match entry.get_password() {
            // It answered, whatever it said.
            Ok(_)
            | Err(
                keyring::Error::NoEntry
                | keyring::Error::BadEncoding(_)
                | keyring::Error::Ambiguous(_),
            ) => true,
            Err(error) => {
                log::warn!("the system keyring is not available: {error}");
                false
            }
        }
    }
}

/// An in-memory store for tests, with a switch that makes it fail like a missing keyring.
#[cfg(test)]
#[derive(Default)]
pub struct MemoryStore {
    secrets: std::sync::Mutex<std::collections::HashMap<String, String>>,
    broken: std::sync::atomic::AtomicBool,
    reads: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
impl MemoryStore {
    pub fn set_broken(&self, broken: bool) {
        self.broken
            .store(broken, std::sync::atomic::Ordering::SeqCst);
    }

    /// How many times a secret was read.
    pub fn reads(&self) -> usize {
        self.reads.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn check(&self) -> AppResult<()> {
        if self.broken.load(std::sync::atomic::Ordering::SeqCst) {
            Err(AppError::Platform(
                "The system keyring is not available".into(),
            ))
        } else {
            Ok(())
        }
    }

    fn map(&self) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, String>> {
        self.secrets.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
impl SecretStore for MemoryStore {
    fn get(&self, site_id: &str) -> AppResult<Option<String>> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.check()?;
        Ok(self.map().get(site_id).cloned())
    }

    fn set(&self, site_id: &str, secret: &str) -> AppResult<()> {
        self.check()?;
        self.map().insert(site_id.to_owned(), secret.to_owned());
        Ok(())
    }

    fn delete(&self, site_id: &str) -> AppResult<()> {
        self.check()?;
        self.map().remove(site_id);
        Ok(())
    }

    fn available(&self) -> bool {
        !self.broken.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_round_trip_and_failure() {
        let store = MemoryStore::default();
        assert_eq!(store.get("a").unwrap(), None);
        store.set("a", "s3cret").unwrap();
        assert_eq!(store.get("a").unwrap().as_deref(), Some("s3cret"));
        store.delete("a").unwrap();
        store.delete("a").unwrap();
        assert_eq!(store.get("a").unwrap(), None);
        assert!(store.available());
        store.set_broken(true);
        assert!(!store.available());
        assert_eq!(
            store.set("a", "x").unwrap_err().code(),
            crate::error::ErrorCode::Platform
        );
        let error = remember_error(&store, store.set("a", "x").unwrap_err());
        assert_eq!(error.to_string(), REMEMBER_UNAVAILABLE);
    }

    #[tokio::test]
    async fn the_probe_runs_once() {
        let store = Arc::new(MemoryStore::default());
        let probe = KeyringProbe::default();
        assert!(
            probe
                .available(Arc::clone(&store) as Arc<dyn SecretStore>)
                .await
        );
        store.set_broken(true);
        // Cached: the startup answer stays.
        assert!(probe.available(store as Arc<dyn SecretStore>).await);
    }

    #[test]
    fn keyring_errors_are_platform_errors_without_secrets() {
        let error = keyring_error(keyring::Error::NoStorageAccess("no dbus".into()));
        assert_eq!(error.code(), crate::error::ErrorCode::Platform);
        assert!(error.to_string().contains("not available"));
    }
}
