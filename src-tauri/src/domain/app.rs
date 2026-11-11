//! App-level status shown before anything else loads.

use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct AppStatus {
    pub version: String,
    /// `linux`, `macos` or `windows`.
    pub platform: String,
    /// Why the database could not be opened; sites cannot be saved then.
    pub data_error: Option<String>,
    /// Whether the OS keyring answered when Gem started. Without it passwords cannot be
    /// remembered; Gem asks for them when connecting.
    pub keyring_available: bool,
}
