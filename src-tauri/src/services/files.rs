//! Browsing and file operations on a session's primary file system.
//!
//! Names typed by the user are validated here ([`path::is_valid_name`], no `.`/`..`
//! components), so a name like `../x` can never create or rename something outside the folder
//! the user is looking at. Deleting never follows a symlink into a directory: the link itself
//! is removed.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use crate::domain::entry::{DeleteFailure, DeleteReport, Entry, EntryKind, Listing};
use crate::error::{AppError, AppResult};
use crate::remote::RemoteFs;
use crate::remote::path::{self, file_name, join, normalize, parent};

/// Highest mode `chmod` accepts (permission bits plus setuid, setgid and sticky).
pub const MAX_MODE: u32 = 0o7777;

/// Checks a path built from a name the user typed and returns it normalised.
fn new_path(raw: &str) -> AppResult<String> {
    if !raw.starts_with('/') {
        return Err(AppError::validation("the path must be absolute"));
    }
    let trimmed = raw.trim_end_matches('/');
    let name = trimmed.rsplit('/').next().unwrap_or_default();
    if !path::is_valid_name(name) || trimmed.split('/').any(|c| c == "." || c == "..") {
        return Err(AppError::validation(format!(
            "“{name}” is not a valid name"
        )));
    }
    Ok(normalize("/", trimmed))
}

/// Lists a directory; the returned path is canonical (symlinks and `..` resolved).
pub async fn list(fs: &dyn RemoteFs, path: &str) -> AppResult<Listing> {
    let canonical = fs.canonicalize(path).await?;
    let entries = fs.list(&canonical).await?;
    Ok(Listing {
        path: canonical,
        entries,
    })
}

pub async fn stat(fs: &dyn RemoteFs, path: &str) -> AppResult<Entry> {
    fs.stat(&normalize("/", path)).await
}

/// The entry for `path` as its folder lists it (a symlink stays a symlink), falling back to
/// `stat`.
async fn entry_of(fs: &dyn RemoteFs, path: &str) -> AppResult<Entry> {
    if let Ok(entries) = fs.list(&parent(path)).await
        && let Some(entry) = entries.into_iter().find(|e| e.name == file_name(path))
    {
        return Ok(entry);
    }
    fs.stat(path).await
}

pub async fn make_dir(fs: &dyn RemoteFs, path: &str) -> AppResult<Entry> {
    let path = new_path(path)?;
    fs.mkdir(&path).await?;
    fs.stat(&path).await
}

/// Creates an empty file; `Conflict` when the name exists.
pub async fn make_file(fs: &dyn RemoteFs, path: &str) -> AppResult<Entry> {
    let path = new_path(path)?;
    fs.create_file(&path).await?;
    fs.stat(&path).await
}

/// Renames (or moves) `from` to `to`; never replaces an existing target.
pub async fn rename(fs: &dyn RemoteFs, from: &str, to: &str) -> AppResult<Entry> {
    let from = normalize("/", from);
    let to = new_path(to)?;
    if from == "/" {
        return Err(AppError::validation("the root folder cannot be renamed"));
    }
    if from == to {
        return entry_of(fs, &to).await;
    }
    match fs.stat(&to).await {
        Ok(_) => {
            return Err(AppError::Conflict(format!(
                "{} already exists",
                file_name(&to)
            )));
        }
        Err(AppError::NotFound { .. }) => {}
        Err(error) => return Err(error),
    }
    fs.rename(&from, &to).await?;
    entry_of(fs, &to).await
}

/// Deletes files and folders (recursively). Never deletes `/`, the home folder or a folder
/// containing it. Failures are reported per path; everything else is still deleted.
pub async fn delete_paths(fs: &dyn RemoteFs, home: &str, paths: &[String]) -> DeleteReport {
    let mut report = DeleteReport::default();
    let mut listings: HashMap<String, Vec<Entry>> = HashMap::new();
    for raw in paths {
        if !raw.starts_with('/') {
            fail(&mut report, raw, "the path must be absolute");
            continue;
        }
        let target = normalize("/", raw);
        if target == "/" || target == home || home.starts_with(&format!("{target}/")) {
            fail(
                &mut report,
                &target,
                "Gem does not delete the root folder or your home folder",
            );
            continue;
        }
        // The folder's listing says what the path is without following symlinks.
        let dir = parent(&target);
        if !listings.contains_key(&dir) {
            match fs.list(&dir).await {
                Ok(entries) => {
                    listings.insert(dir.clone(), entries);
                }
                Err(error) => {
                    fail(&mut report, &target, &error.to_string());
                    continue;
                }
            }
        }
        let kind = listings
            .get(&dir)
            .and_then(|entries| entries.iter().find(|e| e.name == file_name(&target)))
            .map(|e| e.kind);
        match kind {
            None => fail(&mut report, &target, "not found"),
            Some(EntryKind::Dir) => {
                delete_tree(fs, target, &mut report).await;
            }
            Some(_) => {
                remove_one(fs, &target, &mut report).await;
            }
        }
    }
    report
}

