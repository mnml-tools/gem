//! Saved sites, their secrets, trusted host keys and imports.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager, Runtime};

use super::support::{blocking, database, file_dialog, home_dir, local_path, read, show, write};
use crate::domain::site::{SecretUpdate, Site, SiteInput};
use crate::error::{AppError, AppResult};
use crate::services::sessions::SessionManager;
use crate::services::{imports, known_hosts, sites};

/// Largest import file read (a sitemanager.xml or an export is a few KiB).
const MAX_IMPORT_BYTES: u64 = 16 * 1024 * 1024;

fn read_import(path: &Path) -> AppResult<String> {
    let size = std::fs::metadata(path)?.len();
    if size > MAX_IMPORT_BYTES {
        return Err(AppError::validation("the file is too large to import"));
    }
    std::fs::read_to_string(path)
        .map_err(|e| AppError::validation(format!("cannot read the file: {e}")))
}

#[tauri::command]
pub async fn list_sites<R: Runtime>(app: AppHandle<R>) -> AppResult<Vec<Site>> {
    read(&app, sites::list).await
}

#[tauri::command]
pub async fn create_site<R: Runtime>(
    app: AppHandle<R>,
    input: SiteInput,
    secret: Option<String>,
) -> AppResult<Site> {
    let db = database(&app)?;
    let store = app.state::<SessionManager>().secrets();
    blocking(move || sites::create(&db, store.as_ref(), input, secret)).await
}

#[tauri::command]
pub async fn update_site<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    input: SiteInput,
    secret: SecretUpdate,
) -> AppResult<Site> {
    let db = database(&app)?;
    let store = app.state::<SessionManager>().secrets();
    blocking(move || sites::update(&db, store.as_ref(), &id, input, secret)).await
}

/// Deletes the site and its keyring secret.
#[tauri::command]
pub async fn delete_site<R: Runtime>(app: AppHandle<R>, id: String) -> AppResult<()> {
    let db = database(&app)?;
    let store = app.state::<SessionManager>().secrets();
    blocking(move || sites::delete(&db, store.as_ref(), &id)).await
}

#[tauri::command]
pub async fn duplicate_site<R: Runtime>(app: AppHandle<R>, id: String) -> AppResult<Site> {
    let db = database(&app)?;
    let store = app.state::<SessionManager>().secrets();
    blocking(move || sites::duplicate(&db, store.as_ref(), &id)).await
}

#[tauri::command]
pub async fn reorder_sites<R: Runtime>(app: AppHandle<R>, ids: Vec<String>) -> AppResult<()> {
    write(&app, move |tx| sites::reorder(tx, &ids)).await
}

/// Native file dialog for a private key.
#[tauri::command]
pub async fn choose_key_file<R: Runtime>(app: AppHandle<R>) -> AppResult<Option<String>> {
    let mut dialog = file_dialog(&app, "Choose a private key");
    if let Ok(ssh) = home_dir(&app).map(|h| h.join(".ssh"))
        && ssh.is_dir()
    {
        dialog = dialog.set_directory(ssh);
    }
    let picked = show(move || dialog.blocking_pick_file()).await?;
    picked
        .map(|p| local_path(p).map(|p| p.display().to_string()))
        .transpose()
}

/// Removes the trusted key of a host. True if one was stored.
#[tauri::command]
pub async fn forget_host_key<R: Runtime>(
    app: AppHandle<R>,
    host: String,
    port: u16,
) -> AppResult<bool> {
    write(&app, move |tx| known_hosts::forget(tx, &host, port)).await
}

/// Hosts from ~/.ssh/config, for the import dialog (nothing is saved yet).
#[tauri::command]
pub async fn import_ssh_config<R: Runtime>(app: AppHandle<R>) -> AppResult<Vec<SiteInput>> {
    let home = home_dir(&app)?;
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    blocking(move || {
        let config = home.join(".ssh").join("config");
        if !config.is_file() {
            return Ok(Vec::new());
        }
        let text = read_import(&config)?;
        Ok(imports::parse_ssh_config(
            &text,
            &home.display().to_string(),
            &user,
        ))
    })
    .await
}

/// Where FileZilla keeps sitemanager.xml, to open the dialog there.
fn filezilla_dir<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    let candidates = [
        app.path().config_dir().ok().map(|d| d.join("filezilla")),
        app.path().config_dir().ok().map(|d| d.join("FileZilla")),
        home_dir(app).ok().map(|h| h.join(".filezilla")),
    ];
    candidates.into_iter().flatten().find(|d| d.is_dir())
}

/// Sites from a FileZilla sitemanager.xml chosen in a native dialog (passwords are never imported).
#[tauri::command]
pub async fn import_filezilla<R: Runtime>(app: AppHandle<R>) -> AppResult<Vec<SiteInput>> {
    let mut dialog = file_dialog(&app, "Import FileZilla sites (sitemanager.xml)")
        .add_filter("FileZilla site manager", &["xml"]);
    if let Some(dir) = filezilla_dir(&app) {
        dialog = dialog.set_directory(dir);
    }
    let Some(picked) = show(move || dialog.blocking_pick_file()).await? else {
        return Ok(Vec::new());
    };
    let path = local_path(picked)?;
    blocking(move || imports::parse_filezilla(&read_import(&path)?)).await
}

/// Writes every site (no secrets) to a JSON file chosen in a native dialog.
#[tauri::command]
pub async fn export_sites<R: Runtime>(app: AppHandle<R>) -> AppResult<bool> {
    let json = imports::export_json(&read(&app, sites::list).await?)?;
    let dialog = file_dialog(&app, "Export sites")
        .set_file_name("gem-sites.json")
        .add_filter("Gem sites", &["json"]);
    let Some(picked) = show(move || dialog.blocking_save_file()).await? else {
        return Ok(false);
    };
    let path = local_path(picked)?;
    blocking(move || {
        std::fs::write(&path, json.as_bytes())?;
        Ok(())
    })
    .await?;
    Ok(true)
}

/// Reads a JSON export chosen in a native dialog.
#[tauri::command]
pub async fn import_sites_file<R: Runtime>(app: AppHandle<R>) -> AppResult<Vec<SiteInput>> {
    let dialog = file_dialog(&app, "Import sites").add_filter("Gem sites", &["json"]);
    let Some(picked) = show(move || dialog.blocking_pick_file()).await? else {
        return Ok(Vec::new());
    };
    let path = local_path(picked)?;
    blocking(move || imports::import_json(&read_import(&path)?)).await
}
