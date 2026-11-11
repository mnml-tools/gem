//! Remote directory entries.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "bindings/")]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    /// Devices, sockets, pipes: listed but not opened.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct Entry {
    pub name: String,
    /// Absolute remote path (POSIX separators).
    pub path: String,
    pub kind: EntryKind,
    pub size: Option<u64>,
    /// Modification time, seconds since the Unix epoch.
    pub mtime: Option<i64>,
    /// Permission bits (`0o7777` mask), when the protocol reports them.
    pub mode: Option<u32>,
    pub owner: Option<String>,
    pub group: Option<String>,
    /// Symlink target as stored on the server.
    pub link_target: Option<String>,
    /// For symlinks: what the target is (`dir` lets the UI navigate into it), if resolved.
    pub target_kind: Option<EntryKind>,
}

impl Entry {
    /// Whether opening this entry navigates (a directory, or a link to one).
    pub fn is_dir_like(&self) -> bool {
        self.kind == EntryKind::Dir
            || (self.kind == EntryKind::Symlink && self.target_kind == Some(EntryKind::Dir))
    }
}

/// The contents of a remote directory, with its canonical path.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct Listing {
    pub path: String,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct DeleteFailure {
    pub path: String,
    pub message: String,
}

/// Result of deleting a selection (directories recursively).
#[derive(Debug, Clone, Default, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct DeleteReport {
    /// Files and directories removed, counting everything inside removed directories.
    pub deleted: u32,
    pub failed: Vec<DeleteFailure>,
}
