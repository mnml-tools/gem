//! Shared plumbing for commands: database access off the async runtime, sessions, settings
//! and native dialogs.

use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::{Connection, Transaction};
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_dialog::{DialogExt, FileDialogBuilder, FilePath};

use crate::domain::session::SessionEvent;
use crate::domain::settings::Settings;
use crate::error::{AppError, AppResult};
use crate::platform::window::MAIN;
use crate::services::sessions::{EventSink, Session, SessionManager};
use crate::services::settings;
use crate::storage::Database;

/// The opened database, or why it could not be opened (shown by the UI instead of crashing).
pub struct DataState(pub Result<Arc<Database>, String>);

impl DataState {
    pub fn database(&self) -> AppResult<Arc<Database>> {
        self.0
            .as_ref()
            .map(Arc::clone)
            .map_err(|reason| AppError::Storage(reason.clone()))
    }

    pub fn error(&self) -> Option<String> {
        self.0.as_ref().err().cloned()
    }
}

pub fn database<R: Runtime>(app: &AppHandle<R>) -> AppResult<Arc<Database>> {
    app.state::<DataState>().database()
}

/// Runs blocking work (SQLite, keyring, local files) on a worker thread.
pub async fn blocking<T: Send + 'static>(
    job: impl FnOnce() -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(job)
        .await
        .map_err(|e| AppError::Internal(format!("background task failed: {e}")))?
}

/// Runs a read on a worker thread.
pub async fn read<R: Runtime, T: Send + 'static>(
    app: &AppHandle<R>,
    f: impl FnOnce(&Connection) -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    let db = database(app)?;
    blocking(move || db.read(f)).await
}

/// Runs a write transaction on a worker thread.
pub async fn write<R: Runtime, T: Send + 'static>(
    app: &AppHandle<R>,
    f: impl FnOnce(&Transaction<'_>) -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    let db = database(app)?;
    blocking(move || db.write(f)).await
}

/// The current settings; the defaults when the database is unavailable (browsing and the
/// viewer still work then).
pub async fn current_settings<R: Runtime>(app: &AppHandle<R>) -> Settings {
    match read(app, settings::get).await {
        Ok(settings) => settings,
        Err(error) => {
            log::warn!("using default settings: {error}");
            Settings::default()
        }
    }
}

/// A live session by id.
pub fn session<R: Runtime>(app: &AppHandle<R>, id: &str) -> AppResult<Arc<Session>> {
    app.state::<SessionManager>().get(id)
}

/// Forwards a session's events to its tab's channel.
pub fn event_sink(channel: Channel<SessionEvent>) -> EventSink {
    Arc::new(move |event| {
        if let Err(error) = channel.send(event) {
            log::debug!("a session event was not delivered: {error}");
        }
    })
}

/// Where downloads go: the chosen folder, or the OS Downloads folder (created if needed).
pub async fn download_folder<R: Runtime>(
    app: &AppHandle<R>,
    settings: &Settings,
) -> AppResult<PathBuf> {
    let folder = match &settings.download_folder {
        Some(folder) => PathBuf::from(folder),
        None => app
            .path()
            .download_dir()
            .map_err(|e| AppError::Platform(format!("no Downloads folder: {e}")))?,
    };
    let created = folder.clone();
    blocking(move || {
        std::fs::create_dir_all(&created)?;
        Ok(())
    })
    .await?;
    Ok(folder)
}

/// A native file dialog, modal to the main window.
pub fn file_dialog<R: Runtime>(app: &AppHandle<R>, title: &str) -> FileDialogBuilder<R> {
    let dialog = app.dialog().file().set_title(title);
    match app.get_webview_window(MAIN) {
        Some(window) => dialog.set_parent(&window),
        None => dialog,
    }
}

/// Runs a blocking dialog off the async runtime.
pub async fn show<T: Send + 'static>(dialog: impl FnOnce() -> T + Send + 'static) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(dialog)
        .await
        .map_err(|e| AppError::Internal(format!("dialog failed: {e}")))
}

/// The local path of a dialog result (dialogs on desktop always return paths).
pub fn local_path(picked: FilePath) -> AppResult<PathBuf> {
    picked
        .into_path()
        .map_err(|e| AppError::Platform(format!("unusable path: {e}")))
}

/// The user's home directory.
pub fn home_dir<R: Runtime>(app: &AppHandle<R>) -> AppResult<PathBuf> {
    app.path()
        .home_dir()
        .map_err(|e| AppError::Platform(format!("no home folder: {e}")))
}