fn fail(report: &mut DeleteReport, path: &str, message: &str) {
    report.failed.push(DeleteFailure {
        path: path.to_owned(),
        message: message.to_owned(),
    });
}

async fn remove_one(fs: &dyn RemoteFs, path: &str, report: &mut DeleteReport) -> bool {
    match fs.remove_file(path).await {
        Ok(()) => {
            report.deleted += 1;
            true
        }
        Err(error) => {
            fail(report, path, &error.to_string());
            false
        }
    }
}

/// Deletes a folder's contents, then the folder. Returns whether it is gone. A child that
/// cannot be deleted is reported and keeps its folder (which then cannot be empty).
fn delete_tree<'a>(
    fs: &'a dyn RemoteFs,
    dir: String,
    report: &'a mut DeleteReport,
) -> Pin<Box<dyn Future<Output = bool> + Send + 'a>> {
    Box::pin(async move {
        let entries = match fs.list(&dir).await {
            Ok(entries) => entries,
            Err(error) => {
                fail(report, &dir, &error.to_string());
                return false;
            }
        };
        let mut complete = true;
        for entry in entries {
            let child = join(&dir, &entry.name);
            let removed = if entry.kind == EntryKind::Dir {
                delete_tree(fs, child, report).await
            } else {
                // Files, symlinks (even to folders) and special files: remove the entry.
                remove_one(fs, &child, report).await
            };
            complete &= removed;
        }
        if !complete {
            return false;
        }
        match fs.remove_dir(&dir).await {
            Ok(()) => {
                report.deleted += 1;
                true
            }
            Err(error) => {
                fail(report, &dir, &error.to_string());
                false
            }
        }
    })
}

