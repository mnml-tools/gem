//! Domain types shared by the remote layer, services and the UI (through ts-rs bindings).
//!
//! These types are the IPC contract: changing a field means regenerating the bindings
//! (`pnpm bindings`) and updating `src/lib/ipc/api.ts`.

pub mod app;
pub mod command;
pub mod entry;
pub mod file;
pub mod log;
pub mod session;
pub mod settings;
pub mod site;
pub mod transfer;

/// Milliseconds since the Unix epoch, for timestamps shown in the UI.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
