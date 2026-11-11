//! Connection log lines, produced by the remote layer and streamed to the bottom panel.

use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "bindings/")]
pub enum LogKind {
    Status,
    Command,
    Response,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct LogLine {
    pub seq: u64,
    /// Milliseconds since the Unix epoch.
    pub ts: i64,
    pub kind: LogKind,
    pub text: String,
}