/// Changes permission bits (`mode` ≤ 0o7777).
pub async fn chmod(fs: &dyn RemoteFs, path: &str, mode: u32) -> AppResult<Entry> {
    if mode > MAX_MODE {
        return Err(AppError::validation(
            "permissions must be at most 7777 (octal)",
        ));
    }
    if !fs.capabilities().chmod {
        return Err(AppError::Unsupported(
            "this server does not support changing permissions".into(),
        ));
    }
    let path = normalize("/", path);
    fs.chmod(&path, mode).await?;
    entry_of(fs, &path).await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::error::ErrorCode;
    use crate::remote::memory::MemoryServer;

    fn setup() -> (Arc<MemoryServer>, Arc<dyn RemoteFs>) {
        let server = MemoryServer::new();
        let fs = crate::remote::Connection::fs(&*server.connect());
        (server, fs)
    }

    #[tokio::test]
    async fn lists_with_the_canonical_path() {
        let (server, fs) = setup();
        server.add_dir("/var");
        server.add_dir("/var/www");
        server.add_file("/var/www/index.php", b"<?php");
        server.add_link("/home/user/www", "/var/www");
        let listing = list(&*fs, "/home/user/www/../www").await.unwrap();
        assert_eq!(listing.path, "/var/www");
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(
            list(&*fs, "/nope").await.unwrap_err().code(),
            ErrorCode::NotFound
        );
    }

    #[tokio::test]
    async fn creates_and_renames_with_validated_names() {
        let (server, fs) = setup();
        let dir = make_dir(&*fs, "/home/user/app").await.unwrap();
        assert_eq!(dir.kind, EntryKind::Dir);
        let file = make_file(&*fs, "/home/user/app/.env").await.unwrap();
        assert_eq!((file.name.as_str(), file.size), (".env", Some(0)));
        assert_eq!(
            make_file(&*fs, "/home/user/app/.env")
                .await
                .unwrap_err()
                .code(),
            ErrorCode::Conflict
        );
        for bad in [
            "/home/user/app/..",
            "/home/user/app/../x",
            "relative",
            "/home/user/./x",
        ] {
            assert_eq!(
                make_dir(&*fs, bad).await.unwrap_err().code(),
                ErrorCode::Validation,
                "{bad}"
            );
        }
        let renamed = rename(&*fs, "/home/user/app/.env", "/home/user/app/.env.local")
            .await
            .unwrap();
        assert_eq!(renamed.path, "/home/user/app/.env.local");
        assert!(!server.exists("/home/user/app/.env"));
        make_file(&*fs, "/home/user/app/other").await.unwrap();
        assert_eq!(
            rename(&*fs, "/home/user/app/other", "/home/user/app/.env.local")
                .await
                .unwrap_err()
                .code(),
            ErrorCode::Conflict
        );
    }

    #[tokio::test]
    async fn renaming_a_symlink_returns_the_link() {
        let (server, fs) = setup();
        server.add_file("/home/user/real", b"x");
        server.add_link("/home/user/link", "real");
        let entry = rename(&*fs, "/home/user/link", "/home/user/link2")
            .await
            .unwrap();
        assert_eq!(entry.kind, EntryKind::Symlink);
        assert_eq!(entry.link_target.as_deref(), Some("real"));
    }

    #[tokio::test]
    async fn deletes_recursively_without_following_links() {
        let (server, fs) = setup();
        server.add_dir("/data");
        server.add_file("/data/keep.txt", b"keep");
        server.add_dir("/home/user/site");
        server.add_dir("/home/user/site/css");
        server.add_file("/home/user/site/css/a.css", b"a");
        server.add_file("/home/user/site/index.html", b"i");
        server.add_link("/home/user/site/data", "/data");
        server.add_link("/home/user/datalink", "/data");
        server.add_file("/home/user/x.txt", b"x");
        let report = delete_paths(
            &*fs,
            "/home/user",
            &[
                "/home/user/site".into(),
                "/home/user/datalink".into(),
                "/home/user/x.txt".into(),
            ],
        )
        .await;
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        // site, css, a.css, index.html, data link, datalink, x.txt
        assert_eq!(report.deleted, 7);
        assert_eq!(server.file("/data/keep.txt").unwrap(), b"keep");
        assert!(!server.exists("/home/user/site"));
        assert!(!server.exists("/home/user/datalink"));
    }

    #[tokio::test]
    async fn refuses_root_and_home_and_reports_failures() {
        let (server, fs) = setup();
        server.add_dir("/ro");
        server.add_file("/ro/locked", b"x");
        server.add_dir("/ro/sub");
        server.set_mode("/ro", 0o555);
        server.add_dir("/home/user/tree");
        server.add_dir("/home/user/tree/ro");
        server.add_file("/home/user/tree/ro/f", b"f");
        server.add_file("/home/user/tree/ok", b"ok");
        server.set_mode("/home/user/tree/ro", 0o555);
        let report = delete_paths(
            &*fs,
            "/home/user",
            &[
                "/".into(),
                "/home/user/".into(),
                "/home".into(),
                "/ro/locked".into(),
                "/missing".into(),
                "/home/user/tree".into(),
            ],
        )
        .await;
        let failed: Vec<&str> = report.failed.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            failed,
            [
                "/",
                "/home/user",
                "/home",
                "/ro/locked",
                "/missing",
                "/home/user/tree/ro/f"
            ]
        );
        assert!(report.failed[3].message.contains("permission denied"));
        // Only the deletable file inside the tree went; the folders stay.
        assert_eq!(report.deleted, 1);
        assert!(server.exists("/home/user/tree/ro/f"));
        assert!(!server.exists("/home/user/tree/ok"));
    }

    #[tokio::test]
    async fn chmod_checks_the_mode() {
        let (server, fs) = setup();
        server.add_file("/home/user/run.sh", b"#!/bin/sh");
        let entry = chmod(&*fs, "/home/user/run.sh", 0o755).await.unwrap();
        assert_eq!(entry.mode, Some(0o755));
        assert_eq!(
            chmod(&*fs, "/home/user/run.sh", 0o10000)
                .await
                .unwrap_err()
                .code(),
            ErrorCode::Validation
        );
    }
}
