//! SQLite storage: connection management, pragmas and schema migrations.
//!
//! All access goes through [`Database`], which serialises use of a single connection. Writes run
//! in `BEGIN IMMEDIATE` transactions so multi-table changes are atomic.

mod migrations;

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};

use crate::error::{AppError, AppResult};

pub use migrations::{SCHEMA_VERSION, migrate};

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    /// Opens (creating if needed) the database file and brings the schema up to date.
    pub fn open(path: &Path) -> AppResult<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
            restrict_to_user(dir);
        }
        let mut conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        configure(&conn, true)?;
        migrate(&mut conn, Some(path))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// In-memory database, used by tests.
    #[cfg(test)]
    pub fn open_in_memory() -> AppResult<Self> {
        let mut conn = Connection::open_in_memory()?;
        configure(&conn, false)?;
        migrate(&mut conn, None)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> AppResult<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| AppError::Internal("database lock poisoned".into()))
    }

    /// Runs a read-only closure.
    pub fn read<T>(&self, f: impl FnOnce(&Connection) -> AppResult<T>) -> AppResult<T> {
        let conn = self.lock()?;
        f(&conn)
    }

    /// Runs `f` in an immediate transaction; commits on `Ok`, rolls back on `Err`.
    pub fn write<T>(&self, f: impl FnOnce(&Transaction<'_>) -> AppResult<T>) -> AppResult<T> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = f(&tx)?;
        tx.commit()?;
        Ok(value)
    }
}

/// Data directory readable by the user only (Unix); best effort.
fn restrict_to_user(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(error) = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)) {
            log::warn!("could not restrict the data directory: {error}");
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
}

fn configure(conn: &Connection, file_backed: bool) -> AppResult<()> {
    if file_backed {
        // WAL keeps readers and the writer from blocking each other; FULL sync makes every
        // committed change (a new site, a trusted host key) durable even across power loss.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
    }
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    // Schema objects never run untrusted SQL functions.
    conn.pragma_update(None, "trusted_schema", "OFF")?;
    // Deleted rows (sites, host keys) are overwritten instead of lingering in free pages.
    conn.pragma_update(None, "secure_delete", "ON")?;
    Ok(())
}
