//! Remote commands (SSH exec) and snippets (ADR 0014).

use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, Runtime};

use super::support::{read, session, write};
use crate::domain::command::{CommandEvent, RunStarted, Snippet, SnippetInput};
use crate::error::AppResult;
use crate::services::commands::{self, CommandSink};
use crate::services::sessions::SessionManager;
use crate::services::snippets;

/// Global snippets plus those of `site_id`, globals first, each group in its saved order.
#[tauri::command]
pub async fn list_snippets<R: Runtime>(
    app: AppHandle<R>,
    site_id: Option<String>,
) -> AppResult<Vec<Snippet>> {
    read(&app, move |c| snippets::list(c, site_id.as_deref())).await
}

#[tauri::command]
pub async fn create_snippet<R: Runtime>(
    app: AppHandle<R>,
    input: SnippetInput,
) -> AppResult<Snippet> {
    write(&app, move |tx| snippets::create(tx, input)).await
}

#[tauri::command]
pub async fn update_snippet<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    input: SnippetInput,
) -> AppResult<Snippet> {
    write(&app, move |tx| snippets::update(tx, &id, input)).await
}

#[tauri::command]
pub async fn delete_snippet<R: Runtime>(app: AppHandle<R>, id: String) -> AppResult<()> {
    write(&app, move |tx| snippets::delete(tx, &id)).await
}

/// Saves the order of the given snippets (one scope at a time).
#[tauri::command]
pub async fn reorder_snippets<R: Runtime>(app: AppHandle<R>, ids: Vec<String>) -> AppResult<()> {
    write(&app, move |tx| snippets::reorder(tx, &ids)).await
}

/// Runs a command on the session's SSH connection (`{dir}` → shell-quoted `dir`); output, exit and failure stream on `events`.
#[tauri::command]
pub async fn run_command<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    command: String,
    dir: Option<String>,
    events: Channel<CommandEvent>,
) -> AppResult<RunStarted> {
    let session = session(&app, &session_id)?;
    let sink: CommandSink = Arc::new(move |event| {
        if let Err(error) = events.send(event) {
            log::debug!("a command event was not delivered: {error}");
        }
    });
    commands::run(&session, &command, dir.as_deref(), sink)
}

/// Stops a running command (no-op when it already ended).
#[tauri::command]
pub async fn cancel_command<R: Runtime>(app: AppHandle<R>, run_id: String) -> AppResult<()> {
    app.state::<SessionManager>().cancel_run(&run_id);
    Ok(())
}
