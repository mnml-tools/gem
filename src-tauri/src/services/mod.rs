//! Business logic between the commands and the remote layer / storage.
//!
//! Database services are plain functions over `rusqlite` (like Piw) so they can be tested
//! without Tauri; remote services work on `&dyn RemoteFs`, so they are tested against the
//! in-memory remote (`remote::memory`). Live state (sessions and their transfer queues) is
//! owned by [`sessions::SessionManager`], which Tauri manages.

pub mod commands;
pub mod editor;
pub mod files;
pub mod imports;
pub mod known_hosts;
pub mod secrets;
pub mod sessions;
pub mod settings;
pub mod sites;
pub mod snippets;
pub mod transfers;

#[cfg(test)]
mod it;
#[cfg(test)]
pub(crate) mod test_support;

use crate::error::{AppError, AppResult};

/// Runs blocking work (SQLite, keyring, local files) on a worker thread of the current tokio
/// runtime (Tauri's in the app).
pub(crate) async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    tokio::task::spawn_blocking(job)
        .await
        .map_err(|e| AppError::Internal(format!("background task failed: {e}")))?
}
