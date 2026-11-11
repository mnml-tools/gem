//! The in-memory viewer and editor.

use tauri::{AppHandle, Runtime};

use super::support::{current_settings, session};
use crate::domain::file::{FileChunk, OpenedFile, SaveFile, SavedFile};
use crate::error::AppResult;
use crate::services::editor;

/// Reads the first window (or the whole file) into memory.
#[tauri::command]
pub async fn open_file<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    path: String,
    whole: bool,
) -> AppResult<OpenedFile> {
    let window_kib = current_settings(&app).await.preview_window_kib;
    let session = session(&app, &session_id)?;
    let fs = session.fs()?;
    session.observe(editor::open_file(&*fs, &path, whole, window_kib).await)
}

/// Reads the next window; `Conflict` if the file changed since it was opened.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn read_more<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    path: String,
    offset: u64,
    encoding: String,
    expected_size: u64,
    expected_mtime: Option<i64>,
    to_end: bool,
) -> AppResult<FileChunk> {
    let window_kib = current_settings(&app).await.preview_window_kib;
    let session = session(&app, &session_id)?;
    let fs = session.fs()?;
    session.observe(
        editor::read_more(
            &*fs,
            &path,
            offset,
            &encoding,
            expected_size,
            expected_mtime,
            to_end,
            window_kib,
        )
        .await,
    )
}

/// Conflict check, re-encoding and atomic write.
#[tauri::command]
pub async fn save_file<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    path: String,
    request: SaveFile,
) -> AppResult<SavedFile> {
    let session = session(&app, &session_id)?;
    let fs = session.fs()?;
    session.observe(editor::save_file(&*fs, &path, request).await)
}
