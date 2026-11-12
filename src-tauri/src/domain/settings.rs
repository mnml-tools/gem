//! App settings (one row per key in SQLite). Colour mode and accent are kept by the kit's
//! theme store in the webview, like the rest of the family.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::transfer::ConflictPolicy;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = "bindings/")]
pub struct Settings {
    /// Where downloads go; the OS Downloads folder when `None`.
    pub download_folder: Option<String>,
    /// First window loaded by the viewer, in KiB.
    pub preview_window_kib: u32,
    /// Ask before loading files larger than this, in MiB.
    pub confirm_above_mib: u32,
    /// Parallel transfers per connection tab.
    pub transfer_concurrency: u8,
    /// Seconds between keepalives (0 disables them).
    pub keepalive_secs: u32,
    pub show_hidden: bool,
    pub conflict_policy: ConflictPolicy,
    /// Look for a new version when Gem starts (installing it still asks).
    pub check_updates: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            download_folder: None,
            preview_window_kib: 2048,
            confirm_above_mib: 50,
            transfer_concurrency: 2,
            keepalive_secs: 30,
            show_hidden: false,
            conflict_policy: ConflictPolicy::Ask,
            check_updates: true,
        }
    }
}

impl Settings {
    pub const MAX_PREVIEW_WINDOW_KIB: u32 = 64 * 1024;
    pub const MAX_CONCURRENCY: u8 = 8;

    /// Clamps values coming from the UI to sane bounds.
    pub fn normalised(mut self) -> Self {
        self.preview_window_kib = self
            .preview_window_kib
            .clamp(64, Self::MAX_PREVIEW_WINDOW_KIB);
        self.confirm_above_mib = self.confirm_above_mib.clamp(1, 4096);
        self.transfer_concurrency = self.transfer_concurrency.clamp(1, Self::MAX_CONCURRENCY);
        self.keepalive_secs = if self.keepalive_secs == 0 {
            0
        } else {
            self.keepalive_secs.clamp(5, 600)
        };
        self
    }
}
