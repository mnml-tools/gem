//! The transfer queue. Local paths come only from native dialogs and OS drops, never from
//! the webview.

use std::path::PathBuf;

use tauri::{AppHandle, Manager, Runtime};

use super::support::{current_settings, download_folder, file_dialog, local_path, session, show};
use crate::domain::transfer::{ConflictPolicy, Transfer};
use crate::error::AppResult;
use crate::platform::drag_drop::DropRegistry;
use crate::services::sessions::SessionManager;

/// Queues downloads to the download folder (or one chosen in a native dialog).
#[tauri::command]
pub async fn download_paths<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    paths: Vec<String>,
    choose_folder: bool,
) -> AppResult<Vec<Transfer>> {
    let settings = current_settings(&app).await;
    let session = session(&app, &session_id)?;
    let fs = session.fs()?;
    let folder = if choose_folder {
        let mut dialog = file_dialog(&app, "Download to");
        if let Ok(default) = download_folder(&app, &settings).await {
            dialog = dialog.set_directory(default);
        }
        match show(move || dialog.blocking_pick_folder()).await? {
            Some(picked) => local_path(picked)?,
            None => return Ok(Vec::new()),
        }
    } else {
        download_folder(&app, &settings).await?
    };
    let queued = session
        .transfers
        .enqueue_downloads(&*fs, &paths, &folder, &settings)
        .await;
    session.observe(queued)
}

/// Native dialog for files (or folders), then queues uploads.
#[tauri::command]
pub async fn upload_files<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    remote_dir: String,
    folders: bool,
) -> AppResult<Vec<Transfer>> {
    // Fail before showing the dialog when the session is gone.
    session(&app, &session_id)?.fs()?;
    let dialog = file_dialog(
        &app,
        if folders {
            "Upload folders"
        } else {
            "Upload files"
        },
    );
    let picked = if folders {
        show(move || dialog.blocking_pick_folders()).await?
    } else {
        show(move || dialog.blocking_pick_files()).await?
    };
    let Some(picked) = picked else {
        return Ok(Vec::new());
    };
    let paths = picked
        .into_iter()
        .map(local_path)
        .collect::<AppResult<Vec<PathBuf>>>()?;
    upload(&app, &session_id, paths, &remote_dir).await
}

/// Queues uploads of the paths of an OS drop (`gem://drop`).
#[tauri::command]
pub async fn upload_dropped<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    drop_id: String,
    remote_dir: String,
) -> AppResult<Vec<Transfer>> {
    let paths = app.state::<DropRegistry>().take(&drop_id)?;
    upload(&app, &session_id, paths, &remote_dir).await
}

async fn upload<R: Runtime>(
    app: &AppHandle<R>,
    session_id: &str,
    paths: Vec<PathBuf>,
    remote_dir: &str,
) -> AppResult<Vec<Transfer>> {
    let settings = current_settings(app).await;
    let session = session(app, session_id)?;
    let fs = session.fs()?;
    let queued = session
        .transfers
        .enqueue_uploads(&*fs, paths, remote_dir, &settings)
        .await;
    session.observe(queued)
}

#[tauri::command]
pub async fn cancel_transfer<R: Runtime>(app: AppHandle<R>, id: String) -> AppResult<()> {
    app.state::<SessionManager>()
        .session_of_transfer(&id)?
        .transfers
        .cancel(&id)
}

#[tauri::command]
pub async fn retry_transfer<R: Runtime>(app: AppHandle<R>, id: String) -> AppResult<()> {
    app.state::<SessionManager>()
        .session_of_transfer(&id)?
        .transfers
        .retry(&id)
}

/// Forgets finished transfers.
#[tauri::command]
pub async fn clear_transfers<R: Runtime>(app: AppHandle<R>, session_id: String) -> AppResult<()> {
    session(&app, &session_id)?.transfers.clear_finished();
    Ok(())
}

/// Answers a transfer waiting in `conflict` state.
#[tauri::command]
pub async fn resolve_conflict<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    decision: ConflictPolicy,
    apply_to_all: bool,
) -> AppResult<()> {
    app.state::<SessionManager>()
        .session_of_transfer(&id)?
        .transfers
        .resolve(&id, decision, apply_to_all)
}
