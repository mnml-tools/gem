//! Remote commands over SSH (SFTP sites only) and the snippets that store them.
//!
//! Commands run without a terminal (an SSH `exec` channel, no PTY): their output streams to a
//! viewer tab through one `tauri::ipc::Channel` per run. Interactive programs (vim, htop) are
//! out of scope (ADR 0014).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::ErrorPayload;

/// A saved command. `site_id = None` is a global snippet, offered on every SFTP site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct Snippet {
    pub id: String,
    pub site_id: Option<String>,
    pub name: String,
    /// The shell command. `{dir}` is replaced by the file list's folder, shell-quoted.
    pub command: String,
    /// Ask before running (for commands that change things, e.g. `docker restart core`).
    pub confirm: bool,
    pub sort_order: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// What the snippet dialog sends to create or update a snippet.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct SnippetInput {
    pub site_id: Option<String>,
    pub name: String,
    pub command: String,
    pub confirm: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "bindings/")]
pub enum CommandStream {
    Stdout,
    Stderr,
}

/// How a remote command ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct CommandExit {
    /// The exit status, when the server reported one.
    pub code: Option<u32>,
    /// The signal that ended it (e.g. `TERM` after Stop), when the server reported one.
    pub signal: Option<String>,
    /// Stopped by the user.
    pub cancelled: bool,
}

/// Everything a running command pushes to its viewer tab.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
#[ts(export, export_to = "bindings/")]
pub enum CommandEvent {
    /// Decoded output (UTF-8, lossy) with ANSI escape sequences removed, in arrival order.
    Output { stream: CommandStream, text: String },
    /// The command finished (always the last event of a run that started).
    Exit { exit: CommandExit, duration_ms: u64 },
    /// The command could not run or the connection broke (always the last event).
    Failed { error: ErrorPayload },
}

/// Returned by `run_command` before any output arrives.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct RunStarted {
    pub run_id: String,
    /// The command as sent to the server, after `{dir}` expansion.
    pub command: String,
}
