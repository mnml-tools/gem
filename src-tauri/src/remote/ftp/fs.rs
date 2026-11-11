//! [`RemoteFs`] over one FTP control connection.
//!
//! FTP cannot pipeline commands, so every operation holds the control connection (a tokio
//! mutex) from its first command to its last reply. An operation that is abandoned half-way
//! (its future dropped, or timed out) leaves the connection marked for resynchronisation: the
//! next operation first sends `NOOP` and discards replies until the stream is in step again.
//!
//! Partial reads (`read_range`) use `RANG` when the server offers it (it then sends exactly the
//! range). Otherwise `REST` + `RETR`: read the bytes needed, close the data connection, then
//! consume the server's `426`/`226` and a `NOOP` reply, so the control connection stays usable
//! (ADR 0004).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use suppaftp::types::Response;
use suppaftp::{FtpError, FtpResult, Status};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{Mutex, MutexGuard};

use super::listing::{Listed, parse_list_line, parse_mlsx};
use super::reply::{
    self, Op, from_ftp, from_io, is_data_refusal, is_fatal, is_transfer_end, is_unsupported,
};
use super::{Reopen, open_control};
use crate::domain::entry::{Entry, EntryKind};
use crate::domain::session::Capabilities;
use crate::error::{AppError, AppResult};
use crate::remote::sftp::common::{
    CHUNK, STALE_AFTER_SECS, backup_sibling, is_own_sibling, now_secs, random_suffix, temp_sibling,
};
use crate::remote::{Logger, Progress, RemoteFs, WriteOutcome, path};

/// The suppaftp client type for plain FTP and FTPS alike (TLS is used only when negotiated).
pub type Ftp = suppaftp::tokio::AsyncRustlsFtpStream;

/// Pause before the one retry of a refused data connection.
const DATA_RETRY_PAUSE: Duration = Duration::from_millis(250);

/// A data connection opened by a transfer command.
type DataTransfer = suppaftp::tokio::TransferStream<suppaftp::tokio::AsyncRustlsStream>;

/// Symlinks resolved per listing (each costs a round trip on the control connection).
const MAX_LINK_LOOKUPS: usize = 64;

/// What the server announced at login.
#[derive(Debug, Clone, Copy, Default)]
pub struct Features {
    /// `MLST` in `FEAT`: listings and stat use MLSD/MLST facts.
    pub mlst: bool,
    /// Listings carry owner/group (MLST `UNIX.owner*` facts, or a Unix `LIST` format).
    pub owner: bool,
    /// `SYST` says UNIX: permissions, `SITE CHMOD` and symlinks are expected to work.
    pub unix: bool,
    /// `RANG` in `FEAT` (draft-bryan-ftp-range): the server sends exactly a byte range.
    pub rang: bool,
}

struct Control {
    ftp: Ftp,
    /// Set while an operation runs; still set afterwards means it was abandoned half-way.
    needs_resync: bool,
}

pub struct Inner {
    control: Mutex<Control>,
    features: Features,
    home: String,
    logger: Logger,
    op_timeout: Duration,
    closed: AtomicBool,
    dead: AtomicBool,
    chmod: AtomicBool,
    rnto_overwrites: AtomicBool,
    /// A duplicate of the control socket, so `close` can cut a connection that an operation
    /// (a transfer) is holding.
    kill_switch: std::sync::Mutex<Option<std::net::TcpStream>>,
    reopen: Reopen,
}

/// One FTP control connection (the primary of a `Connection`, or a transfer connection).
pub struct FtpFs {
    inner: Arc<Inner>,
}

