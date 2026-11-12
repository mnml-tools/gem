//! Gem desktop application: a minimal remote file manager for SFTP, FTP and FTPS.

mod commands;
mod domain;
mod error;
mod platform;
mod remote;
mod services;
mod storage;
mod text;

use std::sync::Arc;
use std::time::Duration;

use tauri::{App, AppHandle, Manager, RunEvent, WindowEvent};

use commands::support::DataState;
use platform::drag_drop::DropRegistry;
use services::secrets::{KeyringProbe, KeyringStore, SecretStore};
use services::sessions::{RemoteConnector, SessionManager};
use storage::Database;

const DATABASE_FILE: &str = "gem.sqlite3";

/// How long quitting waits for sessions to close politely.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(3);

/// Every command exposed to the UI (keep in sync with build.rs and capabilities/main.json).
#[macro_export]
#[doc(hidden)]
macro_rules! app_commands {
    () => {
        tauri::generate_handler![
            $crate::commands::app::app_status,
            $crate::commands::app::get_settings,
            $crate::commands::app::update_settings,
            $crate::commands::app::choose_download_folder,
            $crate::commands::app::open_download_folder,
            $crate::commands::app::save_log,
            $crate::commands::app::open_release_notes,
            $crate::commands::sites::list_sites,
            $crate::commands::sites::create_site,
            $crate::commands::sites::update_site,
            $crate::commands::sites::delete_site,
            $crate::commands::sites::duplicate_site,
            $crate::commands::sites::reorder_sites,
            $crate::commands::sites::choose_key_file,
            $crate::commands::sites::forget_host_key,
            $crate::commands::sites::import_ssh_config,
            $crate::commands::sites::import_filezilla,
            $crate::commands::sites::export_sites,
            $crate::commands::sites::import_sites_file,
            $crate::commands::sessions::connect,
            $crate::commands::sessions::test_connection,
            $crate::commands::sessions::trust_host_key,
            $crate::commands::sessions::disconnect,
            $crate::commands::sessions::reconnect,
            $crate::commands::files::list_dir,
            $crate::commands::files::stat_path,
            $crate::commands::files::make_dir,
            $crate::commands::files::make_file,
            $crate::commands::files::rename_path,
            $crate::commands::files::delete_paths,
            $crate::commands::files::chmod_path,
            $crate::commands::editor::open_file,
            $crate::commands::editor::read_more,
            $crate::commands::editor::save_file,
            $crate::commands::transfers::download_paths,
            $crate::commands::transfers::upload_files,
            $crate::commands::transfers::upload_dropped,
            $crate::commands::transfers::cancel_transfer,
            $crate::commands::transfers::retry_transfer,
            $crate::commands::transfers::clear_transfers,
            $crate::commands::transfers::resolve_conflict,
            $crate::commands::shell::list_snippets,
            $crate::commands::shell::create_snippet,
            $crate::commands::shell::update_snippet,
            $crate::commands::shell::delete_snippet,
            $crate::commands::shell::reorder_snippets,
            $crate::commands::shell::run_command,
            $crate::commands::shell::cancel_command,
        ]
    };
}

/// Builds and runs the Tauri application.
pub fn run() {
    let builder = tauri::Builder::default()
        // Must be the first plugin so a second launch is forwarded before anything else runs.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Err(error) = platform::window::show_main(app) {
                log::error!("could not show the window: {error}");
            }
        }))
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                // The protocol crates log raw wire traffic at debug/trace (suppaftp writes
                // `PASS <password>` at trace). Pin them so raising the global level can never
                // put a secret in the app log; the connection log is Gem's own, redacted one.
                .level_for("suppaftp", log::LevelFilter::Warn)
                .level_for("russh", log::LevelFilter::Warn)
                .level_for("russh_sftp", log::LevelFilter::Warn)
                .max_file_size(2 * 1024 * 1024)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepSome(3))
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(setup)
        .on_window_event(|window, event| {
            if let WindowEvent::DragDrop(drop) = event {
                platform::drag_drop::handle(window, drop);
            }
        })
        .invoke_handler(app_commands!());

    let app = match builder.build(tauri::generate_context!()) {
        Ok(app) => app,
        Err(error) => {
            eprintln!("Gem failed to start: {error}");
            std::process::exit(1);
        }
    };
    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            close_sessions(handle);
        }
    });
}

fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle();
    app.manage(DataState(open_database(handle)));
    let secrets: Arc<dyn SecretStore> = Arc::new(KeyringStore::new());
    app.manage(SessionManager::new(
        Arc::new(RemoteConnector),
        Arc::clone(&secrets),
    ));
    app.manage(KeyringProbe::default());
    // Probe the keyring once, off the main thread; `app_status` waits for the answer.
    let probe = handle.clone();
    tauri::async_runtime::spawn(async move {
        let available = probe.state::<KeyringProbe>().available(secrets).await;
        log::info!("system keyring available: {available}");
    });
    app.manage(DropRegistry::default());
    platform::window::ensure_main(handle)?;
    Ok(())
}

/// Cancels transfers (removing their partial files) and closes every connection on quit.
fn close_sessions(app: &AppHandle) {
    let Some(sessions) = app.try_state::<SessionManager>() else {
        return;
    };
    tauri::async_runtime::block_on(async {
        if tokio::time::timeout(CLOSE_TIMEOUT, sessions.close_all())
            .await
            .is_err()
        {
            log::warn!("some connections did not close in time");
        }
    });
}

/// Opens the database in the OS app-data directory (never next to the binary). Failures are
/// kept and shown by the UI instead of crashing; the data is never modified then.
fn open_database(app: &AppHandle) -> Result<Arc<Database>, String> {
    let path = match std::env::var_os("GEM_DATA_DIR") {
        Some(dir) => std::path::PathBuf::from(dir).join(DATABASE_FILE),
        None => app
            .path()
            .app_data_dir()
            .map_err(|e| format!("no app data directory: {e}"))?
            .join(DATABASE_FILE),
    };
    let db = Database::open(&path).map_err(|e| e.to_string())?;
    log::info!("database ready (schema v{})", storage::SCHEMA_VERSION);
    Ok(Arc::new(db))
}
