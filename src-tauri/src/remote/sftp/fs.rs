//! [`RemoteFs`] over one SFTP channel.
//!
//! Requests on a channel are independent (each carries an id), so `&self` methods run
//! concurrently and reads and writes are pipelined: up to [`DEPTH`] requests of
//! [`Extensions::read_len`]/[`Extensions::write_len`] bytes are in flight, which keeps memory at
//! about 1 MiB per transfer whatever the file size.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use russh_sftp::client::RawSftpSession;
use russh_sftp::client::error::Error as SftpError;
use russh_sftp::protocol::{FileAttributes, OpenFlags, Packet, StatusCode};
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::task::{JoinHandle, JoinSet};

use super::common::{
    CHUNK, STALE_AFTER_SECS, backup_sibling, is_own_sibling, now_secs, random_suffix, temp_sibling,
};
use super::error::{from_sftp, is_status};
use super::session::{ClientHandler, Extensions};
use crate::domain::entry::{Entry, EntryKind};
use crate::domain::session::Capabilities;
use crate::error::{AppError, AppResult};
use crate::remote::{Logger, Progress, RemoteFs, WriteOutcome, path};

/// Pipelined requests in flight per read or write stream.
const DEPTH: usize = 16;

/// Symlinks resolved concurrently while listing.
const LINK_LOOKUPS: usize = 16;

/// The SSH connection shared by every channel of one `Connection`.
pub struct Shared {
    pub handle: russh::client::Handle<ClientHandler>,
    pub closed: AtomicBool,
}

impl Shared {
    pub fn is_alive(&self) -> bool {
        !self.closed.load(Ordering::Acquire) && !self.handle.is_closed()
    }
}

/// One SFTP channel.
pub struct SftpFs {
    sftp: Arc<RawSftpSession>,
    extensions: Extensions,
    shared: Arc<Shared>,
    logger: Logger,
    closed: AtomicBool,
}

#[derive(Serialize)]
struct PosixRename<'a> {
    oldpath: &'a str,
    newpath: &'a str,
}

impl SftpFs {
    pub fn new(
        sftp: RawSftpSession,
        extensions: Extensions,
        shared: Arc<Shared>,
        logger: Logger,
    ) -> Self {
        Self {
            sftp: Arc::new(sftp),
            extensions,
            shared,
            logger,
            closed: AtomicBool::new(false),
        }
    }

    fn is_open(&self) -> bool {
        !self.closed.load(Ordering::Acquire) && self.shared.is_alive()
    }

    fn ensure_open(&self) -> AppResult<()> {
        if self.is_open() {
            Ok(())
        } else {
            Err(AppError::Disconnected)
        }
    }

    /// Maps and logs a failure. Once the transport is gone every failure is `Disconnected`.
    fn fail(&self, error: SftpError, path: &str) -> AppError {
        let error = if self.is_open() {
            from_sftp(error, path)
        } else {
            AppError::Disconnected
        };
        self.log_error(&error);
        error
    }

    fn log_error(&self, error: &AppError) {
        self.logger.error(error.to_string());
    }

    async fn exists(&self, path: &str) -> AppResult<bool> {
        match self.sftp.lstat(path).await {
            Ok(_) => Ok(true),
            Err(error) if is_status(&error, StatusCode::NoSuchFile) => Ok(false),
            Err(error) => Err(self.fail(error, path)),
        }
    }

    /// Fails with `Conflict` when `path` exists.
    async fn ensure_absent(&self, path: &str) -> AppResult<()> {
        if self.exists(path).await? {
            let error = AppError::Conflict(format!("{path} already exists"));
            self.log_error(&error);
            return Err(error);
        }
        Ok(())
    }

    /// A failed creation: `Conflict` when `path` exists after all, else the mapped error.
    async fn conflict_or(&self, error: SftpError, path: &str) -> AppError {
        if self.exists(path).await.unwrap_or(false) {
            let conflict = AppError::Conflict(format!("{path} already exists"));
            self.log_error(&conflict);
            conflict
        } else {
            self.fail(error, path)
        }
    }

    async fn open(
        &self,
        path: &str,
        flags: OpenFlags,
        attrs: FileAttributes,
    ) -> Result<String, SftpError> {
        self.sftp
            .open(path, flags, attrs)
            .await
            .map(|handle| handle.handle)
    }

