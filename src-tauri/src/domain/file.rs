//! Remote files opened in the viewer/editor. Contents live in memory only.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Line endings found in a text file. Text sent to the UI is always normalised to `\n`;
/// saving converts back to `dominant`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "bindings/")]
pub enum LineEnding {
    Lf,
    Crlf,
    Cr,
}

/// Why a file is shown read-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "bindings/")]
pub enum ReadOnlyReason {
    /// Only part of the file is loaded.
    Partial,
    Binary,
    /// Decoding and re-encoding would not give back the same bytes.
    Encoding,
    /// The encoding cannot be written back (UTF-16).
    UnsupportedEncoding,
}

/// A text file opened from the server: its metadata and the first window of its contents.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct OpenedFile {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub mtime: Option<i64>,
    /// Decoded text, line endings normalised to `\n`. Empty for binary files.
    pub text: String,
    /// Byte offset where the next window starts (== size when complete).
    pub next_offset: u64,
    pub complete: bool,
    pub binary: bool,
    /// WHATWG encoding label: `utf-8`, `windows-1252`, `utf-16le`, `utf-16be`.
    pub encoding: String,
    pub bom: bool,
    pub line_ending: LineEnding,
    /// More than one kind of line ending was found; saving uses `line_ending` everywhere.
    pub mixed_line_endings: bool,
    pub read_only: Option<ReadOnlyReason>,
}

/// The next window of a partially loaded file. Appending windows never makes a file
/// editable: "Load all" reopens it whole (`open_file` with `whole = true`), which runs the
/// round-trip checks on the complete contents.
#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct FileChunk {
    /// Decoded with the encoding of the first window, line endings normalised to `\n`.
    pub text: String,
    pub next_offset: u64,
    pub complete: bool,
}

/// How to write a file back: the format it was opened with, and what it looked like then.
#[derive(Debug, Clone, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct SaveFile {
    /// Text with `\n` line endings, exactly as in the editor.
    pub text: String,
    pub encoding: String,
    pub bom: bool,
    pub line_ending: LineEnding,
    pub expected_size: u64,
    pub expected_mtime: Option<i64>,
    /// Skip the conflict check (the user chose "Overwrite").
    pub force: bool,
    /// Overwrite in place when the folder does not allow a temp file + rename. The first save
    /// sends `false`; on `unsupported` the UI asks the user, then retries with `true`.
    pub allow_in_place: bool,
}

#[derive(Debug, Clone, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct SavedFile {
    pub size: u64,
    pub mtime: Option<i64>,
    /// False when the server could not do temp-file-and-rename and the file was overwritten
    /// in place.
    pub atomic: bool,
}
