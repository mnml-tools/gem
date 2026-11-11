//! Connection tabs: connect, test, trust, disconnect, reconnect.

use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, Runtime};

use super::support::{database, event_sink, write};
use crate::domain::session::{ConnectTarget, HostKeyInfo, SessionEvent, SessionInfo, TestResult};
use crate::domain::site::SiteInput;
use crate::error::AppResult;
use crate::services::known_hosts;
use crate::services::sessions::SessionManager;

/// Opens a connection tab's session. `events` streams its log, state and transfers.
#[tauri::command]
pub async fn connect<R: Runtime>(
    app: AppHandle<R>,
    target: ConnectTarget,
    secret: Option<String>,
    remember: bool,
    events: Channel<SessionEvent>,
) -> AppResult<SessionInfo> {
    let db = database(&app)?;
    app.state::<SessionManager>()
        .connect(db, target, secret, remember, event_sink(events))
        .await
}

/// Connects, reads the home directory and disconnects.
#[tauri::command]
pub async fn test_connection<R: Runtime>(
    app: AppHandle<R>,
    site: SiteInput,
    secret: Option<String>,
    site_id: Option<String>,
) -> AppResult<TestResult> {
    let db = database(&app)?;
    app.state::<SessionManager>()
        .test_connection(db, site, secret, site_id)
        .await
}

/// Stores a host key or certificate the user accepted. Refuses to replace a different stored key.
#[tauri::command]
pub async fn trust_host_key<R: Runtime>(app: AppHandle<R>, key: HostKeyInfo) -> AppResult<()> {
    write(&app, move |tx| known_hosts::trust(tx, &key)).await
}

/// Closes the session, cancelling its transfers.
#[tauri::command]
pub async fn disconnect<R: Runtime>(app: AppHandle<R>, session_id: String) -> AppResult<()> {
    app.state::<SessionManager>().disconnect(&session_id).await
}

/// Reopens a dropped session with the same site and event channel.
#[tauri::command]
pub async fn reconnect<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
    secret: Option<String>,
) -> AppResult<SessionInfo> {
    let db = database(&app)?;
    app.state::<SessionManager>()
        .reconnect(db, &session_id, secret)
        .await
}