    async fn close_handle(&self, handle: String) {
        if let Err(error) = self.sftp.close(handle).await {
            log::debug!("closing an SFTP handle failed: {error}");
        }
    }

    /// Writes all of `data` to an open handle and, when the server supports it, fsyncs it.
    async fn write_handle(&self, handle: &str, data: &[u8]) -> Result<(), SftpError> {
        let mut writer = Writer::new(Arc::clone(&self.sftp), handle.to_owned(), 0);
        for piece in data.chunks(self.extensions.write_len as usize) {
            writer.write(piece.to_vec()).await?;
        }
        writer.finish().await?;
        if self.extensions.fsync {
            self.sftp.fsync(handle).await?;
        }
        Ok(())
    }

    async fn rename_raw(&self, from: &str, to: &str) -> Result<(), SftpError> {
        self.sftp.rename(from, to).await.map(|_| ())
    }

    /// `posix-rename@openssh.com`: replaces `to` atomically.
    async fn posix_rename(&self, from: &str, to: &str) -> Result<(), SftpError> {
        let data = russh_sftp::ser::to_bytes(&PosixRename {
            oldpath: from,
            newpath: to,
        })
        .map_err(|error| SftpError::UnexpectedBehavior(error.to_string()))?;
        match self
            .sftp
            .extended("posix-rename@openssh.com", data.to_vec())
            .await?
        {
            Packet::Status(status) if status.status_code == StatusCode::Ok => Ok(()),
            Packet::Status(status) => Err(SftpError::Status(status)),
            _ => Err(SftpError::UnexpectedPacket),
        }
    }

    /// Moves the written temp file over `target`.
    ///
    /// With `posix-rename@openssh.com` this is one atomic step. Without it, plain SFTP rename
    /// refuses an existing target, so the target is first moved aside to a backup sibling,
    /// then the temp file takes its name and the backup is deleted; if the second step fails the
    /// backup is moved back. See ADR 0005. Returns whether it was one atomic step.
    async fn move_into_place(&self, temp: &str, target: &str) -> AppResult<bool> {
        if self.extensions.posix_rename {
            self.logger
                .command(format!("posix-rename {temp} -> {target}"));
            return self
                .posix_rename(temp, target)
                .await
                .map(|()| true)
                .map_err(|error| self.fail(error, target));
        }
        if !self.exists(target).await? {
            self.logger.command(format!("rename {temp} -> {target}"));
            return self
                .rename_raw(temp, target)
                .await
                .map(|()| true)
                .map_err(|error| self.fail(error, target));
        }
        if self
            .sftp
            .lstat(target)
            .await
            .is_ok_and(|attrs| kind_of(&attrs.attrs, None) == EntryKind::Dir)
        {
            let error = AppError::Validation(format!("{target} is a folder"));
            self.log_error(&error);
            return Err(error);
        }
        let backup = backup_sibling(target, &random_suffix()?);
        self.logger.command(format!("rename {target} -> {backup}"));
        self.rename_raw(target, &backup)
            .await
            .map_err(|error| self.fail(error, target))?;
        self.logger.command(format!("rename {temp} -> {target}"));
        if let Err(error) = self.rename_raw(temp, target).await {
            let error = self.fail(error, target);
            self.logger
                .command(format!("rename {backup} -> {target} (restore)"));
            if let Err(restore) = self.rename_raw(&backup, target).await {
                self.logger.error(format!(
                    "could not restore {target} from {backup}: {restore}"
                ));
            }
            return Err(error);
        }
        self.logger.command(format!("remove {backup}"));
        if let Err(error) = self.sftp.remove(backup.as_str()).await {
            // The new contents are in place; a leftover backup is only clutter.
            self.logger
                .error(format!("could not remove the backup {backup}: {error}"));
        }
        Ok(false)
    }