impl FtpFs {
    pub fn new(
        ftp: Ftp,
        features: Features,
        home: String,
        logger: Logger,
        op_timeout: Duration,
        kill_switch: Option<std::net::TcpStream>,
        reopen: Reopen,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                control: Mutex::new(Control {
                    ftp,
                    needs_resync: false,
                }),
                features,
                home,
                logger,
                op_timeout,
                closed: AtomicBool::new(false),
                dead: AtomicBool::new(false),
                chmod: AtomicBool::new(features.unix),
                rnto_overwrites: AtomicBool::new(true),
                kill_switch: std::sync::Mutex::new(kill_switch),
                reopen,
            }),
        }
    }

    pub fn is_alive(&self) -> bool {
        !self.inner.closed.load(Ordering::Acquire) && !self.inner.dead.load(Ordering::Acquire)
    }

    fn logger(&self) -> &Logger {
        &self.inner.logger
    }

    fn ensure_open(&self) -> AppResult<()> {
        if self.is_alive() {
            Ok(())
        } else {
            Err(AppError::Disconnected)
        }
    }

    /// Locks the control connection for one operation, resynchronising it first if the
    /// previous operation was abandoned.
    async fn lock(&self) -> AppResult<MutexGuard<'_, Control>> {
        self.ensure_open()?;
        let mut control = self.inner.control.lock().await;
        self.ensure_open()?;
        if control.needs_resync {
            if let Err(error) = resync(&mut control.ftp, self.logger()).await {
                self.logger()
                    .error(format!("the control connection is out of step: {error}"));
                // Like after a cut-short download: start a fresh control connection.
                if self.reopen(&mut control.ftp).await.is_err() {
                    self.inner.dead.store(true, Ordering::Release);
                    return Err(AppError::Disconnected);
                }
            }
            control.needs_resync = false;
        }
        control.needs_resync = true;
        Ok(control)
    }

    /// Runs one control-connection operation with the operation timeout; `None` when it
    /// timed out (the connection then stays marked for resynchronisation).
    async fn timed<T>(&self, op: impl Future<Output = AppResult<T>>) -> Option<AppResult<T>> {
        tokio::time::timeout(self.inner.op_timeout, op).await.ok()
    }

    /// Maps a failure, marking the connection dead when the control stream broke.
    fn fail(&self, error: &FtpError, path: &str, op: Op) -> AppError {
        if is_fatal(error) {
            self.inner.dead.store(true, Ordering::Release);
        }
        from_ftp(error, path, op)
    }

    /// Logs a failure of a command whose line was logged by the caller.
    fn log_failure(&self, error: &FtpError) {
        match error {
            FtpError::UnexpectedResponse(response) => self.logger().error(reply::text(response)),
            other => self.logger().error(other.to_string()),
        }
    }

    fn log_reply(&self, result: &FtpResult<Response>) {
        match result {
            Ok(response) => self.logger().response(reply::text(response)),
            Err(error) => self.log_failure(error),
        }
    }

    /// Sends one command line and logs it with its reply.
    async fn command(&self, ftp: &mut Ftp, line: &str, expect: &[Status]) -> FtpResult<Response> {
        self.logger().command(line);
        let result = ftp.custom_command(line, expect).await;
        self.log_reply(&result);
        result
    }

    /// Logs the outcome of a probe (see [`reply::probe_log_kind`]): expected negative replies are
    /// responses, not errors, so a normal listing shows no red lines.
    fn log_probe<T>(&self, result: &FtpResult<T>, ok_text: impl FnOnce(&T) -> String) {
        let text = match result {
            Ok(value) => ok_text(value),
            Err(FtpError::UnexpectedResponse(response)) => reply::text(response),
            Err(error) => error.to_string(),
        };
        self.logger().log(reply::probe_log_kind(result), text);
    }

    /// [`Self::log_probe`] for a failure already in hand.
    fn log_probe_failure(&self, error: &FtpError) {
        match error {
            FtpError::UnexpectedResponse(response) => self.logger().response(reply::text(response)),
            other => self.logger().error(other.to_string()),
        }
    }

    /// Sends one probe command line and logs it with its reply (see [`Self::log_probe`]).
    async fn probe(&self, ftp: &mut Ftp, line: &str, expect: &[Status]) -> FtpResult<Response> {
        self.logger().command(line);
        let result = ftp.custom_command(line, expect).await;
        self.log_probe(&result, reply::text);
        result
    }

    /// `CWD` used to find out whether a path is a folder.
    async fn probe_cwd(&self, ftp: &mut Ftp, dir: &str) -> FtpResult<Response> {
        self.probe(
            ftp,
            &format!("CWD {dir}"),
            &[Status::RequestedFileActionOk, Status::CommandOk],
        )
        .await
    }

    /// `SIZE` used to find out whether a path is a file; its size when it is.
    async fn probe_size(&self, ftp: &mut Ftp, target: &str) -> FtpResult<Option<u64>> {
        self.probe(ftp, &format!("SIZE {target}"), &[Status::File])
            .await
            .map(|response| reply::size_reply(&response))
    }

    /// `MLST` used to find out what a path is (logged as a probe).
    async fn probe_mlst(&self, ftp: &mut Ftp, target: &str) -> FtpResult<String> {
        self.logger().command(format!("MLST {target}"));
        let result = ftp.mlst(Some(target)).await;
        self.log_probe(&result, Clone::clone);
        result
    }

    async fn cwd(&self, ftp: &mut Ftp, dir: &str) -> FtpResult<Response> {
        self.command(
            ftp,
            &format!("CWD {dir}"),
            &[Status::RequestedFileActionOk, Status::CommandOk],
        )
        .await
    }

    async fn rnfr_rnto(&self, ftp: &mut Ftp, from: &str, to: &str) -> FtpResult<()> {
        self.command(ftp, &format!("RNFR {from}"), &[Status::RequestFilePending])
            .await?;
        self.command(
            ftp,
            &format!("RNTO {to}"),
            &[Status::RequestedFileActionOk, Status::CommandOk],
        )
        .await
        .map(|_| ())
    }

    async fn delete(&self, ftp: &mut Ftp, target: &str) -> FtpResult<Response> {
        self.command(
            ftp,
            &format!("DELE {target}"),
            &[Status::RequestedFileActionOk, Status::CommandOk],
        )
        .await
    }

    /// `NOOP` and its reply, skipping any reply still queued before it (after an early-closed
    /// transfer). Transfer replies are never `200`, so the first `200` is the NOOP's.
    async fn sync_noop(&self, ftp: &mut Ftp) -> AppResult<()> {
        self.logger().command("NOOP");
        let mut result = ftp.custom_command("NOOP", &[Status::CommandOk]).await;
        for _ in 0..4 {
            match result {
                Err(FtpError::UnexpectedResponse(_)) => {
                    result = ftp.read_response(Status::CommandOk).await;
                }
                _ => break,
            }
        }
        self.log_reply(&result);
        result
            .map(|_| ())
            .map_err(|error| self.fail(&error, "", Op::Read))
    }

    /// Closes a download before its end and brings the control connection back in step.
    ///
    /// Some servers end the whole session when a TLS download is cut short (ProFTPD's mod_tls,
    /// even after `ABOR`). The data asked for was already read, so the control connection is
    /// then reopened (same site, same trust store) instead of failing the operation.
    async fn close_early(
        &self,
        ftp: &mut Ftp,
        stream: suppaftp::tokio::TransferStream<suppaftp::tokio::AsyncRustlsStream>,
    ) -> AppResult<()> {
        let synced = match stream.finish().await {
            Ok(()) => self.sync_noop(ftp).await,
            Err(FtpError::UnexpectedResponse(response))
                if is_transfer_end(response.status)
                    || matches!(reply::code(&response), 426 | 450 | 451) =>
            {
                self.logger().response(reply::text(&response));
                self.sync_noop(ftp).await
            }
            Err(error) if is_fatal(&error) => {
                self.log_failure(&error);
                Err(self.fail(&error, "", Op::Read))
            }
            Err(error) => {
                self.log_failure(&error);
                self.sync_noop(ftp).await
            }
        };
        match synced {
            Ok(()) => Ok(()),
            Err(AppError::Disconnected) => self.reopen(ftp).await,
            Err(error) => Err(error),
        }
    }

    /// Replaces a control connection the server closed with a new one.
    async fn reopen(&self, ftp: &mut Ftp) -> AppResult<()> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(AppError::Disconnected);
        }
        self.logger()
            .status("The control connection was lost after an interrupted transfer; reconnecting");
        let opened = open_control(
            &self.inner.reopen.params,
            &self.inner.reopen.verifier,
            self.logger(),
        )
        .await?;
        *ftp = opened.ftp;
        if let Ok(mut slot) = self.inner.kill_switch.lock() {
            *slot = opened.kill_switch;
        }
        self.inner.dead.store(false, Ordering::Release);
        Ok(())
    }

    /// One retry of a data command whose data connection was refused (see
    /// [`is_data_refusal`]); never after data has moved, because callers only retry the opening.
    async fn retry_pause(&self, attempts: &mut u8, error: &FtpError) -> bool {
        if *attempts > 0 || !is_data_refusal(error) {
            return false;
        }
        *attempts += 1;
        let reason = match error {
            FtpError::UnexpectedResponse(response) => reply::text(response),
            other => other.to_string(),
        };
        self.logger()
            .status(format!("Data connection refused ({reason}), retrying"));
        tokio::time::sleep(DATA_RETRY_PAUSE).await;
        true
    }

    /// `LIST [argument]` (already logged by the caller), retried once on a refused data
    /// connection. The listing arrives whole, so a refusal means nothing was read yet.
    async fn list_lines(&self, ftp: &mut Ftp, argument: Option<&str>) -> FtpResult<Vec<String>> {
        let mut attempts = 0;
        loop {
            match ftp.list(argument).await {
                Err(error) if self.retry_pause(&mut attempts, &error).await => {}
                other => return other,
            }
        }
    }

    /// `MLSD dir` (logged by the caller), retried once on a refused data connection.
    async fn mlsd_lines(&self, ftp: &mut Ftp, dir: &str) -> FtpResult<Vec<String>> {
        let mut attempts = 0;
        loop {
            match ftp.mlsd(Some(dir)).await {
                Err(error) if self.retry_pause(&mut attempts, &error).await => {}
                other => return other,
            }
        }
    }

    /// `STOR target` (logged by the caller), retried once on a refused data connection.
    async fn stor_stream(&self, ftp: &mut Ftp, target: &str) -> FtpResult<DataTransfer> {
        let mut attempts = 0;
        loop {
            match ftp.put_with_stream(target).await {
                Err(error) if self.retry_pause(&mut attempts, &error).await => {}
                other => return other,
            }
        }
    }

    /// The entry named `path` as listed in its parent folder (`CWD parent` + `LIST -a name`);
    /// `None` when it is not there or the folder cannot be read.
    async fn list_one(&self, ftp: &mut Ftp, target: &str) -> AppResult<Option<Listed>> {
        let name = path::file_name(target).to_owned();
        if self.probe_cwd(ftp, &path::parent(target)).await.is_err() {
            return Ok(None);
        }
        // A leading dash would be read as an option.
        let argument = if name.starts_with('-') {
            format!("./{name}")
        } else {
            name.clone()
        };
        self.logger().command(format!("LIST -a {argument}"));
        let lines = match self.list_lines(ftp, Some(&format!("-a {argument}"))).await {
            Ok(lines) => lines,
            Err(error) if is_fatal(&error) => {
                self.log_failure(&error);
                return Err(self.fail(&error, target, Op::Read));
            }
            Err(error) => {
                // "No such file": an expected answer when checking whether a name is free.
                self.log_probe_failure(&error);
                return Ok(None);
            }
        };
        let now = Utc::now();
        let found = lines
            .iter()
            .filter_map(|line| parse_list_line(line, now))
            .find(|listed| !listed.is_dot && listed.name == name);
        self.logger().response(match &found {
            Some(_) => format!("{name} found"),
            None => format!("{name} not listed"),
        });
        Ok(found)
    }

    /// Whether `path` exists (as a file, folder or link, even a broken one).
    async fn exists(&self, ftp: &mut Ftp, target: &str) -> AppResult<bool> {
        if self.inner.features.mlst {
            match self.probe_mlst(ftp, target).await {
                Ok(_) => return Ok(true),
                Err(error) if is_fatal(&error) => {
                    return Err(self.fail(&error, target, Op::Read));
                }
                Err(_) => {}
            }
        } else {
            if self.probe_size(ftp, target).await.is_ok() {
                return Ok(true);
            }
            if self.probe_cwd(ftp, target).await.is_ok() {
                return Ok(true);
            }
        }
        // A broken symlink or an unreadable entry: look for its name in the folder.
        Ok(self.list_one(ftp, target).await?.is_some())
    }

    async fn ensure_absent(&self, ftp: &mut Ftp, target: &str) -> AppResult<()> {
        if self.exists(ftp, target).await? {
            let error = AppError::Conflict(format!("{target} already exists"));
            self.logger().error(error.to_string());
            return Err(error);
        }
        Ok(())
    }

    /// Maps a failed lookup or removal. A vague `550` (vsftpd never says why) is resolved by
    /// checking whether the path exists: then the refusal is about permissions.
    async fn classify(&self, ftp: &mut Ftp, error: FtpError, target: &str) -> AppError {
        let mapped = self.fail(&error, target, Op::Read);
        let FtpError::UnexpectedResponse(response) = &error else {
            return mapped;
        };
        let vague = reply::code(response) == 550 && matches!(mapped, AppError::NotFound { .. });
        if vague && !self.inner.dead.load(Ordering::Acquire) {
            let text = reply::text(response).to_ascii_lowercase();
            let says_missing = ["no such", "not found", "does not exist", "doesn't exist"]
                .iter()
                .any(|word| text.contains(word));
            if !says_missing && matches!(self.exists(ftp, target).await, Ok(true)) {
                return AppError::PermissionDenied(target.to_owned());
            }
        }
        mapped
    }

    /// `STOR` of `data` into `target`. The error says whether the server refused to create the
    /// file (`Err(Refused)`) or failed while it was being written (`Err(Failed)`).
    async fn store(&self, ftp: &mut Ftp, target: &str, data: &[u8]) -> Result<(), StoreError> {
        self.logger().command(format!("STOR {target}"));
        let mut stream = match self.stor_stream(ftp, target).await {
            Ok(stream) => stream,
            Err(error) => {
                self.log_failure(&error);
                return Err(StoreError::Refused(error));
            }
        };
        for piece in data.chunks(CHUNK) {
            if let Err(error) = stream.write_all(piece).await {
                drop(stream);
                let error = FtpError::ConnectionError(error);
                self.log_failure(&error);
                return Err(StoreError::Failed(error));
            }
        }
        match stream.finish().await {
            Ok(()) => {
                self.logger()
                    .response(format!("{} bytes stored in {target}", data.len()));
                Ok(())
            }
            Err(error) => {
                self.log_failure(&error);
                Err(StoreError::Failed(error))
            }
        }
    }

    /// Renames the temp file over `target`. Servers whose `RNTO` refuses an existing target get
    /// the backup dance: target → backup, temp → target, delete backup (restored on failure).
    /// Returns whether it was one atomic step.
    async fn move_into_place(&self, ftp: &mut Ftp, temp: &str, target: &str) -> AppResult<bool> {
        if self.inner.rnto_overwrites.load(Ordering::Acquire) {
            match self.rnfr_rnto(ftp, temp, target).await {
                Ok(()) => return Ok(true),
                Err(error) if is_fatal(&error) => {
                    return Err(self.fail(&error, target, Op::Write));
                }
                Err(error) => {
                    if !self.exists(ftp, target).await? {
                        return Err(self.fail(&error, target, Op::Write));
                    }
                    self.logger().status(
                        "the server does not rename over existing files; using a backup copy",
                    );
                    self.inner.rnto_overwrites.store(false, Ordering::Release);
                }
            }
        } else if !self.exists(ftp, target).await? {
            return self
                .rnfr_rnto(ftp, temp, target)
                .await
                .map(|()| true)
                .map_err(|error| self.fail(&error, target, Op::Write));
        }
        // Never move a folder aside to put a file in its place.
        if self.cwd(ftp, target).await.is_ok() {
            let error = AppError::Validation(format!("{target} is a folder"));
            self.logger().error(error.to_string());
            return Err(error);
        }
        let backup = backup_sibling(target, &random_suffix()?);
        self.rnfr_rnto(ftp, target, &backup)
            .await
            .map_err(|error| self.fail(&error, target, Op::Write))?;
        if let Err(error) = self.rnfr_rnto(ftp, temp, target).await {
            let mapped = self.fail(&error, target, Op::Write);
            if !is_fatal(&error) && self.rnfr_rnto(ftp, &backup, target).await.is_err() {
                self.logger()
                    .error(format!("could not restore {target} from {backup}"));
            }
            return Err(mapped);
        }
        if self.delete(ftp, &backup).await.is_err() {
            self.logger()
                .error(format!("could not remove the backup {backup}"));
        }
        Ok(false)
    }

    async fn resolve_links(&self, ftp: &mut Ftp, entries: &mut [Entry]) -> AppResult<()> {
        let links: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.kind == EntryKind::Symlink)
            .map(|(index, _)| index)
            .take(MAX_LINK_LOOKUPS)
            .collect();
        for index in links {
            let link = entries[index].path.clone();
            let kind = if self.inner.features.mlst {
                match self.probe_mlst(ftp, &link).await {
                    Ok(line) => parse_mlsx(&line).map(|listed| listed.kind),
                    Err(error) if is_fatal(&error) => {
                        return Err(self.fail(&error, &link, Op::Read));
                    }
                    Err(_) => None,
                }
            } else if self.probe_cwd(ftp, &link).await.is_ok() {
                Some(EntryKind::Dir)
            } else {
                match self.probe_size(ftp, &link).await {
                    Ok(size) => {
                        entries[index].size = size;
                        Some(EntryKind::File)
                    }
                    Err(error) if is_fatal(&error) => {
                        return Err(self.fail(&error, &link, Op::Read));
                    }
                    Err(_) => None,
                }
            };
            // A link to a file shows the file's size (MLST reports the link's own size, and
            // LIST the length of the link text): SIZE measures what the path leads to.
            if kind == Some(EntryKind::File) && self.inner.features.mlst {
                entries[index].size = self.probe_size(ftp, &link).await.ok().flatten();
            }
            entries[index].target_kind = kind;
        }
        Ok(())
    }

    async fn do_list(&self, ftp: &mut Ftp, dir: &str) -> AppResult<Vec<Entry>> {
        let mut entries = self.list_entries(ftp, dir).await?;
        self.resolve_links(ftp, &mut entries).await?;
        Ok(entries)
    }

    /// A folder's entries without resolving symlinks.
    async fn list_entries(&self, ftp: &mut Ftp, dir: &str) -> AppResult<Vec<Entry>> {
        let now = Utc::now();
        let lines = if self.inner.features.mlst {
            self.logger().command(format!("MLSD {dir}"));
            match self.mlsd_lines(ftp, dir).await {
                Ok(lines) => lines,
                Err(error) => {
                    self.log_failure(&error);
                    return Err(self.classify(ftp, error, dir).await);
                }
            }
        } else {
            // `CWD` first: paths with spaces or a leading dash survive, and `LIST -a <path>`
            // lists nothing on vsftpd when the path is absolute.
            if let Err(error) = self.cwd(ftp, dir).await {
                return Err(self.classify(ftp, error, dir).await);
            }
            self.logger().command("LIST -a");
            match self.list_lines(ftp, Some("-a")).await {
                Ok(lines) => lines,
                Err(error) if is_fatal(&error) => {
                    self.log_failure(&error);
                    return Err(self.fail(&error, dir, Op::Read));
                }
                Err(error) => {
                    // Some servers take `-a` for a file name.
                    self.log_failure(&error);
                    self.logger().command("LIST");
                    self.list_lines(ftp, None).await.map_err(|error| {
                        self.log_failure(&error);
                        self.fail(&error, dir, Op::Read)
                    })?
                }
            }
        };
        let entries: Vec<Entry> = lines
            .iter()
            .filter_map(|line| {
                if self.inner.features.mlst {
                    parse_mlsx(line)
                } else {
                    parse_list_line(line, now)
                }
            })
            .filter(|listed| !listed.is_dot)
            .map(|listed| listed.into_entry(dir))
            .collect();
        self.logger().response(format!("{} entries", entries.len()));
        Ok(entries)
    }

    /// Removes Gem's own temp and backup siblings of `target` (exact names only) older than
    /// [`STALE_AFTER_SECS`]: leftovers of a save interrupted by a dropped connection. `keep` is
    /// never removed. Without MLST the age comes from `MDTM` (UTC), not from `LIST`'s local
    /// time. Failures are logged and ignored.
    async fn sweep_stale(&self, ftp: &mut Ftp, target: &str, keep: Option<&str>) {
        let dir = path::parent(target);
        let Ok(entries) = self.list_entries(ftp, &dir).await else {
            return;
        };
        let now = now_secs();
        for entry in entries {
            if !is_own_sibling(&entry.name, target) || Some(entry.path.as_str()) == keep {
                continue;
            }
            let mtime = if self.inner.features.mlst {
                entry.mtime
            } else {
                self.logger().command(format!("MDTM {}", entry.path));
                match ftp.mdtm(&entry.path).await {
                    Ok(time) => Some(time.and_utc().timestamp()),
                    Err(error) => {
                        self.log_failure(&error);
                        None
                    }
                }
            };
            if !mtime.is_some_and(|mtime| now - mtime > STALE_AFTER_SECS) {
                continue;
            }
            match self.delete(ftp, &entry.path).await {
                Ok(_) => self
                    .logger()
                    .status(format!("Removed the stale temp file {}", entry.path)),
                Err(_) => self.logger().error(format!(
                    "could not remove the stale temp file {}",
                    entry.path
                )),
            }
        }
    }

    async fn do_stat(&self, ftp: &mut Ftp, target: &str) -> AppResult<Entry> {
        if self.inner.features.mlst {
            self.logger().command(format!("MLST {target}"));
            let mut entry = match ftp.mlst(Some(target)).await {
                Ok(line) => {
                    self.logger().response(line.clone());
                    parse_mlsx(&line)
                        .map(|listed| listed.into_entry_at(target.to_owned()))
                        .ok_or_else(|| AppError::Remote(format!("unreadable MLST reply: {line}")))?
                }
                Err(error) => {
                    self.log_failure(&error);
                    return Err(self.classify(ftp, error, target).await);
                }
            };
            // ProFTPD's MLST follows a symlink for `type` but reports the link's own size;
            // SIZE always measures the file the path leads to.
            if entry.kind == EntryKind::File {
                self.logger().command(format!("SIZE {target}"));
                match ftp.size(target).await {
                    Ok(size) => {
                        self.logger().response(format!("213 {size}"));
                        entry.size = Some(size as u64);
                    }
                    Err(error) if is_fatal(&error) => {
                        self.log_failure(&error);
                        return Err(self.fail(&error, target, Op::Read));
                    }
                    Err(error) => self.log_failure(&error),
                }
            }
            return Ok(entry);
        }
        // No MLST: SIZE (files only) + MDTM, permissions and owner from the parent's listing.
        self.logger().command(format!("SIZE {target}"));
        let size = match ftp.size(target).await {
            Ok(size) => {
                self.logger().response(format!("213 {size}"));
                size as u64
            }
            Err(error) if is_fatal(&error) => {
                self.log_failure(&error);
                return Err(self.fail(&error, target, Op::Read));
            }
            Err(error) => {
                // Not a file (or not there): an expected answer while finding out which.
                self.log_probe_failure(&error);
                if self.probe_cwd(ftp, target).await.is_ok() {
                    return Ok(Entry {
                        name: path::file_name(target).to_owned(),
                        path: target.to_owned(),
                        kind: EntryKind::Dir,
                        size: None,
                        mtime: None,
                        mode: None,
                        owner: None,
                        group: None,
                        link_target: None,
                        target_kind: None,
                    });
                }
                return Err(self.classify(ftp, error, target).await);
            }
        };
        self.logger().command(format!("MDTM {target}"));
        let mut mtime = match ftp.mdtm(target).await {
            Ok(time) => {
                let time = time.and_utc().timestamp();
                self.logger().response(format!("213 {time}"));
                Some(time)
            }
            Err(error) => {
                self.log_failure(&error);
                None
            }
        };
        let listed = self.list_one(ftp, target).await?;
        let (mut mode, mut owner, mut group) = (None, None, None);
        if let Some(listed) = listed.filter(|listed| listed.kind != EntryKind::Symlink) {
            mode = listed.mode;
            owner = listed.owner;
            group = listed.group;
            mtime = mtime.or(listed.mtime);
        }
        Ok(Entry {
            name: path::file_name(target).to_owned(),
            path: target.to_owned(),
            kind: EntryKind::File,
            size: Some(size),
            mtime,
            mode,
            owner,
            group,
            link_target: None,
            target_kind: None,
        })
    }

    async fn do_read_range(
        &self,
        ftp: &mut Ftp,
        target: &str,
        offset: u64,
        len: u64,
    ) -> AppResult<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        if self.inner.features.rang
            && let Some(bytes) = self.read_rang(ftp, target, offset, len).await?
        {
            return Ok(bytes);
        }
        let mut stream = match self.open_retr(ftp, target, offset).await? {
            Some(stream) => stream,
            None => return Ok(Vec::new()),
        };
        let (out, at_end) = self.read_window(&mut stream, len).await?;
        if at_end {
            self.finish_download(stream, target).await?;
        } else {
            self.close_early(ftp, stream).await?;
        }
        self.logger()
            .response(format!("{} bytes read from {target}", out.len()));
        Ok(out)
    }

    /// `SIZE` + `RANG first last` + `RETR`: the server sends exactly the range and ends the
    /// transfer itself, so nothing is closed early (ProFTPD drops the whole session when a TLS
    /// data connection is closed while it is still sending). `None` when the server refuses
    /// the range; the caller falls back to `REST`.
    async fn read_rang(
        &self,
        ftp: &mut Ftp,
        target: &str,
        offset: u64,
        len: u64,
    ) -> AppResult<Option<Vec<u8>>> {
        self.logger().command(format!("SIZE {target}"));
        let size = match ftp.size(target).await {
            Ok(size) => {
                self.logger().response(format!("213 {size}"));
                size as u64
            }
            Err(error) => {
                self.log_failure(&error);
                if is_fatal(&error) {
                    return Err(self.fail(&error, target, Op::Read));
                }
                return Ok(None);
            }
        };
        if offset >= size {
            return Ok(Some(Vec::new()));
        }
        // RANG takes an inclusive end, which must not pass the end of the file.
        let last = offset.saturating_add(len).min(size) - 1;
        let mut attempts = 0;
        let opened = loop {
            // Sent again on a retry: the failed RETR may have consumed the range.
            match self
                .command(
                    ftp,
                    &format!("RANG {offset} {last}"),
                    &[Status::RequestFilePending],
                )
                .await
            {
                Ok(_) => {}
                Err(error) if is_fatal(&error) => {
                    return Err(self.fail(&error, target, Op::Read));
                }
                Err(_) => return Ok(None),
            }
            self.logger().command(format!("RETR {target}"));
            match ftp.retr_as_stream(target).await {
                Err(error) if self.retry_pause(&mut attempts, &error).await => {}
                other => break other,
            }
        };
        let mut stream = match opened {
            Ok(stream) => stream,
            Err(error) => {
                self.log_failure(&error);
                if is_fatal(&error) {
                    return Err(self.fail(&error, target, Op::Read));
                }
                return Err(self.classify(ftp, error, target).await);
            }
        };
        // Never trust the server to stop: ProFTPD 1.3.9 over TLS ignores the end of a range
        // longer than 64 KiB that does not start at 0 and sends the rest of the file.
        let (out, at_end) = self.read_window(&mut stream, last + 1 - offset).await?;
        if at_end {
            self.finish_download(stream, target).await?;
        } else {
            self.logger()
                .status("the server sent more than the requested range");
            self.close_early(ftp, stream).await?;
        }
        self.logger()
            .response(format!("{} bytes read from {target}", out.len()));
        Ok(Some(out))
    }

    /// Reads up to `len` bytes, then checks whether the transfer ended there. `true` when the
    /// data connection reached its end (the server finishes the transfer by itself).
    async fn read_window(
        &self,
        stream: &mut (impl AsyncRead + Unpin),
        len: u64,
    ) -> AppResult<(Vec<u8>, bool)> {
        let capacity = usize::try_from(len.min(64 * 1024 * 1024)).unwrap_or(0);
        let mut out = Vec::with_capacity(capacity);
        let mut buffer = vec![0u8; CHUNK];
        while (out.len() as u64) < len {
            let want = usize::try_from(len - out.len() as u64)
                .unwrap_or(CHUNK)
                .min(CHUNK);
            let read = self.timed_read(stream, &mut buffer[..want]).await?;
            if read == 0 {
                return Ok((out, true));
            }
            out.extend_from_slice(&buffer[..read]);
        }
        // One more read: 0 means the file (or range) ended exactly here.
        let read = self.timed_read(stream, &mut buffer[..1]).await?;
        Ok((out, read == 0))
    }

    /// `REST offset` (when not 0) + `RETR`. `None` when the offset is at or past the end.
    async fn open_retr(
        &self,
        ftp: &mut Ftp,
        target: &str,
        offset: u64,
    ) -> AppResult<Option<suppaftp::tokio::TransferStream<suppaftp::tokio::AsyncRustlsStream>>>
    {
        let rest = usize::try_from(offset)
            .map_err(|_| AppError::validation("offset too large for this platform"))?;
        let mut attempts = 0;
        let opened = loop {
            // Armed again on a retry: suppaftp consumes the offset even when RETR fails.
            if rest > 0 {
                ftp.resume_transfer(rest)
                    .await
                    .map_err(|error| self.fail(&error, target, Op::Read))?;
                self.logger().command(format!("REST {offset}"));
            }
            self.logger().command(format!("RETR {target}"));
            match ftp.retr_as_stream(target).await {
                Err(error) if self.retry_pause(&mut attempts, &error).await => {}
                other => break other,
            }
        };
        match opened {
            Ok(stream) => Ok(Some(stream)),
            Err(error) => {
                self.log_failure(&error);
                if is_fatal(&error) {
                    return Err(self.fail(&error, target, Op::Read));
                }
                // REST past the end is refused by some servers (554); that is an empty read.
                if offset > 0
                    && let Ok(size) = ftp.size(target).await
                    && offset >= size as u64
                {
                    return Ok(None);
                }
                Err(self.classify(ftp, error, target).await)
            }
        }
    }

    async fn timed_read(
        &self,
        stream: &mut (impl AsyncRead + Unpin),
        buffer: &mut [u8],
    ) -> AppResult<usize> {
        match tokio::time::timeout(self.inner.op_timeout, stream.read(buffer)).await {
            Ok(Ok(read)) => Ok(read),
            Ok(Err(error)) => {
                let mapped = from_io(&error);
                self.logger().error(format!("data connection: {error}"));
                Err(mapped)
            }
            Err(_) => {
                let error = AppError::Connection("the data connection stalled".into());
                self.logger().error(error.to_string());
                Err(error)
            }
        }
    }

    async fn finish_download(
        &self,
        stream: suppaftp::tokio::TransferStream<suppaftp::tokio::AsyncRustlsStream>,
        target: &str,
    ) -> AppResult<()> {
        stream.finish().await.map_err(|error| {
            self.log_failure(&error);
            self.fail(&error, target, Op::Read)
        })
    }

    async fn do_write_atomic(
        &self,
        ftp: &mut Ftp,
        target: &str,
        data: &[u8],
        mode: Option<u32>,
        allow_in_place: bool,
    ) -> AppResult<WriteOutcome> {
        self.sweep_stale(ftp, target, None).await;
        let temp = temp_sibling(target, &random_suffix()?);
        let mut guard = TempGuard::new(Arc::clone(&self.inner), temp.clone());
        match self.store(ftp, &temp, data).await {
            Ok(()) => {}
            Err(StoreError::Refused(error)) => {
                guard.disarm();
                let mapped = self.fail(&error, &temp, Op::Write);
                if !matches!(mapped, AppError::PermissionDenied(_)) {
                    return Err(mapped);
                }
                let dir = path::parent(target);
                if !allow_in_place {
                    let error = AppError::Unsupported(format!(
                        "the folder {dir} isn't writable, so {target} can't be saved safely \
                         (temp file + rename); it can only be overwritten in place"
                    ));
                    self.logger().error(error.to_string());
                    return Err(error);
                }
                self.logger().status(format!(
                    "{dir} is not writable; overwriting {target} in place (not atomic)"
                ));
                return match self.store(ftp, target, data).await {
                    Ok(()) => Ok(WriteOutcome { atomic: false }),
                    Err(StoreError::Refused(error) | StoreError::Failed(error)) => {
                        Err(self.fail(&error, target, Op::Write))
                    }
                };
            }
            Err(StoreError::Failed(error)) => {
                let mapped = self.fail(&error, target, Op::Write);
                if !is_fatal(&error) {
                    guard.disarm();
                    let _ = self.delete(ftp, &temp).await;
                }
                return Err(mapped);
            }
        }
        let result = async {
            if let Some(mode) = mode
                && self.inner.chmod.load(Ordering::Acquire)
            {
                let line = format!("SITE CHMOD {:o} {temp}", mode & 0o7777);
                match self.command(ftp, &line, &[Status::CommandOk]).await {
                    Ok(_) => {}
                    Err(error) if is_fatal(&error) => {
                        return Err(self.fail(&error, &temp, Op::Write));
                    }
                    Err(FtpError::UnexpectedResponse(response)) => {
                        // The contents are still saved; only the permissions could not be set.
                        if is_unsupported(reply::code(&response)) {
                            self.inner.chmod.store(false, Ordering::Release);
                        }
                        self.logger()
                            .error(format!("permissions of {target} could not be applied"));
                    }
                    Err(_) => {}
                }
            }
            self.move_into_place(ftp, &temp, target).await
        }
        .await;
        match result {
            // A backup dance still never shows a half-written file: temp file + rename.
            Ok(_) => {
                guard.disarm();
                self.logger()
                    .response(format!("saved {target} ({} bytes, atomic)", data.len()));
                Ok(WriteOutcome { atomic: true })
            }
            Err(error) => {
                if !matches!(error, AppError::Disconnected) {
                    guard.disarm();
                    let _ = self.delete(ftp, &temp).await;
                }
                Err(error)
            }
        }
    }

    async fn do_download(
        &self,
        ftp: &mut Ftp,
        source: &str,
        offset: u64,
        sink: &mut (dyn AsyncWrite + Send + Unpin),
        progress: Progress<'_>,
        cancel: &AtomicBool,
    ) -> AppResult<u64> {
        let Some(mut stream) = self.open_retr(ftp, source, offset).await? else {
            return Ok(0);
        };
        let mut buffer = vec![0u8; CHUNK];
        let mut total = 0u64;
        loop {
            if cancel.load(Ordering::Acquire) {
                // Some servers (ProFTPD over TLS) end the whole session when a download is cut
                // short; the download is cancelled either way.
                let _ = self.close_early(ftp, stream).await;
                self.logger()
                    .status(format!("download of {source} cancelled"));
                return Err(AppError::Cancelled);
            }
            let read = self.timed_read(&mut stream, &mut buffer).await?;
            if read == 0 {
                break;
            }
            if let Err(error) = sink.write_all(&buffer[..read]).await {
                let _ = self.close_early(ftp, stream).await;
                return Err(AppError::Platform(format!("local file error: {error}")));
            }
            total += read as u64;
            progress(total);
        }
        self.finish_download(stream, source).await?;
        sink.flush()
            .await
            .map_err(|error| AppError::Platform(format!("local file error: {error}")))?;
        self.logger()
            .response(format!("downloaded {total} bytes from {source}"));
        Ok(total)
    }

    async fn do_upload(
        &self,
        ftp: &mut Ftp,
        source: &mut (dyn AsyncRead + Send + Unpin),
        target: &str,
        progress: Progress<'_>,
        cancel: &AtomicBool,
    ) -> AppResult<u64> {
        self.logger().command(format!("STOR {target}"));
        let mut stream = match self.stor_stream(ftp, target).await {
            Ok(stream) => stream,
            Err(error) => {
                self.log_failure(&error);
                return Err(self.fail(&error, target, Op::Write));
            }
        };
        let mut buffer = vec![0u8; CHUNK];
        let mut total = 0u64;
        let outcome: AppResult<()> = loop {
            if cancel.load(Ordering::Acquire) {
                break Err(AppError::Cancelled);
            }
            let read = match source.read(&mut buffer).await {
                Ok(0) => break Ok(()),
                Ok(read) => read,
                Err(error) => break Err(AppError::Platform(format!("local file error: {error}"))),
            };
            match tokio::time::timeout(self.inner.op_timeout, stream.write_all(&buffer[..read]))
                .await
            {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    self.logger().error(format!("data connection: {error}"));
                    break Err(from_io(&error));
                }
                Err(_) => break Err(AppError::Connection("the data connection stalled".into())),
            }
            total += read as u64;
            progress(total);
        };
        // The server only learns that the upload ended when the data connection closes, so a
        // cancelled upload leaves a partial file (callers upload to a temp name; see ADR 0005).
        let finished = stream.finish().await;
        match (outcome, finished) {
            (Ok(()), Ok(())) => {
                self.logger()
                    .response(format!("uploaded {total} bytes to {target}"));
                Ok(total)
            }
            (Ok(()), Err(error)) => {
                self.log_failure(&error);
                Err(self.fail(&error, target, Op::Write))
            }
            (Err(AppError::Cancelled), _) => {
                self.logger()
                    .status(format!("upload to {target} cancelled"));
                Err(AppError::Cancelled)
            }
            (Err(error), _) => Err(error),
        }
    }

    /// Ends an operation. A timed-out one (`None`) leaves the connection marked for
    /// resynchronisation; anything else left it in step.
    fn settle<T>(
        &self,
        control: &mut MutexGuard<'_, Control>,
        outcome: Option<AppResult<T>>,
    ) -> AppResult<T> {
        match outcome {
            Some(result) => {
                control.needs_resync = false;
                result
            }
            None => {
                let error = AppError::Connection("the server did not answer in time".into());
                self.logger().error(error.to_string());
                Err(error)
            }
        }
    }

    /// QUIT and stop: every later call fails with `Disconnected`.
    pub async fn shutdown(&self) {
        if self.inner.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        // A transfer may hold the control connection: do not wait for it, cut the socket.
        let Ok(mut control) =
            tokio::time::timeout(Duration::from_millis(100), self.inner.control.lock()).await
        else {
            if let Ok(slot) = self.inner.kill_switch.lock()
                && let Some(socket) = slot.as_ref()
            {
                let _ = socket.shutdown(std::net::Shutdown::Both);
            }
            return;
        };
        if !self.inner.dead.load(Ordering::Acquire) {
            self.logger().command("QUIT");
            match tokio::time::timeout(Duration::from_secs(3), control.ftp.quit()).await {
                Ok(Ok(())) => self.logger().response("221 Goodbye"),
                Ok(Err(error)) => self.log_failure(&error),
                Err(_) => {}
            }
        }
        drop(control);
    }
}

