//! The in-memory viewer and editor: open a window (or the whole file), read more, save back.
//!
//! Contents never touch the local disk. Saving is guarded twice: the file must look exactly
//! as it did when opened (size and mtime, unless the user chose "Overwrite"), and it is written
//! with `write_atomic` (temp file, original permissions, rename). Saving through a symlink
//! writes the link's target, so the link survives.

use crate::domain::entry::EntryKind;
use crate::domain::file::{FileChunk, OpenedFile, SaveFile, SavedFile};
use crate::error::{AppError, AppResult};
use crate::remote::RemoteFs;
use crate::remote::path::{file_name, normalize, parent};
use crate::text;

/// Largest file loaded whole (the UI asks before loading files above its own threshold).
pub const MAX_WHOLE_FILE: u64 = 512 * 1024 * 1024;

const CHANGED: &str = "The file changed on the server. Reopen it.";

/// Symlink hops followed when saving.
const MAX_LINKS: usize = 8;

/// Reads the first window (`window_kib`) or, with `whole`, the entire file.
pub async fn open_file(
    fs: &dyn RemoteFs,
    path: &str,
    whole: bool,
    window_kib: u32,
) -> AppResult<OpenedFile> {
    let path = normalize("/", path);
    let entry = fs.stat(&path).await?;
    if entry.is_dir_like() {
        return Err(AppError::validation(format!(
            "{} is a folder",
            file_name(&path)
        )));
    }
    match entry.kind {
        EntryKind::Other => {
            return Err(AppError::validation(format!(
                "{} is not a regular file and cannot be opened",
                file_name(&path)
            )));
        }
        EntryKind::File | EntryKind::Symlink | EntryKind::Dir => {}
    }
    let window = u64::from(window_kib.max(1)) * 1024;
    let (bytes, complete) = match (whole, entry.size) {
        (true, Some(size)) => {
            if size > MAX_WHOLE_FILE {
                return Err(AppError::validation(format!(
                    "the file is too large to load whole (more than {} MiB)",
                    MAX_WHOLE_FILE / (1024 * 1024)
                )));
            }
            (fs.read_range(&path, 0, size).await?, true)
        }
        (true, None) => read_to_end(fs, &path, window).await?,
        (false, size) => {
            let bytes = fs.read_range(&path, 0, window).await?;
            let read = bytes.len() as u64;
            let complete = size.map_or(read < window, |size| read >= size || read < window);
            (bytes, complete)
        }
    };
    let opened = text::open(&bytes, complete);
    Ok(OpenedFile {
        name: file_name(&path).to_owned(),
        size: entry.size.unwrap_or(bytes.len() as u64),
        mtime: entry.mtime,
        text: opened.text,
        next_offset: opened.consumed as u64,
        complete,
        binary: opened.binary,
        encoding: opened.encoding.to_owned(),
        bom: opened.bom,
        line_ending: opened.line_ending,
        mixed_line_endings: opened.mixed_line_endings,
        read_only: opened.read_only,
        path,
    })
}

/// Reads a file of unknown size until a short read.
async fn read_to_end(fs: &dyn RemoteFs, path: &str, window: u64) -> AppResult<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    loop {
        let chunk = fs.read_range(path, bytes.len() as u64, window).await?;
        let short = (chunk.len() as u64) < window;
        bytes.extend(chunk);
        if bytes.len() as u64 > MAX_WHOLE_FILE {
            return Err(AppError::validation("the file is too large to load whole"));
        }
        if short {
            return Ok((bytes, true));
        }
    }
}

/// Reads the window at `offset` (or everything up to the end), decoded with the encoding of
/// the first window. `Conflict` when the file changed since it was opened.
#[allow(clippy::too_many_arguments)]
pub async fn read_more(
    fs: &dyn RemoteFs,
    path: &str,
    offset: u64,
    encoding: &str,
    expected_size: u64,
    expected_mtime: Option<i64>,
    to_end: bool,
    window_kib: u32,
) -> AppResult<FileChunk> {
    let encoding = text::encoding_for_label(encoding)?;
    let path = normalize("/", path);
    let entry = fs.stat(&path).await?;
    if entry.size != Some(expected_size) || entry.mtime != expected_mtime {
        return Err(AppError::Conflict(CHANGED.into()));
    }
    if offset > expected_size {
        return Err(AppError::validation("offset past the end of the file"));
    }
    let remaining = expected_size - offset;
    let len = if to_end {
        if remaining > MAX_WHOLE_FILE {
            return Err(AppError::validation(
                "the rest of the file is too large to load",
            ));
        }
        remaining
    } else {
        remaining.min(u64::from(window_kib.max(1)) * 1024)
    };
    let bytes = fs.read_range(&path, offset, len).await?;
    let complete = bytes.len() as u64 >= remaining || (bytes.len() as u64) < len;
    let decoded = text::decode_window(encoding, &bytes, complete);
    Ok(FileChunk {
        text: text::lines::normalise(&decoded.raw),
        next_offset: offset + decoded.consumed as u64,
        complete,
    })
}