    /// Removes Gem's own temp and backup siblings of `target` (exact names only) that are older
    /// than [`STALE_AFTER_SECS`]: leftovers of a save interrupted by a dropped connection.
    /// `keep` is never removed. Failures are logged and ignored.
    async fn sweep_stale(&self, target: &str, keep: Option<&str>) {
        let dir = path::parent(target);
        self.logger
            .command(format!("opendir {dir} (stale temp files of {target})"));
        let Ok(handle) = self
            .sftp
            .opendir(dir.as_str())
            .await
            .map(|handle| handle.handle)
        else {
            return;
        };
        let mut stale = Vec::new();
        let now = now_secs();
        // Until EOF (or any error: the sweep is best effort).
        while let Ok(name) = self.sftp.readdir(handle.as_str()).await {
            stale.extend(
                name.files
                    .into_iter()
                    .filter(|file| is_own_sibling(&file.filename, target))
                    .filter(|file| {
                        file.attrs
                            .mtime
                            .is_some_and(|mtime| now - i64::from(mtime) > STALE_AFTER_SECS)
                    })
                    .map(|file| path::join(&dir, &file.filename))
                    .filter(|stale| Some(stale.as_str()) != keep),
            );
        }
        self.close_handle(handle).await;
        for stale in stale {
            match self.sftp.remove(stale.as_str()).await {
                Ok(_) => self
                    .logger
                    .status(format!("Removed the stale temp file {stale}")),
                Err(error) => self.logger.error(format!(
                    "could not remove the stale temp file {stale}: {error}"
                )),
            }
        }
    }

    /// Overwrites `path` in place (truncate + write): the non-atomic fallback.
    async fn write_in_place(&self, path: &str, data: &[u8]) -> AppResult<WriteOutcome> {
        self.logger.command(format!(
            "open {path} (write, truncate; in place, not atomic)"
        ));
        let handle = self
            .open(
                path,
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
                FileAttributes::empty(),
            )
            .await
            .map_err(|error| self.fail(error, path))?;
        let written = self.write_handle(&handle, data).await;
        self.close_handle(handle).await;
        written.map_err(|error| self.fail(error, path))?;
        self.logger
            .response(format!("{} bytes written to {path} in place", data.len()));
        Ok(WriteOutcome { atomic: false })
    }

    /// Resolves symlink targets (best effort: a broken link keeps `target_kind = None`).
    async fn resolve_links(&self, entries: &mut [Entry]) {
        let mut tasks = JoinSet::new();
        let links: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.kind == EntryKind::Symlink)
            .map(|(index, _)| index)
            .collect();
        let mut pending = links.into_iter();
        let spawn = |tasks: &mut JoinSet<_>, index: usize, entries: &[Entry]| {
            let sftp = Arc::clone(&self.sftp);
            let link = entries[index].path.clone();
            tasks.spawn(async move {
                let target = sftp
                    .readlink(link.as_str())
                    .await
                    .ok()
                    .and_then(|name| name.files.into_iter().next())
                    .map(|file| file.filename);
                let target_attrs = sftp.stat(link.as_str()).await.ok().map(|attrs| attrs.attrs);
                let kind = target_attrs.as_ref().map(|attrs| kind_of(attrs, None));
                // A link to a file shows the file's size, not the length of the link text.
                let size = target_attrs
                    .filter(|_| kind == Some(EntryKind::File))
                    .and_then(|attrs| attrs.size);
                (index, target, kind, size)
            });
        };
        for index in pending.by_ref().take(LINK_LOOKUPS) {
            spawn(&mut tasks, index, entries);
        }
        while let Some(joined) = tasks.join_next().await {
            if let Ok((index, target, kind, size)) = joined
                && let Some(entry) = entries.get_mut(index)
            {
                entry.link_target = target;
                entry.target_kind = kind;
                entry.size = size;
            }
            if let Some(index) = pending.next() {
                spawn(&mut tasks, index, entries);
            }
        }
    }

    pub fn close_channel(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            let _ = self.sftp.close_session();
        }
    }
}

