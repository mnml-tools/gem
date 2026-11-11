//! Files dragged from the OS onto the main window.
//!
//! Local paths never come from the webview (plan §4.2 rule 6): the window's native drag & drop
//! events are handled here. While files hover, the UI gets `gem://drag` with the pointer in CSS
//! pixels (for the drop-target highlight); on drop, the paths are kept in [`DropRegistry`]
//! under a fresh id and the UI gets `gem://drop` with that id and the file names only. Uploading
//! them (`upload_dropped`) consumes the id. Unclaimed drops expire after a few minutes.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{DragDropEvent, Emitter, Manager, Runtime, Window};

use super::window::MAIN;
use crate::domain::transfer::{DragHover, DropPayload};
use crate::error::{AppError, AppResult};

pub const EVENT_DRAG: &str = "gem://drag";
pub const EVENT_DROP: &str = "gem://drop";

/// Drops kept at most; older ones are forgotten first.
const MAX_DROPS: usize = 16;
/// How long a drop can wait for `upload_dropped`.
const DROP_TTL: Duration = Duration::from_secs(5 * 60);

struct StoredDrop {
    id: String,
    at: Instant,
    paths: Vec<PathBuf>,
}

/// Paths of recent drops, by drop id (Tauri-managed state).
#[derive(Default)]
pub struct DropRegistry {
    drops: Mutex<VecDeque<StoredDrop>>,
}

impl DropRegistry {
    fn lock(&self) -> std::sync::MutexGuard<'_, VecDeque<StoredDrop>> {
        self.drops.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Keeps the paths of a drop; returns its id.
    pub fn store(&self, paths: Vec<PathBuf>) -> String {
        self.store_at(paths, Instant::now())
    }

    fn store_at(&self, paths: Vec<PathBuf>, now: Instant) -> String {
        let id = uuid::Uuid::now_v7().to_string();
        let mut drops = self.lock();
        drops.retain(|d| now.duration_since(d.at) < DROP_TTL);
        while drops.len() >= MAX_DROPS {
            drops.pop_front();
        }
        drops.push_back(StoredDrop {
            id: id.clone(),
            at: now,
            paths,
        });
        id
    }

    /// Takes the paths of a drop (once).
    pub fn take(&self, id: &str) -> AppResult<Vec<PathBuf>> {
        self.take_at(id, Instant::now())
    }

    fn take_at(&self, id: &str, now: Instant) -> AppResult<Vec<PathBuf>> {
        let mut drops = self.lock();
        drops.retain(|d| now.duration_since(d.at) < DROP_TTL);
        let index = drops
            .iter()
            .position(|d| d.id == id)
            .ok_or_else(|| AppError::not_found("dropped files (drop them again)"))?;
        Ok(drops.remove(index).map(|d| d.paths).unwrap_or_default())
    }
}

fn names(paths: &[PathBuf]) -> Vec<String> {
    paths
        .iter()
        .map(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string())
        })
        .collect()
}

/// Handles `WindowEvent::DragDrop` for the main window.
pub fn handle<R: Runtime>(window: &Window<R>, event: &DragDropEvent) {
    if window.label() != MAIN {
        return;
    }
    let scale = window.scale_factor().unwrap_or(1.0);
    let result = match event {
        DragDropEvent::Enter { position, .. } | DragDropEvent::Over { position } => {
            let at = position.to_logical::<f64>(scale);
            window.emit_to(MAIN, EVENT_DRAG, DragHover::Over { x: at.x, y: at.y })
        }
        DragDropEvent::Drop { paths, position } if !paths.is_empty() => {
            let at = position.to_logical::<f64>(scale);
            let drop_id = window.state::<DropRegistry>().store(paths.clone());
            window
                .emit_to(MAIN, EVENT_DRAG, DragHover::Leave)
                .and_then(|()| {
                    window.emit_to(
                        MAIN,
                        EVENT_DROP,
                        DropPayload {
                            drop_id,
                            names: names(paths),
                            x: at.x,
                            y: at.y,
                        },
                    )
                })
        }
        DragDropEvent::Drop { .. } | DragDropEvent::Leave => {
            window.emit_to(MAIN, EVENT_DRAG, DragHover::Leave)
        }
        _ => Ok(()),
    };
    if let Err(error) = result {
        log::warn!("could not forward a drag and drop event: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_are_taken_once() {
        let registry = DropRegistry::default();
        let id = registry.store(vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]);
        assert_eq!(registry.take(&id).unwrap().len(), 2);
        assert_eq!(
            registry.take(&id).unwrap_err().code(),
            crate::error::ErrorCode::NotFound
        );
    }

    #[test]
    fn drops_expire_and_are_bounded() {
        let registry = DropRegistry::default();
        let start = Instant::now();
        let old = registry.store_at(vec![PathBuf::from("/a")], start);
        assert!(registry.take_at(&old, start + DROP_TTL).is_err());
        let ids: Vec<String> = (0..MAX_DROPS + 2)
            .map(|i| registry.store_at(vec![PathBuf::from(format!("/{i}"))], start))
            .collect();
        assert!(registry.take_at(&ids[0], start).is_err(), "oldest evicted");
        assert!(registry.take_at(&ids[MAX_DROPS + 1], start).is_ok());
    }

    #[test]
    fn only_names_reach_the_ui() {
        assert_eq!(
            names(&[PathBuf::from("/home/me/site"), PathBuf::from("/tmp/a.txt")]),
            ["site", "a.txt"]
        );
    }
}