/// Follows symlinks at `path` so a save writes the target and keeps the link.
async fn resolve_links(fs: &dyn RemoteFs, path: &str) -> AppResult<String> {
    if !fs.capabilities().symlinks {
        return Ok(path.to_owned());
    }
    let mut current = path.to_owned();
    for _ in 0..MAX_LINKS {
        let dir = parent(&current);
        let Ok(entries) = fs.list(&dir).await else {
            return Ok(current);
        };
        let link = entries
            .into_iter()
            .find(|e| e.name == file_name(&current))
            .filter(|e| e.kind == EntryKind::Symlink)
            .and_then(|e| e.link_target);
        match link {
            Some(target) => current = normalize(&dir, &target),
            None => return Ok(current),
        }
    }
    Err(AppError::validation("too many levels of symbolic links"))
}

/// Writes the editor's text back: conflict check (unless `force`), re-encoding with the
/// original encoding, BOM and line ending, then an atomic write keeping the permissions.
/// `Unsupported` from the remote layer means the folder only allows an in-place overwrite;
/// the UI asks and retries with `allow_in_place`.
pub async fn save_file(fs: &dyn RemoteFs, path: &str, request: SaveFile) -> AppResult<SavedFile> {
    // Only UTF-8 and windows-1252 can be written; UTF-16, binary or partial files never get
    // here with a valid request.
    let bytes = text::encode_for_save(
        &request.text,
        &request.encoding,
        request.bom,
        request.line_ending,
    )?;
    let target = resolve_links(fs, &normalize("/", path)).await?;
    let current = match fs.stat(&target).await {
        Ok(entry) => Some(entry),
        Err(AppError::NotFound { .. }) => None,
        Err(error) => return Err(error),
    };
    if !request.force {
        match &current {
            None => {
                return Err(AppError::Conflict(
                    "The file was deleted on the server since it was opened.".into(),
                ));
            }
            Some(entry)
                if entry.size != Some(request.expected_size)
                    || entry.mtime != request.expected_mtime =>
            {
                return Err(AppError::Conflict(CHANGED.into()));
            }
            Some(_) => {}
        }
    }
    if current.as_ref().is_some_and(|e| e.kind == EntryKind::Dir) {
        return Err(AppError::validation("a folder now has this name"));
    }
    let mode = current.and_then(|e| e.mode);
    let outcome = fs
        .write_atomic(&target, &bytes, mode, request.allow_in_place)
        .await?;
    let saved = fs.stat(&target).await?;
    Ok(SavedFile {
        size: saved.size.unwrap_or(bytes.len() as u64),
        mtime: saved.mtime,
        atomic: outcome.atomic,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::domain::file::{LineEnding, ReadOnlyReason};
    use crate::error::ErrorCode;
    use crate::remote::memory::MemoryServer;

    fn setup() -> (Arc<MemoryServer>, Arc<dyn RemoteFs>) {
        let server = MemoryServer::new();
        let fs = crate::remote::Connection::fs(&*server.connect());
        (server, fs)
    }

    fn request(opened: &OpenedFile, text: &str) -> SaveFile {
        SaveFile {
            text: text.into(),
            encoding: opened.encoding.clone(),
            bom: opened.bom,
            line_ending: opened.line_ending,
            expected_size: opened.size,
            expected_mtime: opened.mtime,
            force: false,
            allow_in_place: false,
        }
    }

    #[tokio::test]
    async fn opens_saves_and_keeps_format_and_permissions() {
        let (server, fs) = setup();
        let path = "/home/user/legacy.ini";
        server.add_file(path, b"[db]\r\nname=caf\xE9\r\n");
        server.set_mode(path, 0o640);
        let opened = open_file(&*fs, path, false, 2048).await.unwrap();
        assert_eq!(opened.read_only, None);
        assert!(opened.complete);
        assert_eq!(opened.next_offset, opened.size);
        assert_eq!(opened.encoding, "windows-1252");
        assert_eq!(opened.line_ending, LineEnding::Crlf);
        assert_eq!(opened.text, "[db]\nname=café\n");
        let saved = save_file(&*fs, path, request(&opened, "[db]\nname=caféx\n"))
            .await
            .unwrap();
        assert!(saved.atomic);
        assert_eq!(server.file(path).unwrap(), b"[db]\r\nname=caf\xE9x\r\n");
        assert_eq!(server.mode(path), Some(0o640));
        assert_eq!(saved.size, 18);
        assert!(!server.paths().iter().any(|p| p.contains(".gem-")));
    }

    #[tokio::test]
    async fn refuses_folders_and_reports_binary_files() {
        let (server, fs) = setup();
        assert_eq!(
            open_file(&*fs, "/home", false, 64)
                .await
                .unwrap_err()
                .code(),
            ErrorCode::Validation
        );
        server.add_file("/home/user/a.png", b"\x89PNG\0\0");
        let opened = open_file(&*fs, "/home/user/a.png", false, 64)
            .await
            .unwrap();
        assert!(opened.binary);
        assert_eq!(opened.read_only, Some(ReadOnlyReason::Binary));
    }

    #[tokio::test]
    async fn large_files_open_partially_and_load_more_in_windows() {
        let (server, fs) = setup();
        let line = "log line with ünïcödé and an emoji 😀\r\n";
        let content = line.repeat(10_000); // ~420 KiB
        let path = "/home/user/app.log";
        server.add_file(path, content.as_bytes());
        let opened = open_file(&*fs, path, false, 64).await.unwrap();
        assert!(!opened.complete);
        assert_eq!(opened.read_only, Some(ReadOnlyReason::Partial));
        let mut text = opened.text.clone();
        let mut offset = opened.next_offset;
        let mut complete = false;
        while !complete {
            let chunk = read_more(
                &*fs,
                path,
                offset,
                &opened.encoding,
                opened.size,
                opened.mtime,
                false,
                64,
            )
            .await
            .unwrap();
            text.push_str(&chunk.text);
            offset = chunk.next_offset;
            complete = chunk.complete;
        }
        assert_eq!(offset, opened.size);
        assert_eq!(text, content.replace("\r\n", "\n"));
        // "Load all" from the first window.
        let rest = read_more(
            &*fs,
            path,
            opened.next_offset,
            &opened.encoding,
            opened.size,
            opened.mtime,
            true,
            64,
        )
        .await
        .unwrap();
        assert!(rest.complete);
        assert_eq!(opened.text + &rest.text, content.replace("\r\n", "\n"));
        // Reopened whole, it is editable.
        let whole = open_file(&*fs, path, true, 64).await.unwrap();
        assert_eq!(whole.read_only, None);
        assert!(whole.complete);
    }

    #[tokio::test]
    async fn read_more_detects_changes() {
        let (server, fs) = setup();
        let path = "/home/user/app.log";
        server.add_file(path, &vec![b'a'; 200 * 1024]);
        let opened = open_file(&*fs, path, false, 64).await.unwrap();
        server.replace_file(path, &vec![b'b'; 200 * 1024]);
        let error = read_more(
            &*fs,
            path,
            opened.next_offset,
            "utf-8",
            opened.size,
            opened.mtime,
            false,
            64,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), ErrorCode::Conflict);
        assert!(error.to_string().contains("Reopen"));
    }

    #[tokio::test]
    async fn conflicts_unless_forced() {
        let (server, fs) = setup();
        let path = "/home/user/config.yaml";
        server.add_file(path, b"a: 1\n");
        let opened = open_file(&*fs, path, false, 64).await.unwrap();
        server.replace_file(path, b"a: 2\n");
        let error = save_file(&*fs, path, request(&opened, "a: 3\n"))
            .await
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::Conflict);
        assert_eq!(server.file(path).unwrap(), b"a: 2\n");
        let forced = SaveFile {
            force: true,
            ..request(&opened, "a: 3\n")
        };
        save_file(&*fs, path, forced).await.unwrap();
        assert_eq!(server.file(path).unwrap(), b"a: 3\n");
        // Deleted on the server meanwhile: a conflict too.
        let opened = open_file(&*fs, path, false, 64).await.unwrap();
        fs.remove_file(path).await.unwrap();
        assert_eq!(
            save_file(&*fs, path, request(&opened, "x"))
                .await
                .unwrap_err()
                .code(),
            ErrorCode::Conflict
        );
    }

    #[tokio::test]
    async fn encoding_errors_never_write() {
        let (server, fs) = setup();
        let path = "/home/user/latin.txt";
        server.add_file(path, b"caf\xE9\n");
        let opened = open_file(&*fs, path, false, 64).await.unwrap();
        let error = save_file(&*fs, path, request(&opened, "café ✓\n"))
            .await
            .unwrap_err();
        assert_eq!(error.code(), ErrorCode::Validation);
        assert!(error.to_string().contains("line 1"));
        assert_eq!(server.file(path).unwrap(), b"caf\xE9\n");
        let utf16 = SaveFile {
            encoding: "utf-16le".into(),
            ..request(&opened, "x")
        };
        assert_eq!(
            save_file(&*fs, path, utf16).await.unwrap_err().code(),
            ErrorCode::Validation
        );
    }

    #[tokio::test]
    async fn mixed_line_endings_save_with_the_dominant_one() {
        let (server, fs) = setup();
        let path = "/home/user/mixed.txt";
        server.add_file(path, b"a\r\nb\r\nc\nd");
        let opened = open_file(&*fs, path, false, 64).await.unwrap();
        assert!(opened.mixed_line_endings);
        assert_eq!(opened.read_only, None);
        save_file(&*fs, path, request(&opened, &opened.text))
            .await
            .unwrap();
        assert_eq!(server.file(path).unwrap(), b"a\r\nb\r\nc\r\nd");
    }

    #[tokio::test]
    async fn read_only_folder_needs_permission_for_an_in_place_save() {
        let (server, fs) = setup();
        server.add_dir("/srv");
        server.add_file("/srv/app.conf", b"x=1\n");
        server.set_mode("/srv", 0o555);
        let opened = open_file(&*fs, "/srv/app.conf", false, 64).await.unwrap();
        let first = save_file(&*fs, "/srv/app.conf", request(&opened, "x=2\n")).await;
        assert_eq!(first.unwrap_err().code(), ErrorCode::Unsupported);
        assert_eq!(server.file("/srv/app.conf").unwrap(), b"x=1\n");
        let again = SaveFile {
            allow_in_place: true,
            ..request(&opened, "x=2\n")
        };
        let saved = save_file(&*fs, "/srv/app.conf", again).await.unwrap();
        assert!(!saved.atomic);
        assert_eq!(server.file("/srv/app.conf").unwrap(), b"x=2\n");
    }

    #[tokio::test]
    async fn saving_through_a_symlink_keeps_the_link() {
        let (server, fs) = setup();
        server.add_dir("/etc");
        server.add_dir("/etc/nginx");
        server.add_file("/etc/nginx/available", b"server {}\n");
        server.add_link("/etc/nginx/enabled", "available");
        let opened = open_file(&*fs, "/etc/nginx/enabled", false, 64)
            .await
            .unwrap();
        assert_eq!(opened.text, "server {}\n");
        save_file(
            &*fs,
            "/etc/nginx/enabled",
            request(&opened, "server { listen 80; }\n"),
        )
        .await
        .unwrap();
        assert_eq!(
            server.file("/etc/nginx/available").unwrap(),
            b"server { listen 80; }\n"
        );
        let listing = fs.list("/etc/nginx").await.unwrap();
        let link = listing.iter().find(|e| e.name == "enabled").unwrap();
        assert_eq!(link.kind, EntryKind::Symlink);
    }

    #[tokio::test]
    async fn a_failed_write_keeps_the_original() {
        let (server, fs) = setup();
        let path = "/home/user/f.txt";
        server.add_file(path, b"old\n");
        let opened = open_file(&*fs, path, false, 64).await.unwrap();
        server.fail_next_writes(1);
        assert!(
            save_file(&*fs, path, request(&opened, "new\n"))
                .await
                .is_err()
        );
        assert_eq!(server.file(path).unwrap(), b"old\n");
        assert!(!server.paths().iter().any(|p| p.contains(".gem-")));
    }
}