#[async_trait]
impl RemoteFs for SftpFs {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            exec: false,
            atomic_rename: self.extensions.posix_rename,
            chmod: true,
            owner: true,
            symlinks: true,
        }
    }

    async fn home(&self) -> AppResult<String> {
        self.canonicalize(".").await
    }

    async fn canonicalize(&self, path: &str) -> AppResult<String> {
        self.ensure_open()?;
        self.logger.command(format!("realpath {path}"));
        let name = self
            .sftp
            .realpath(path)
            .await
            .map_err(|error| self.fail(error, path))?;
        let resolved = name
            .files
            .into_iter()
            .next()
            .map(|file| file.filename)
            .ok_or_else(|| AppError::Remote(format!("the server did not resolve {path}")))?;
        // OpenSSH resolves paths that do not exist; FTP's CWD does not. Be consistent.
        self.sftp
            .stat(resolved.as_str())
            .await
            .map_err(|error| self.fail(error, path))?;
        self.logger.response(resolved.clone());
        Ok(resolved)
    }

    async fn list(&self, dir: &str) -> AppResult<Vec<Entry>> {
        self.ensure_open()?;
        self.logger.command(format!("opendir {dir}"));
        let handle = self
            .sftp
            .opendir(dir)
            .await
            .map_err(|error| self.fail(error, dir))?
            .handle;
        let mut entries = Vec::new();
        loop {
            match self.sftp.readdir(handle.as_str()).await {
                Ok(name) => entries.extend(
                    name.files
                        .into_iter()
                        .filter(|file| file.filename != "." && file.filename != "..")
                        .map(|file| {
                            let path = path::join(dir, &file.filename);
                            entry_from(file.filename, path, &file.attrs, Some(&file.longname))
                        }),
                ),
                Err(error) if is_status(&error, StatusCode::Eof) => break,
                Err(error) => {
                    self.close_handle(handle).await;
                    return Err(self.fail(error, dir));
                }
            }
        }
        self.close_handle(handle).await;
        self.resolve_links(&mut entries).await;
        self.logger.response(format!("{} entries", entries.len()));
        Ok(entries)
    }

    async fn stat(&self, target: &str) -> AppResult<Entry> {
        self.ensure_open()?;
        self.logger.command(format!("stat {target}"));
        let attrs = self
            .sftp
            .stat(target)
            .await
            .map_err(|error| self.fail(error, target))?
            .attrs;
        let entry = entry_from(
            path::file_name(target).to_owned(),
            target.to_owned(),
            &attrs,
            None,
        );
        self.logger.response(describe(&entry));
        Ok(entry)
    }

    async fn read_range(&self, target: &str, offset: u64, len: u64) -> AppResult<Vec<u8>> {
        self.ensure_open()?;
        self.logger
            .command(format!("open {target} (read {len} bytes at {offset})"));
        let handle = self
            .open(target, OpenFlags::READ, FileAttributes::empty())
            .await
            .map_err(|error| self.fail(error, target))?;
        let mut reader = Reader::new(
            Arc::clone(&self.sftp),
            handle.clone(),
            offset,
            Some(offset.saturating_add(len)),
            self.extensions.read_len,
        );
        let capacity = usize::try_from(len.min(64 * 1024 * 1024)).unwrap_or(0);
        let mut out = Vec::with_capacity(capacity);
        let result = loop {
            match reader.next().await {
                Ok(Some(chunk)) => out.extend_from_slice(&chunk),
                Ok(None) => break Ok(()),
                Err(error) => break Err(error),
            }
        };
        drop(reader);
        self.close_handle(handle).await;
        result.map_err(|error| self.fail(error, target))?;
        self.logger.response(format!("{} bytes", out.len()));
        Ok(out)
    }

    async fn write_atomic(
        &self,
        target: &str,
        data: &[u8],
        mode: Option<u32>,
        allow_in_place: bool,
    ) -> AppResult<WriteOutcome> {
        self.ensure_open()?;
        self.sweep_stale(target, None).await;
        let temp = temp_sibling(target, &random_suffix()?);
        self.logger.command(format!(
            "open {temp} (write, exclusive; {} bytes for {target})",
            data.len()
        ));
        let attrs = FileAttributes {
            permissions: mode.map(|mode| mode & 0o7777),
            ..FileAttributes::empty()
        };
        // Armed before the temp file is even requested: if this future is dropped while the
        // open is in flight, the server may still create the file.
        let mut guard = TempGuard::new(Arc::clone(&self.sftp), temp.clone(), None);
        let handle = match self
            .open(
                &temp,
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
                attrs,
            )
            .await
        {
            Ok(handle) => handle,
            Err(error) if is_status(&error, StatusCode::PermissionDenied) => {
                guard.disarm();
                let dir = path::parent(target);
                if allow_in_place {
                    self.logger.status(format!(
                        "{dir} is not writable; overwriting {target} in place"
                    ));
                    return self.write_in_place(target, data).await;
                }
                let error = AppError::Unsupported(format!(
                    "the folder {dir} isn't writable, so {target} can't be saved safely \
                     (temp file + rename); it can only be overwritten in place"
                ));
                self.log_error(&error);
                return Err(error);
            }
            Err(error) => {
                guard.disarm();
                return Err(self.fail(error, target));
            }
        };
        // From here on the temp file exists: it is removed on every failure, and by the guard
        // if this future is dropped (cancelled) half-way.
        guard.handle = Some(handle.clone());
        let written = async {
            self.write_handle(&handle, data).await?;
            if let Some(mode) = mode {
                // Explicitly, because the mode given at open time is subject to the umask.
                let attrs = FileAttributes {
                    permissions: Some(mode & 0o7777),
                    ..FileAttributes::empty()
                };
                self.sftp.fsetstat(handle.as_str(), attrs).await?;
            }
            Ok::<_, SftpError>(())
        }
        .await;
        guard.handle = None;
        self.close_handle(handle).await;
        let result = match written {
            Ok(()) => self.move_into_place(&temp, target).await,
            Err(error) => Err(self.fail(error, target)),
        };
        match result {
            // A backup dance still never shows a half-written file: temp file + rename.
            Ok(_) => {
                guard.disarm();
                self.logger
                    .response(format!("saved {target} ({} bytes, atomic)", data.len()));
                Ok(WriteOutcome { atomic: true })
            }
            Err(error) => {
                guard.disarm();
                self.logger.command(format!("remove {temp}"));
                if let Err(cleanup) = self.sftp.remove(temp.as_str()).await
                    && !is_status(&cleanup, StatusCode::NoSuchFile)
                {
                    self.logger
                        .error(format!("could not remove the temp file {temp}: {cleanup}"));
                }
                Err(error)
            }
        }
    }

    async fn replace(&self, temp: &str, target: &str) -> AppResult<WriteOutcome> {
        self.ensure_open()?;
        if !self.exists(temp).await? {
            let error = AppError::not_found(temp);
            self.log_error(&error);
            return Err(error);
        }
        self.sweep_stale(target, Some(temp)).await;
        let atomic = self.move_into_place(temp, target).await?;
        self.logger.response(format!(
            "replaced {target} ({})",
            if atomic {
                "atomic"
            } else {
                "via a backup copy"
            }
        ));
        Ok(WriteOutcome { atomic })
    }

    async fn create_file(&self, target: &str) -> AppResult<()> {
        self.ensure_open()?;
        self.logger
            .command(format!("open {target} (create, exclusive)"));
        match self
            .open(
                target,
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE,
                FileAttributes::empty(),
            )
            .await
        {
            Ok(handle) => {
                self.close_handle(handle).await;
                self.logger.response(format!("created {target}"));
                Ok(())
            }
            // OpenSSH answers a plain "Failure" when O_EXCL hits an existing file.
            Err(error) => Err(self.conflict_or(error, target).await),
        }
    }

    async fn mkdir(&self, target: &str) -> AppResult<()> {
        self.ensure_open()?;
        self.logger.command(format!("mkdir {target}"));
        match self.sftp.mkdir(target, FileAttributes::empty()).await {
            Ok(_) => {
                self.logger.response(format!("created {target}"));
                Ok(())
            }
            Err(error) => Err(self.conflict_or(error, target).await),
        }
    }

    async fn rename(&self, from: &str, to: &str) -> AppResult<()> {
        self.ensure_open()?;
        // SSH_FXP_RENAME already refuses an existing target on OpenSSH, but not on every
        // server; check first so a rename never overwrites.
        self.ensure_absent(to).await?;
        self.logger.command(format!("rename {from} -> {to}"));
        self.rename_raw(from, to)
            .await
            .map_err(|error| self.fail(error, from))?;
        self.logger.response("renamed");
        Ok(())
    }

    async fn remove_file(&self, target: &str) -> AppResult<()> {
        self.ensure_open()?;
        self.logger.command(format!("remove {target}"));
        self.sftp
            .remove(target)
            .await
            .map_err(|error| self.fail(error, target))?;
        self.logger.response(format!("removed {target}"));
        Ok(())
    }

    async fn remove_dir(&self, target: &str) -> AppResult<()> {
        self.ensure_open()?;
        self.logger.command(format!("rmdir {target}"));
        match self.sftp.rmdir(target).await {
            Ok(_) => {
                self.logger.response(format!("removed {target}"));
                Ok(())
            }
            Err(error) if is_status(&error, StatusCode::Failure) => {
                let error = AppError::Remote(format!(
                    "{target} could not be removed (is the folder empty?)"
                ));
                self.log_error(&error);
                Err(error)
            }
            Err(error) => Err(self.fail(error, target)),
        }
    }

    async fn chmod(&self, target: &str, mode: u32) -> AppResult<()> {
        self.ensure_open()?;
        let mode = mode & 0o7777;
        self.logger.command(format!("chmod {mode:o} {target}"));
        let attrs = FileAttributes {
            permissions: Some(mode),
            ..FileAttributes::empty()
        };
        self.sftp
            .setstat(target, attrs)
            .await
            .map_err(|error| self.fail(error, target))?;
        self.logger.response("permissions changed");
        Ok(())
    }

    async fn download(
        &self,
        source: &str,
        offset: u64,
        sink: &mut (dyn AsyncWrite + Send + Unpin),
        progress: Progress<'_>,
        cancel: &AtomicBool,
    ) -> AppResult<u64> {
        self.ensure_open()?;
        self.logger
            .command(format!("open {source} (download from {offset})"));
        let handle = self
            .open(source, OpenFlags::READ, FileAttributes::empty())
            .await
            .map_err(|error| self.fail(error, source))?;
        let mut reader = Reader::new(
            Arc::clone(&self.sftp),
            handle.clone(),
            offset,
            None,
            self.extensions.read_len,
        );
        let mut total = 0u64;
        let result = async {
            loop {
                if cancel.load(Ordering::Acquire) {
                    return Err(AppError::Cancelled);
                }
                let Some(chunk) = reader
                    .next()
                    .await
                    .map_err(|error| self.fail(error, source))?
                else {
                    break;
                };
                sink.write_all(&chunk).await.map_err(local_error)?;
                total += chunk.len() as u64;
                progress(total);
            }
            sink.flush().await.map_err(local_error)?;
            Ok(())
        }
        .await;
        drop(reader);
        self.close_handle(handle).await;
        match result {
            Ok(()) => {
                self.logger
                    .response(format!("downloaded {total} bytes from {source}"));
                Ok(total)
            }
            Err(AppError::Cancelled) => {
                self.logger
                    .status(format!("download of {source} cancelled"));
                Err(AppError::Cancelled)
            }
            Err(error) => Err(error),
        }
    }

    async fn upload(
        &self,
        source: &mut (dyn AsyncRead + Send + Unpin),
        target: &str,
        progress: Progress<'_>,
        cancel: &AtomicBool,
    ) -> AppResult<u64> {
        self.ensure_open()?;
        self.logger
            .command(format!("open {target} (write, truncate; upload)"));
        let handle = self
            .open(
                target,
                OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
                FileAttributes::empty(),
            )
            .await
            .map_err(|error| self.fail(error, target))?;
        let mut writer = Writer::new(Arc::clone(&self.sftp), handle.clone(), 0);
        let mut buffer = vec![0u8; CHUNK];
        let mut total = 0u64;
        let result = async {
            loop {
                if cancel.load(Ordering::Acquire) {
                    return Err(AppError::Cancelled);
                }
                let read = source.read(&mut buffer).await.map_err(local_error)?;
                if read == 0 {
                    break;
                }
                for piece in buffer[..read].chunks(self.extensions.write_len as usize) {
                    writer
                        .write(piece.to_vec())
                        .await
                        .map_err(|error| self.fail(error, target))?;
                }
                total += read as u64;
                progress(total);
            }
            writer
                .finish()
                .await
                .map_err(|error| self.fail(error, target))
        }
        .await;
        drop(writer);
        self.close_handle(handle).await;
        match result {
            Ok(()) => {
                self.logger
                    .response(format!("uploaded {total} bytes to {target}"));
                Ok(total)
            }
            Err(AppError::Cancelled) => {
                self.logger.status(format!("upload to {target} cancelled"));
                Err(AppError::Cancelled)
            }
            Err(error) => Err(error),
        }
    }

    async fn keepalive(&self) -> AppResult<()> {
        self.ensure_open()?;
        self.sftp
            .realpath(".")
            .await
            .map(|_| ())
            .map_err(|error| self.fail(error, "."))
    }

    async fn close(&self) {
        if self.is_open() {
            self.logger.status("SFTP channel closed");
        }
        self.close_channel();
    }
}