/// Why a `STOR` failed: before the file was created, or while it was being written.
enum StoreError {
    Refused(FtpError),
    Failed(FtpError),
}

/// Sends `NOOP` and discards replies until its `200` arrives and the stream stays quiet, after
/// an operation was abandoned between a command and its reply.
async fn resync(ftp: &mut Ftp, logger: &Logger) -> FtpResult<()> {
    logger.status("Resynchronising the control connection");
    let attempt = async {
        logger.command("NOOP");
        let mut result = ftp.custom_command("NOOP", &[Status::CommandOk]).await;
        for _ in 0..8 {
            match result {
                Err(FtpError::UnexpectedResponse(_)) => {
                    result = ftp.read_response(Status::CommandOk).await;
                }
                _ => break,
            }
        }
        result?;
        // A stale 200 may have been taken for the NOOP's: wait until nothing more arrives.
        loop {
            let late = tokio::time::timeout(
                Duration::from_millis(300),
                ftp.read_response(Status::CommandOk),
            )
            .await;
            match late {
                Err(_) => return Ok(()),
                Ok(Ok(_) | Err(FtpError::UnexpectedResponse(_))) => {}
                Ok(Err(error)) => return Err(error),
            }
        }
    };
    match tokio::time::timeout(Duration::from_secs(10), attempt).await {
        Ok(result) => result,
        Err(_) => Err(FtpError::ConnectionError(
            std::io::ErrorKind::TimedOut.into(),
        )),
    }
}

