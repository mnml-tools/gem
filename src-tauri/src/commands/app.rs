//! App status, settings, download folder and log export.

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_opener::OpenerExt;

use super::support::{
    DataState, blocking, current_settings, download_folder, file_dialog, local_path, read, show,
    write,
};
use crate::domain::app::AppStatus;
use crate::domain::settings::Settings;
use crate::error::{AppError, AppResult};
use crate::services::secrets::KeyringProbe;
use crate::services::sessions::SessionManager;
use crate::services::settings;
use crate::services::transfers::local::sanitize_name;

/// Version, platform and whether the database opened.
#[tauri::command]
pub async fn app_status<R: Runtime>(app: AppHandle<R>) -> AppResult<AppStatus> {
    let platform = match std::env::consts::OS {
        "macos" => "macos",
        "windows" => "windows",
        _ => "linux",
    };
    let store = app.state::<SessionManager>().secrets();
    let keyring_available = app.state::<KeyringProbe>().available(store).await;
    Ok(AppStatus {
        version: app.package_info().version.to_string(),
        platform: platform.to_owned(),
        data_error: app.state::<DataState>().error(),
        keyring_available,
    })
}

#[tauri::command]
pub async fn get_settings<R: Runtime>(app: AppHandle<R>) -> AppResult<Settings> {
    read(&app, settings::get).await
}

/// Saves settings (values are clamped).
#[tauri::command]
pub async fn update_settings<R: Runtime>(
    app: AppHandle<R>,
    settings: Settings,
) -> AppResult<Settings> {
    write(&app, move |tx| settings::update(tx, settings)).await
}

/// Native folder dialog; stores the choice as the download folder.
#[tauri::command]
pub async fn choose_download_folder<R: Runtime>(app: AppHandle<R>) -> AppResult<Settings> {
    let current = read(&app, settings::get).await?;
    let mut dialog = file_dialog(&app, "Choose the download folder");
    if let Ok(folder) = download_folder(&app, &current).await {
        dialog = dialog.set_directory(folder);
    }
    let Some(picked) = show(move || dialog.blocking_pick_folder()).await? else {
        return Ok(current);
    };
    let folder = local_path(picked)?.display().to_string();
    write(&app, move |tx| {
        settings::set_download_folder(tx, Some(folder))
    })
    .await
}

/// Opens the download folder in the OS file manager.
#[tauri::command]
pub async fn open_download_folder<R: Runtime>(app: AppHandle<R>) -> AppResult<()> {
    let settings = current_settings(&app).await;
    let folder = download_folder(&app, &settings).await?;
    app.opener()
        .open_path(folder.display().to_string(), None::<&str>)
        .map_err(|e| AppError::Platform(format!("could not open the folder: {e}")))
}

/// The latest release's page, opened by the update dialog's "What's new".
const RELEASE_NOTES_URL: &str = "https://github.com/mnml-tools/gem/releases/latest";

/// Opens the latest release notes in the default browser. The URL is fixed here: the webview
/// cannot open arbitrary links.
#[tauri::command]
pub async fn open_release_notes<R: Runtime>(app: AppHandle<R>) -> AppResult<()> {
    app.opener()
        .open_url(RELEASE_NOTES_URL, None::<&str>)
        .map_err(|e| AppError::Platform(format!("could not open the release notes: {e}")))
}

/// Native save dialog for a connection log. False when cancelled.
#[tauri::command]
pub async fn save_log<R: Runtime>(
    app: AppHandle<R>,
    text: String,
    suggested_name: String,
) -> AppResult<bool> {
    let name = sanitize_name(suggested_name.trim());
    let name = if name == "_" {
        "gem.log".to_owned()
    } else {
        name
    };
    let dialog = file_dialog(&app, "Save the connection log")
        .set_file_name(name)
        .add_filter("Log", &["log", "txt"]);
    let Some(picked) = show(move || dialog.blocking_save_file()).await? else {
        return Ok(false);
    };
    let path = local_path(picked)?;
    blocking(move || {
        std::fs::write(&path, text.as_bytes())?;
        Ok(())
    })
    .await?;
    Ok(true)
}