/// A local I/O failure (the download sink or the upload source).
fn local_error(error: std::io::Error) -> AppError {
    AppError::Platform(format!("local file error: {error}"))
}

fn describe(entry: &Entry) -> String {
    let kind = match entry.kind {
        EntryKind::File => "file",
        EntryKind::Dir => "folder",
        EntryKind::Symlink => "symlink",
        EntryKind::Other => "special file",
    };
    match entry.size {
        Some(size) if entry.kind == EntryKind::File => format!("{kind}, {size} bytes"),
        _ => kind.to_owned(),
    }
}

/// File type from the `S_IFMT` bits of the permissions, else from the long name's first char.
fn kind_of(attrs: &FileAttributes, longname: Option<&str>) -> EntryKind {
    if let Some(mode) = attrs.permissions {
        return match mode & 0o170_000 {
            0o040_000 => EntryKind::Dir,
            0o100_000 => EntryKind::File,
            0o120_000 => EntryKind::Symlink,
            0 => EntryKind::File,
            _ => EntryKind::Other,
        };
    }
    match longname.and_then(|name| name.chars().next()) {
        Some('d') => EntryKind::Dir,
        Some('l') => EntryKind::Symlink,
        Some('-') | None => EntryKind::File,
        Some(_) => EntryKind::Other,
    }
}

