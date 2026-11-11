//! Upload and download jobs.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "bindings/")]
pub enum TransferDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "bindings/")]
pub enum TransferState {
    Queued,
    Running,
    /// The target exists and the policy is `ask`: waiting for `resolve_conflict`.
    Conflict,
    Done,
    Failed,
    Cancelled,
    Skipped,
}

impl TransferState {
    pub fn is_finished(self) -> bool {
        matches!(
            self,
            Self::Done | Self::Failed | Self::Cancelled | Self::Skipped
        )
    }
}

/// What to do when the target of a transfer already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "bindings/")]
pub enum ConflictPolicy {
    Ask,
    Overwrite,
    Skip,
    /// Keep both: the new file gets a ` (1)` style suffix.
    Rename,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct Transfer {
    pub id: String,
    pub session_id: String,
    pub direction: TransferDirection,
    pub name: String,
    pub local_path: String,
    pub remote_path: String,
    pub size: Option<u64>,
    pub done: u64,
    pub state: TransferState,
    pub error: Option<String>,
    /// Milliseconds since the Unix epoch.
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

/// Emitted as `gem://drop` when files are dropped on the window from the OS. Paths stay in
/// Rust; the UI only sees names and uses `drop_id` to upload them (`upload_dropped`).
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct DropPayload {
    pub drop_id: String,
    pub names: Vec<String>,
    /// Drop position in CSS pixels, relative to the webview.
    pub x: f64,
    pub y: f64,
}

/// Emitted as `gem://drag` while files from the OS are dragged over the window.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export, export_to = "bindings/")]
pub enum DragHover {
    Over { x: f64, y: f64 },
    Leave,
}
