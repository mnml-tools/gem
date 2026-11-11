//! Browsing and file operations on a session.

use tauri::{AppHandle, Runtime};

use super::support::session;
use crate::domain::entry::{DeleteReport, Entry, Listing};
use crate::error::AppResult;
use crate::services::files;

#[tauri::command]
pub async fn list_dir<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    path: String,
) -> AppResult<Listing> {
    let session = session(&app, &session_id)?;
    let fs = session.fs()?;
    session.observe(files::list(&*fs, &path).await)
}

#[tauri::command]
pub async fn stat_path<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    path: String,
) -> AppResult<Entry> {
    let session = session(&app, &session_id)?;
    let fs = session.fs()?;
    session.observe(files::stat(&*fs, &path).await)
}

#[tauri::command]
pub async fn make_dir<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    path: String,
) -> AppResult<Entry> {
    let session = session(&app, &session_id)?;
    let fs = session.fs()?;
    session.observe(files::make_dir(&*fs, &path).await)
}

/// Creates an empty file; fails if the name exists.
#[tauri::command]
pub async fn make_file<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    path: String,
) -> AppResult<Entry> {
    let session = session(&app, &session_id)?;
    let fs = session.fs()?;
    session.observe(files::make_file(&*fs, &path).await)
}

/// Fails if the target exists.
#[tauri::command]
pub async fn rename_path<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    from: String,
    to: String,
) -> AppResult<Entry> {
    let session = session(&app, &session_id)?;
    let fs = session.fs()?;
    session.observe(files::rename(&*fs, &from, &to).await)
}

/// Deletes files and directories (recursively).
#[tauri::command]
pub async fn delete_paths<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    paths: Vec<String>,
) -> AppResult<DeleteReport> {
    let session = session(&app, &session_id)?;
    let fs = session.fs()?;
    let home = session.home().unwrap_or_default();
    let report = files::delete_paths(&*fs, &home, &paths).await;
    // A dropped connection shows up as failures in the report; let the session notice it.
    if !report.failed.is_empty() {
        session.check_alive();
    }
    Ok(report)
}

#[tauri::command]
pub async fn chmod_path<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    path: String,
    mode: u32,
) -> AppResult<Entry> {
    let session = session(&app, &session_id)?;
    let fs = session.fs()?;
    session.observe(files::chmod(&*fs, &path, mode).await)
}