/// Owner and group from an `ls -l` style long name (`-rw-r--r-- 1 www www 42 …`).
fn owner_group(longname: &str) -> Option<(String, String)> {
    let mut fields = longname.split_whitespace();
    let mode = fields.next()?;
    let links = fields.next()?;
    if mode.len() < 10 || !links.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((fields.next()?.to_owned(), fields.next()?.to_owned()))
}

fn entry_from(name: String, path: String, attrs: &FileAttributes, longname: Option<&str>) -> Entry {
    let kind = kind_of(attrs, longname);
    let (owner, group) = match longname.and_then(owner_group) {
        Some((owner, group)) => (Some(owner), Some(group)),
        None => (
            attrs.user.clone().or(attrs.uid.map(|uid| uid.to_string())),
            attrs.group.clone().or(attrs.gid.map(|gid| gid.to_string())),
        ),
    };
    Entry {
        name,
        path,
        kind,
        size: attrs.size,
        mtime: attrs.mtime.map(i64::from),
        mode: attrs.permissions.map(|mode| mode & 0o7777),
        owner,
        group,
        link_target: None,
        target_kind: None,
    }
}

/// Removes a temp file if an atomic write is abandoned (its future dropped) before it finished.
struct TempGuard {
    sftp: Arc<RawSftpSession>,
    path: Option<String>,
    handle: Option<String>,
}