/// Deletes the temp file of an atomic write whose future was dropped before it finished.
struct TempGuard {
    inner: Arc<Inner>,
    path: Option<String>,
}

impl TempGuard {
    fn new(inner: Arc<Inner>, path: String) -> Self {
        Self {
            inner,
            path: Some(path),
        }
    }

    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        let Some(temp) = self.path.take() else {
            return;
        };
        let fs = FtpFs {
            inner: Arc::clone(&self.inner),
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let Ok(mut control) = fs.lock().await else {
                    return;
                };
                let _ = fs.delete(&mut control.ftp, &temp).await;
                control.needs_resync = false;
            });
        }
    }
}

#[async_trait]
impl RemoteFs for FtpFs {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            exec: false,
            atomic_rename: self.inner.rnto_overwrites.load(Ordering::Acquire),
            chmod: self.inner.chmod.load(Ordering::Acquire),
            owner: self.inner.features.owner,
            symlinks: self.inner.features.unix,
        }
    }

    async fn home(&self) -> AppResult<String> {
        self.ensure_open()?;
        Ok(self.inner.home.clone())
    }

    async fn canonicalize(&self, target: &str) -> AppResult<String> {
        let mut control = self.lock().await?;
        let result = self
            .timed(async {
                let (fs, ftp) = (self, &mut control.ftp);
                if let Err(error) = fs.cwd(ftp, target).await {
                    return Err(fs.classify(ftp, error, target).await);
                }
                fs.logger().command("PWD");
                let resolved = ftp.pwd().await.map_err(|error| {
                    fs.log_failure(&error);
                    fs.fail(&error, target, Op::Read)
                })?;
                fs.logger().response(format!("257 \"{resolved}\""));
                Ok(resolved)
            })
            .await;
        self.settle(&mut control, result)
    }

    async fn list(&self, dir: &str) -> AppResult<Vec<Entry>> {
        let mut control = self.lock().await?;
        let result = self.timed(self.do_list(&mut control.ftp, dir)).await;
        self.settle(&mut control, result)
    }

    async fn stat(&self, target: &str) -> AppResult<Entry> {
        let mut control = self.lock().await?;
        let result = self.timed(self.do_stat(&mut control.ftp, target)).await;
        self.settle(&mut control, result)
    }

    async fn read_range(&self, target: &str, offset: u64, len: u64) -> AppResult<Vec<u8>> {
        let mut control = self.lock().await?;
        let result = self
            .do_read_range(&mut control.ftp, target, offset, len)
            .await;
        self.settle(&mut control, Some(result))
    }

    async fn write_atomic(
        &self,
        target: &str,
        data: &[u8],
        mode: Option<u32>,
        allow_in_place: bool,
    ) -> AppResult<WriteOutcome> {
        let mut control = self.lock().await?;
        let result = self
            .do_write_atomic(&mut control.ftp, target, data, mode, allow_in_place)
            .await;
        self.settle(&mut control, Some(result))
    }

    async fn replace(&self, temp: &str, target: &str) -> AppResult<WriteOutcome> {
        let mut control = self.lock().await?;
        let result = self
            .timed(async {
                let ftp = &mut control.ftp;
                if !self.exists(ftp, temp).await? {
                    let error = AppError::not_found(temp);
                    self.logger().error(error.to_string());
                    return Err(error);
                }
                self.sweep_stale(ftp, target, Some(temp)).await;
                let atomic = self.move_into_place(ftp, temp, target).await?;
                self.logger().response(format!(
                    "replaced {target} ({})",
                    if atomic {
                        "atomic"
                    } else {
                        "via a backup copy"
                    }
                ));
                Ok(WriteOutcome { atomic })
            })
            .await;
        self.settle(&mut control, result)
    }

    async fn create_file(&self, target: &str) -> AppResult<()> {
        let mut control = self.lock().await?;
        let result = self
            .timed(async {
                let (fs, ftp) = (self, &mut control.ftp);
                fs.ensure_absent(ftp, target).await?;
                match fs.store(ftp, target, &[]).await {
                    Ok(()) => Ok(()),
                    Err(StoreError::Refused(error) | StoreError::Failed(error)) => {
                        Err(fs.fail(&error, target, Op::Write))
                    }
                }
            })
            .await;
        self.settle(&mut control, result)
    }

    async fn mkdir(&self, target: &str) -> AppResult<()> {
        let mut control = self.lock().await?;
        let result = self
            .timed(async {
                let (fs, ftp) = (self, &mut control.ftp);
                // Some servers (ProFTPD) answer 257 to MKD of an existing folder.
                fs.ensure_absent(ftp, target).await?;
                fs.command(
                    ftp,
                    &format!("MKD {target}"),
                    &[
                        Status::PathCreated,
                        Status::RequestedFileActionOk,
                        Status::CommandOk,
                    ],
                )
                .await
                .map(|_| ())
                .map_err(|error| fs.fail(&error, target, Op::Write))
            })
            .await;
        self.settle(&mut control, result)
    }

    async fn rename(&self, from: &str, to: &str) -> AppResult<()> {
        let mut control = self.lock().await?;
        let result = self
            .timed(async {
                let (fs, ftp) = (self, &mut control.ftp);
                fs.ensure_absent(ftp, to).await?;
                if let Err(error) = fs
                    .command(ftp, &format!("RNFR {from}"), &[Status::RequestFilePending])
                    .await
                {
                    return Err(fs.classify(ftp, error, from).await);
                }
                fs.command(
                    ftp,
                    &format!("RNTO {to}"),
                    &[Status::RequestedFileActionOk, Status::CommandOk],
                )
                .await
                .map(|_| ())
                .map_err(|error| fs.fail(&error, to, Op::Write))
            })
            .await;
        self.settle(&mut control, result)
    }

    async fn remove_file(&self, target: &str) -> AppResult<()> {
        let mut control = self.lock().await?;
        let result = self
            .timed(async {
                let (fs, ftp) = (self, &mut control.ftp);
                match fs.delete(ftp, target).await {
                    Ok(_) => Ok(()),
                    Err(error) => Err(fs.classify(ftp, error, target).await),
                }
            })
            .await;
        self.settle(&mut control, result)
    }

    async fn remove_dir(&self, target: &str) -> AppResult<()> {
        let mut control = self.lock().await?;
        let result = self
            .timed(async {
                let (fs, ftp) = (self, &mut control.ftp);
                match fs
                    .command(
                        ftp,
                        &format!("RMD {target}"),
                        &[Status::RequestedFileActionOk, Status::CommandOk],
                    )
                    .await
                {
                    Ok(_) => Ok(()),
                    Err(error) => match fs.classify(ftp, error, target).await {
                        // vsftpd says neither "not empty" nor "permission denied".
                        AppError::PermissionDenied(_) => Err(AppError::Remote(format!(
                            "{target} could not be removed (is the folder empty?)"
                        ))),
                        other => Err(other),
                    },
                }
            })
            .await;
        self.settle(&mut control, result)
    }

    async fn chmod(&self, target: &str, mode: u32) -> AppResult<()> {
        if !self.inner.chmod.load(Ordering::Acquire) {
            return Err(AppError::Unsupported(
                "this server does not support changing permissions".into(),
            ));
        }
        let mut control = self.lock().await?;
        let result = self
            .timed(async {
                let (fs, ftp) = (self, &mut control.ftp);
                let line = format!("SITE CHMOD {:o} {target}", mode & 0o7777);
                match fs.command(ftp, &line, &[Status::CommandOk]).await {
                    Ok(_) => Ok(()),
                    Err(FtpError::UnexpectedResponse(response))
                        if is_unsupported(reply::code(&response)) =>
                    {
                        fs.inner.chmod.store(false, Ordering::Release);
                        Err(AppError::Unsupported(
                            "this server does not support changing permissions".into(),
                        ))
                    }
                    Err(error) => Err(fs.classify(ftp, error, target).await),
                }
            })
            .await;
        self.settle(&mut control, result)
    }

    async fn download(
        &self,
        source: &str,
        offset: u64,
        sink: &mut (dyn AsyncWrite + Send + Unpin),
        progress: Progress<'_>,
        cancel: &AtomicBool,
    ) -> AppResult<u64> {
        let mut control = self.lock().await?;
        let result = self
            .do_download(&mut control.ftp, source, offset, sink, progress, cancel)
            .await;
        self.settle(&mut control, Some(result))
    }

    async fn upload(
        &self,
        source: &mut (dyn AsyncRead + Send + Unpin),
        target: &str,
        progress: Progress<'_>,
        cancel: &AtomicBool,
    ) -> AppResult<u64> {
        let mut control = self.lock().await?;
        let result = self
            .do_upload(&mut control.ftp, source, target, progress, cancel)
            .await;
        self.settle(&mut control, Some(result))
    }

    async fn keepalive(&self) -> AppResult<()> {
        let mut control = self.lock().await?;
        let result = self
            .timed(async {
                let (fs, ftp) = (self, &mut control.ftp);
                ftp.custom_command("NOOP", &[Status::CommandOk])
                    .await
                    .map(|_| ())
                    .map_err(|error| {
                        fs.log_failure(&error);
                        fs.fail(&error, "", Op::Read)
                    })
            })
            .await;
        self.settle(&mut control, result)
    }

    async fn close(&self) {
        self.shutdown().await;
    }
}