impl TempGuard {
    fn new(sftp: Arc<RawSftpSession>, path: String, handle: Option<String>) -> Self {
        Self {
            sftp,
            path: Some(path),
            handle,
        }
    }

    fn disarm(&mut self) {
        self.path = None;
        self.handle = None;
    }
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        let Some(path) = self.path.take() else {
            return;
        };
        let handle = self.handle.take();
        let sftp = Arc::clone(&self.sftp);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Some(handle) = handle {
                    let _ = sftp.close(handle).await;
                }
                let _ = sftp.remove(path).await;
            });
        }
    }
}

/// One read request in flight: its offset, its length and the task awaiting the reply.
type InFlightRead = (u64, u32, JoinHandle<Result<Vec<u8>, SftpError>>);

/// Sequential reader with up to [`DEPTH`] read requests in flight, returning chunks in order.
struct Reader {
    sftp: Arc<RawSftpSession>,
    handle: String,
    next_offset: u64,
    end: Option<u64>,
    block: u32,
    eof: bool,
    inflight: VecDeque<InFlightRead>,
}

impl Reader {
    fn new(
        sftp: Arc<RawSftpSession>,
        handle: String,
        offset: u64,
        end: Option<u64>,
        block: u32,
    ) -> Self {
        Self {
            sftp,
            handle,
            next_offset: offset,
            end,
            block: block.max(1),
            eof: false,
            inflight: VecDeque::new(),
        }
    }

    fn fill(&mut self) {
        while !self.eof && self.inflight.len() < DEPTH {
            let len = match self.end {
                Some(end) if self.next_offset >= end => break,
                Some(end) => u32::try_from((end - self.next_offset).min(u64::from(self.block)))
                    .unwrap_or(self.block),
                None => self.block,
            };
            let sftp = Arc::clone(&self.sftp);
            let handle = self.handle.clone();
            let offset = self.next_offset;
            let task =
                tokio::spawn(
                    async move { sftp.read(handle, offset, len).await.map(|data| data.data) },
                );
            self.inflight.push_back((offset, len, task));
            self.next_offset += u64::from(len);
        }
    }

    /// The next chunk, `None` at the end of the range or file.
    async fn next(&mut self) -> Result<Option<Vec<u8>>, SftpError> {
        self.fill();
        let Some((offset, len, task)) = self.inflight.pop_front() else {
            return Ok(None);
        };
        let result = task
            .await
            .map_err(|error| SftpError::UnexpectedBehavior(error.to_string()))?;
        match result {
            Ok(data) if data.is_empty() => {
                self.stop();
                Ok(None)
            }
            Ok(mut data) => {
                // A short read is not the end of the file: fetch the rest of this block before
                // handing out later blocks, so chunks stay contiguous.
                while data.len() < len as usize {
                    let at = offset + data.len() as u64;
                    let want = len - u32::try_from(data.len()).unwrap_or(len);
                    match self.sftp.read(self.handle.as_str(), at, want).await {
                        Ok(more) if !more.data.is_empty() => data.extend_from_slice(&more.data),
                        Ok(_) => {
                            self.stop();
                            break;
                        }
                        Err(error) if is_status(&error, StatusCode::Eof) => {
                            self.stop();
                            break;
                        }
                        Err(error) => {
                            self.stop();
                            return Err(error);
                        }
                    }
                }
                Ok(Some(data))
            }
            Err(error) if is_status(&error, StatusCode::Eof) => {
                self.stop();
                Ok(None)
            }
            Err(error) => {
                self.stop();
                Err(error)
            }
        }
    }

    /// Stops issuing requests and abandons those in flight (their replies are ignored).
    fn stop(&mut self) {
        self.eof = true;
        for (_, _, task) in self.inflight.drain(..) {
            task.abort();
        }
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Sequential writer with up to [`DEPTH`] write requests in flight.
struct Writer {
    sftp: Arc<RawSftpSession>,
    handle: String,
    offset: u64,
    inflight: VecDeque<JoinHandle<Result<(), SftpError>>>,
}

impl Writer {
    fn new(sftp: Arc<RawSftpSession>, handle: String, offset: u64) -> Self {
        Self {
            sftp,
            handle,
            offset,
            inflight: VecDeque::new(),
        }
    }

    async fn wait_one(&mut self) -> Result<(), SftpError> {
        if let Some(task) = self.inflight.pop_front() {
            task.await
                .map_err(|error| SftpError::UnexpectedBehavior(error.to_string()))??;
        }
        Ok(())
    }

    async fn write(&mut self, data: Vec<u8>) -> Result<(), SftpError> {
        while self.inflight.len() >= DEPTH {
            self.wait_one().await?;
        }
        let sftp = Arc::clone(&self.sftp);
        let handle = self.handle.clone();
        let offset = self.offset;
        self.offset += data.len() as u64;
        self.inflight.push_back(tokio::spawn(async move {
            sftp.write(handle, offset, data).await.map(|_| ())
        }));
        Ok(())
    }

    async fn finish(&mut self) -> Result<(), SftpError> {
        while !self.inflight.is_empty() {
            self.wait_one().await?;
        }
        Ok(())
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        for task in self.inflight.drain(..) {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_owner_and_group_from_long_names() {
        assert_eq!(
            owner_group("-rw-r--r--    1 www-data www-data   4096 Nov  8 17:05 index.php"),
            Some(("www-data".into(), "www-data".into()))
        );
        assert_eq!(owner_group("index.php"), None);
    }

    #[test]
    fn file_types_come_from_mode_bits() {
        let attrs = |mode| FileAttributes {
            permissions: Some(mode),
            ..FileAttributes::empty()
        };
        assert_eq!(kind_of(&attrs(0o040_755), None), EntryKind::Dir);
        assert_eq!(kind_of(&attrs(0o100_644), None), EntryKind::File);
        assert_eq!(kind_of(&attrs(0o120_777), None), EntryKind::Symlink);
        // Block devices share bits with directories; they must not look like folders.
        assert_eq!(kind_of(&attrs(0o060_660), None), EntryKind::Other);
        assert_eq!(kind_of(&attrs(0o140_755), None), EntryKind::Other);
        assert_eq!(
            kind_of(
                &FileAttributes::empty(),
                Some("drwxr-xr-x 2 a b 0 Jan 1 2020 x")
            ),
            EntryKind::Dir
        );
        let entry = entry_from("x".into(), "/x".into(), &attrs(0o102_755), None);
        assert_eq!(entry.mode, Some(0o2755));
    }
}
