//! An in-memory [`RemoteFs`] and [`Connection`] for service tests (no network).
//!
//! It follows POSIX semantics closely enough for the services to be tested against it:
//! `NotFound` / `Conflict` / `PermissionDenied`, permission bits (a directory without the owner
//! write bit refuses new entries, a file without it refuses writes), modification times that
//! advance on every change, and symlinks (followed by `stat`, `list` and reads; not followed
//! by `rename`, `remove_*` or `write_atomic`, which replace the link itself like a real server).
//!
//! Fault injection hooks on [`MemoryServer`] let tests fail the next write or rename, break an
//! upload after N bytes, slow reads down (for cancel tests), refuse secondary channels and drop
//! the whole connection. [`MemoryServer::enable_shell`] adds a scripted [`RemoteShell`]
//! (canned output, exit codes, slow or endless streaming) for the remote-command tests.

#![cfg(test)]

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::path::{file_name, join, normalize, parent};
use super::{Connection, OutputSink, Progress, RemoteFs, RemoteShell, WriteOutcome};
use crate::domain::command::{CommandExit, CommandStream};
use crate::domain::entry::{Entry, EntryKind};
use crate::domain::session::Capabilities;
use crate::error::{AppError, AppResult};

/// Size of the chunks streamed by `download` and `upload`.
const CHUNK: usize = 16 * 1024;
const MAX_LINKS: usize = 16;

#[derive(Debug, Clone)]
enum Data {
    File(Vec<u8>),
    Dir,
    Link(String),
}

#[derive(Debug, Clone)]
struct Node {
    data: Data,
    mode: u32,
    mtime: i64,
}

#[derive(Debug, Default)]
struct Faults {
    fail_writes: u32,
    fail_renames: u32,
    fail_replaces: u32,
    /// `replace` answers `Unsupported`, like a backend that does not implement it.
    no_replace: bool,
    fail_upload_after: Option<u64>,
    read_delay: Option<Duration>,
    fail_secondaries: u32,
}

/// The shared state of a fake server: its tree, clock, faults and counters. Every connection
/// and channel opened on it sees the same files.
pub struct MemoryServer {
    tree: Mutex<BTreeMap<String, Node>>,
    faults: Mutex<Faults>,
    clock: AtomicI64,
    alive: AtomicBool,
    active: AtomicUsize,
    max_active: AtomicUsize,
    secondaries: AtomicUsize,
    home: String,
    /// `Some` when the server offers a shell (like SSH); the scripts by command.
    shell: Mutex<Option<HashMap<String, Script>>>,
    /// Every command run, as received.
    executed: Mutex<Vec<String>>,
    /// Open command channels, and the most the server allows (`MaxSessions`).
    open_execs: AtomicUsize,
    max_execs: Mutex<Option<usize>>,
}

/// What the fake shell does for one command.
#[derive(Debug, Clone, Default)]
pub struct Script {
    /// Chunks sent in order (stream, bytes).
    pub output: Vec<(CommandStream, Vec<u8>)>,
    /// Wait before each chunk.
    pub delay: Option<Duration>,
    /// Repeat the output until cancelled (`tail -f`).
    pub endless: bool,
    pub code: Option<u32>,
    /// Fail with a connection error after the output.
    pub fail: Option<String>,
    /// Ignores `TERM`: a stop ends with the channel closed (no exit status, no signal).
    pub ignores_term: bool,
}

impl Script {
    pub fn stdout(text: &str, code: u32) -> Self {
        Self {
            output: vec![(CommandStream::Stdout, text.as_bytes().to_vec())],
            code: Some(code),
            ..Self::default()
        }
    }
}

impl MemoryServer {
    /// A server with `/`, `/home` and the home directory `/home/user` (all writable).
    pub fn new() -> Arc<Self> {
        let server = Arc::new(Self {
            tree: Mutex::new(BTreeMap::new()),
            faults: Mutex::new(Faults::default()),
            clock: AtomicI64::new(1_700_000_000),
            alive: AtomicBool::new(true),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            secondaries: AtomicUsize::new(0),
            home: "/home/user".into(),
            shell: Mutex::new(None),
            executed: Mutex::new(Vec::new()),
            open_execs: AtomicUsize::new(0),
            max_execs: Mutex::new(None),
        });
        server.add_dir("/");
        server.add_dir("/home");
        server.add_dir("/home/user");
        server
    }

    /// A new, authenticated connection to this server.
    pub fn connect(self: &Arc<Self>) -> Arc<MemoryConnection> {
        let closed = Arc::new(AtomicBool::new(false));
        Arc::new(MemoryConnection {
            primary: Arc::new(MemoryFs::new(Arc::clone(self), Arc::clone(&closed))),
            server: Arc::clone(self),
            closed,
        })
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, Node>> {
        self.tree.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn faults(&self) -> MutexGuard<'_, Faults> {
        self.faults.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn tick(&self) -> i64 {
        self.clock.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn insert(&self, path: &str, data: Data, mode: u32) {
        let mtime = self.tick();
        self.lock()
            .insert(normalize("/", path), Node { data, mode, mtime });
    }

    // ----- Test setup and inspection -------------------------------------------------------

    pub fn add_dir(&self, path: &str) {
        self.insert(path, Data::Dir, 0o755);
    }

    pub fn add_file(&self, path: &str, bytes: &[u8]) {
        self.insert(path, Data::File(bytes.to_vec()), 0o644);
    }

    pub fn add_link(&self, path: &str, target: &str) {
        self.insert(path, Data::Link(target.to_owned()), 0o777);
    }

    pub fn set_mode(&self, path: &str, mode: u32) {
        if let Some(node) = self.lock().get_mut(&normalize("/", path)) {
            node.mode = mode;
        }
    }

    /// Simulates a change made by someone else: new contents and a new mtime.
    pub fn replace_file(&self, path: &str, bytes: &[u8]) {
        let mtime = self.tick();
        if let Some(node) = self.lock().get_mut(&normalize("/", path)) {
            node.data = Data::File(bytes.to_vec());
            node.mtime = mtime;
        }
    }

    pub fn file(&self, path: &str) -> Option<Vec<u8>> {
        match self.lock().get(&normalize("/", path)).map(|n| &n.data) {
            Some(Data::File(bytes)) => Some(bytes.clone()),
            _ => None,
        }
    }

    pub fn mode(&self, path: &str) -> Option<u32> {
        self.lock().get(&normalize("/", path)).map(|n| n.mode)
    }

    pub fn exists(&self, path: &str) -> bool {
        self.lock().contains_key(&normalize("/", path))
    }

    /// Every path on the server, sorted (to assert that no temp file was left behind).
    pub fn paths(&self) -> Vec<String> {
        self.lock().keys().cloned().collect()
    }

    // ----- Fault injection -----------------------------------------------------------------

    /// The next `n` writes (`write_atomic`, `create_file`, `upload`) fail.
    pub fn fail_next_writes(&self, n: u32) {
        self.faults().fail_writes = n;
    }

    /// The next rename (including the one inside `write_atomic`) fails.
    pub fn fail_next_rename(&self) {
        self.faults().fail_renames = 1;
    }

    /// The next `replace` fails (the target and the temp file are left as they were).
    pub fn fail_next_replace(&self) {
        self.faults().fail_replaces = 1;
    }

    /// Makes `replace` unsupported (a protocol without it).
    pub fn disable_replace(&self) {
        self.faults().no_replace = true;
    }

    /// Uploads break with a connection error once they have written `bytes`.
    pub fn fail_uploads_after(&self, bytes: Option<u64>) {
        self.faults().fail_upload_after = bytes;
    }

    /// Every chunk of a read or download waits this long (lets tests cancel mid-transfer).
    pub fn slow_reads(&self, delay: Option<Duration>) {
        self.faults().read_delay = delay;
    }

    /// The next `n` calls to `open_secondary` fail.
    pub fn fail_next_secondaries(&self, n: u32) {
        self.faults().fail_secondaries = n;
    }

    /// Breaks the transport: every channel of every connection fails from now on.
    pub fn drop_connection(&self) {
        self.alive.store(false, Ordering::SeqCst);
    }

    /// Brings the server back after [`Self::drop_connection`] (new connections work again).
    pub fn restore(&self) {
        self.alive.store(true, Ordering::SeqCst);
    }

    /// Whether the transport is up (see [`Self::drop_connection`]).
    pub fn is_up(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Offers a shell (`Connection::shell` returns `Some`), with no scripts yet.
    pub fn enable_shell(&self) {
        let mut shell = self.shell.lock().unwrap_or_else(|e| e.into_inner());
        shell.get_or_insert_with(HashMap::new);
    }

    /// What the shell does for `command` (exactly as sent). Unknown commands print
    /// "not found" on stderr and exit with 127.
    pub fn script(&self, command: &str, script: Script) {
        let mut shell = self.shell.lock().unwrap_or_else(|e| e.into_inner());
        shell
            .get_or_insert_with(HashMap::new)
            .insert(command.to_owned(), script);
    }

    /// At most `n` commands at once; more are refused like OpenSSH's `MaxSessions`.
    pub fn limit_execs(&self, n: usize) {
        *self.max_execs.lock().unwrap_or_else(|e| e.into_inner()) = Some(n);
    }

    /// Every command the shell received.
    pub fn executed(&self) -> Vec<String> {
        self.executed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Most streaming operations (downloads and uploads) seen running at the same time.
    pub fn max_active(&self) -> usize {
        self.max_active.load(Ordering::SeqCst)
    }

    /// How many secondary channels were opened.
    pub fn secondaries_opened(&self) -> usize {
        self.secondaries.load(Ordering::SeqCst)
    }

    fn take_fault(&self, pick: impl FnOnce(&mut Faults) -> &mut u32) -> bool {
        let mut faults = self.faults();
        let counter = pick(&mut faults);
        if *counter > 0 {
            *counter -= 1;
            true
        } else {
            false
        }
    }

    fn read_delay(&self) -> Option<Duration> {
        self.faults().read_delay
    }
}

/// Counts streaming operations in flight (for the concurrency-limit tests).
struct ActiveGuard<'a>(&'a MemoryServer);

impl<'a> ActiveGuard<'a> {
    fn new(server: &'a MemoryServer) -> Self {
        let now = server.active.fetch_add(1, Ordering::SeqCst) + 1;
        server.max_active.fetch_max(now, Ordering::SeqCst);
        Self(server)
    }
}

impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Follows symlinks in `path` (the last component only when `follow_last`).
fn resolve(
    tree: &BTreeMap<String, Node>,
    path: &str,
    follow_last: bool,
    depth: usize,
) -> AppResult<String> {
    if depth > MAX_LINKS {
        return Err(AppError::Remote(format!(
            "{path}: too many levels of symbolic links"
        )));
    }
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    let mut current = String::from("/");
    for (index, part) in parts.iter().enumerate() {
        let candidate = join(&current, part);
        let last = index + 1 == parts.len();
        match tree.get(&candidate).map(|n| &n.data) {
            Some(Data::Link(target)) if !last || follow_last => {
                let target = normalize(&current, target);
                current = resolve(tree, &target, true, depth + 1)?;
            }
            _ => current = candidate,
        }
    }
    Ok(current)
}

fn not_found(path: &str) -> AppError {
    AppError::not_found(path.to_owned())
}

fn denied(path: &str) -> AppError {
    AppError::PermissionDenied(path.to_owned())
}

/// The parent of `resolved` must be an existing directory the user may write to.
fn check_parent_writable(tree: &BTreeMap<String, Node>, resolved: &str) -> AppResult<()> {
    let dir = parent(resolved);
    match tree.get(&dir) {
        Some(Node {
            data: Data::Dir,
            mode,
            ..
        }) => {
            if mode & 0o200 == 0 {
                Err(denied(&dir))
            } else {
                Ok(())
            }
        }
        Some(_) => Err(AppError::Remote(format!("{dir} is not a folder"))),
        None => Err(not_found(&dir)),
    }
}

fn kind_of(data: &Data) -> EntryKind {
    match data {
        Data::File(_) => EntryKind::File,
        Data::Dir => EntryKind::Dir,
        Data::Link(_) => EntryKind::Symlink,
    }
}

fn entry(path: &str, node: &Node) -> Entry {
    Entry {
        name: file_name(path).to_owned(),
        path: path.to_owned(),
        kind: kind_of(&node.data),
        size: match &node.data {
            Data::File(bytes) => Some(bytes.len() as u64),
            Data::Dir => None,
            Data::Link(target) => Some(target.len() as u64),
        },
        mtime: Some(node.mtime),
        mode: Some(node.mode & 0o7777),
        owner: Some("user".into()),
        group: Some("user".into()),
        link_target: match &node.data {
            Data::Link(target) => Some(target.clone()),
            _ => None,
        },
        target_kind: None,
    }
}

/// One channel on a [`MemoryServer`].
pub struct MemoryFs {
    server: Arc<MemoryServer>,
    connection_closed: Arc<AtomicBool>,
    closed: AtomicBool,
}

impl MemoryFs {
    fn new(server: Arc<MemoryServer>, connection_closed: Arc<AtomicBool>) -> Self {
        Self {
            server,
            connection_closed,
            closed: AtomicBool::new(false),
        }
    }

    fn check(&self) -> AppResult<()> {
        if self.closed.load(Ordering::SeqCst) || self.connection_closed.load(Ordering::SeqCst) {
            return Err(AppError::Disconnected);
        }
        if !self.server.alive.load(Ordering::SeqCst) {
            return Err(AppError::Connection("connection reset by peer".into()));
        }
        Ok(())
    }

    fn injected_write_failure(&self) -> AppResult<()> {
        if self.server.take_fault(|f| &mut f.fail_writes) {
            return Err(AppError::Remote("write failed (injected)".into()));
        }
        Ok(())
    }

    fn read_file(&self, path: &str) -> AppResult<Vec<u8>> {
        let tree = self.server.lock();
        let resolved = resolve(&tree, &normalize("/", path), true, 0)?;
        match tree.get(&resolved) {
            Some(Node {
                data: Data::File(bytes),
                mode,
                ..
            }) => {
                if mode & 0o400 == 0 {
                    Err(denied(path))
                } else {
                    Ok(bytes.clone())
                }
            }
            Some(_) => Err(AppError::Remote(format!("{path} is not a file"))),
            None => Err(not_found(path)),
        }
    }
}

#[async_trait]
impl RemoteFs for MemoryFs {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            exec: false,
            atomic_rename: true,
            chmod: true,
            owner: true,
            symlinks: true,
        }
    }

    async fn home(&self) -> AppResult<String> {
        self.check()?;
        Ok(self.server.home.clone())
    }

    async fn canonicalize(&self, path: &str) -> AppResult<String> {
        self.check()?;
        let tree = self.server.lock();
        let resolved = resolve(&tree, &normalize("/", path), true, 0)?;
        match tree.get(&resolved).map(|n| &n.data) {
            Some(Data::Dir) => Ok(resolved),
            Some(_) => Err(AppError::Remote(format!("{path} is not a folder"))),
            None => Err(not_found(path)),
        }
    }

    async fn list(&self, path: &str) -> AppResult<Vec<Entry>> {
        self.check()?;
        let tree = self.server.lock();
        let shown = normalize("/", path);
        let resolved = resolve(&tree, &shown, true, 0)?;
        match tree.get(&resolved) {
            Some(Node {
                data: Data::Dir,
                mode,
                ..
            }) if mode & 0o400 == 0 => return Err(denied(path)),
            Some(Node {
                data: Data::Dir, ..
            }) => {}
            Some(_) => return Err(AppError::Remote(format!("{path} is not a folder"))),
            None => return Err(not_found(path)),
        }
        let mut entries = Vec::new();
        for (child, node) in tree.iter() {
            if child == "/" || parent(child) != resolved {
                continue;
            }
            let mut item = entry(&join(&shown, file_name(child)), node);
            if let Data::Link(_) = node.data {
                item.target_kind = resolve(&tree, child, true, 0)
                    .ok()
                    .and_then(|target| tree.get(&target).map(|n| kind_of(&n.data)));
            }
            entries.push(item);
        }
        Ok(entries)
    }

    async fn stat(&self, path: &str) -> AppResult<Entry> {
        self.check()?;
        let tree = self.server.lock();
        let shown = normalize("/", path);
        let resolved = resolve(&tree, &shown, true, 0)?;
        let node = tree.get(&resolved).ok_or_else(|| not_found(path))?;
        Ok(entry(&shown, node))
    }

    async fn read_range(&self, path: &str, offset: u64, len: u64) -> AppResult<Vec<u8>> {
        self.check()?;
        if let Some(delay) = self.server.read_delay() {
            tokio::time::sleep(delay).await;
        }
        let bytes = self.read_file(path)?;
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        let end = start
            .saturating_add(usize::try_from(len).unwrap_or(usize::MAX))
            .min(bytes.len());
        Ok(bytes[start..end].to_vec())
    }

    async fn write_atomic(
        &self,
        path: &str,
        data: &[u8],
        mode: Option<u32>,
        allow_in_place: bool,
    ) -> AppResult<WriteOutcome> {
        self.check()?;
        let mtime = self.server.tick();
        let mut tree = self.server.lock();
        let target = resolve(&tree, &normalize("/", path), false, 0)?;
        let existing = tree.get(&target).cloned();
        if matches!(existing.as_ref().map(|n| &n.data), Some(Data::Dir)) {
            return Err(AppError::Remote(format!("{path} is a folder")));
        }
        if let Err(error) = check_parent_writable(&tree, &target) {
            if !matches!(error, AppError::PermissionDenied(_)) {
                return Err(error);
            }
            // No temp file possible: overwrite in place only when the user agreed.
            let Some(existing) = existing else {
                return Err(error);
            };
            if !allow_in_place {
                return Err(AppError::Unsupported(
                    "the folder does not allow a temporary file; the file can only be \
                     overwritten in place"
                        .into(),
                ));
            }
            if existing.mode & 0o200 == 0 {
                return Err(denied(path));
            }
            drop(tree);
            self.injected_write_failure()?;
            let mut tree = self.server.lock();
            if let Some(node) = tree.get_mut(&target) {
                node.data = Data::File(data.to_vec());
                node.mtime = mtime;
            }
            return Ok(WriteOutcome { atomic: false });
        }
        let temp = join(
            &parent(&target),
            &format!(".{}.gem-{mtime}", file_name(&target)),
        );
        let mode = mode.or(existing.as_ref().map(|n| n.mode)).unwrap_or(0o644);
        tree.insert(
            temp.clone(),
            Node {
                data: Data::File(data.to_vec()),
                mode,
                mtime,
            },
        );
        drop(tree);
        let failed = self.injected_write_failure().err().or_else(|| {
            self.server
                .take_fault(|f| &mut f.fail_renames)
                .then(|| AppError::Remote("rename failed (injected)".into()))
        });
        let mut tree = self.server.lock();
        if let Some(error) = failed {
            tree.remove(&temp);
            return Err(error);
        }
        if let Some(node) = tree.remove(&temp) {
            tree.insert(target, node);
        }
        Ok(WriteOutcome { atomic: true })
    }

    async fn create_file(&self, path: &str) -> AppResult<()> {
        self.check()?;
        let mtime = self.server.tick();
        self.injected_write_failure()?;
        let mut tree = self.server.lock();
        let target = resolve(&tree, &normalize("/", path), false, 0)?;
        if tree.contains_key(&target) {
            return Err(AppError::Conflict(format!(
                "{} already exists",
                file_name(path)
            )));
        }
        check_parent_writable(&tree, &target)?;
        tree.insert(
            target,
            Node {
                data: Data::File(Vec::new()),
                mode: 0o644,
                mtime,
            },
        );
        Ok(())
    }

    async fn mkdir(&self, path: &str) -> AppResult<()> {
        self.check()?;
        let mtime = self.server.tick();
        let mut tree = self.server.lock();
        let target = resolve(&tree, &normalize("/", path), false, 0)?;
        if tree.contains_key(&target) {
            return Err(AppError::Conflict(format!(
                "{} already exists",
                file_name(path)
            )));
        }
        check_parent_writable(&tree, &target)?;
        tree.insert(
            target,
            Node {
                data: Data::Dir,
                mode: 0o755,
                mtime,
            },
        );
        Ok(())
    }

    async fn rename(&self, from: &str, to: &str) -> AppResult<()> {
        self.check()?;
        if self.server.take_fault(|f| &mut f.fail_renames) {
            return Err(AppError::Remote("rename failed (injected)".into()));
        }
        let mut tree = self.server.lock();
        let source = resolve(&tree, &normalize("/", from), false, 0)?;
        let target = resolve(&tree, &normalize("/", to), false, 0)?;
        if !tree.contains_key(&source) {
            return Err(not_found(from));
        }
        if tree.contains_key(&target) {
            return Err(AppError::Conflict(format!(
                "{} already exists",
                file_name(to)
            )));
        }
        if target.starts_with(&format!("{source}/")) {
            return Err(AppError::validation("cannot move a folder into itself"));
        }
        check_parent_writable(&tree, &source)?;
        check_parent_writable(&tree, &target)?;
        let prefix = format!("{source}/");
        let moved: Vec<String> = tree
            .keys()
            .filter(|k| *k == &source || k.starts_with(&prefix))
            .cloned()
            .collect();
        for key in moved {
            if let Some(node) = tree.remove(&key) {
                let new_key = format!("{target}{}", &key[source.len()..]);
                tree.insert(new_key, node);
            }
        }
        Ok(())
    }

    async fn replace(&self, temp: &str, target: &str) -> AppResult<WriteOutcome> {
        self.check()?;
        if self.server.faults().no_replace {
            return Err(AppError::Unsupported(
                "replace is not implemented for this protocol".into(),
            ));
        }
        if self.server.take_fault(|f| &mut f.fail_replaces) {
            return Err(AppError::Remote("replace failed (injected)".into()));
        }
        let mut tree = self.server.lock();
        let source = resolve(&tree, &normalize("/", temp), false, 0)?;
        let target = resolve(&tree, &normalize("/", target), false, 0)?;
        match tree.get(&source).map(|n| &n.data) {
            Some(Data::File(_)) => {}
            Some(_) => return Err(AppError::Remote(format!("{temp} is not a file"))),
            None => return Err(not_found(temp)),
        }
        if matches!(tree.get(&target).map(|n| &n.data), Some(Data::Dir)) {
            return Err(AppError::Remote(format!(
                "{} is a folder",
                file_name(&target)
            )));
        }
        check_parent_writable(&tree, &source)?;
        check_parent_writable(&tree, &target)?;
        // One step under the lock: readers see the old file or the new one, never neither.
        if let Some(node) = tree.remove(&source) {
            tree.insert(target, node);
        }
        Ok(WriteOutcome { atomic: true })
    }

    async fn remove_file(&self, path: &str) -> AppResult<()> {
        self.check()?;
        let mut tree = self.server.lock();
        let target = resolve(&tree, &normalize("/", path), false, 0)?;
        match tree.get(&target).map(|n| &n.data) {
            None => return Err(not_found(path)),
            Some(Data::Dir) => return Err(AppError::Remote(format!("{path} is a folder"))),
            Some(_) => {}
        }
        check_parent_writable(&tree, &target)?;
        tree.remove(&target);
        Ok(())
    }

    async fn remove_dir(&self, path: &str) -> AppResult<()> {
        self.check()?;
        let mut tree = self.server.lock();
        let target = resolve(&tree, &normalize("/", path), false, 0)?;
        match tree.get(&target).map(|n| &n.data) {
            None => return Err(not_found(path)),
            Some(Data::Dir) => {}
            Some(_) => return Err(AppError::Remote(format!("{path} is not a folder"))),
        }
        if tree.keys().any(|k| k != &target && parent(k) == target) {
            return Err(AppError::Remote(format!("{path}: folder not empty")));
        }
        check_parent_writable(&tree, &target)?;
        tree.remove(&target);
        Ok(())
    }

    async fn chmod(&self, path: &str, mode: u32) -> AppResult<()> {
        self.check()?;
        let mut tree = self.server.lock();
        let target = resolve(&tree, &normalize("/", path), true, 0)?;
        let node = tree.get_mut(&target).ok_or_else(|| not_found(path))?;
        node.mode = mode & 0o7777;
        Ok(())
    }

    async fn download(
        &self,
        path: &str,
        offset: u64,
        sink: &mut (dyn AsyncWrite + Send + Unpin),
        progress: Progress<'_>,
        cancel: &AtomicBool,
    ) -> AppResult<u64> {
        self.check()?;
        let _active = ActiveGuard::new(&self.server);
        let bytes = self.read_file(path)?;
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        let mut total = 0u64;
        for chunk in bytes[start..].chunks(CHUNK) {
            if cancel.load(Ordering::SeqCst) {
                return Err(AppError::Cancelled);
            }
            if let Some(delay) = self.server.read_delay() {
                tokio::time::sleep(delay).await;
            }
            self.check()?;
            sink.write_all(chunk).await?;
            total += chunk.len() as u64;
            progress(total);
        }
        sink.flush().await?;
        Ok(total)
    }

    async fn upload(
        &self,
        source: &mut (dyn AsyncRead + Send + Unpin),
        path: &str,
        progress: Progress<'_>,
        cancel: &AtomicBool,
    ) -> AppResult<u64> {
        self.check()?;
        let _active = ActiveGuard::new(&self.server);
        self.injected_write_failure()?;
        let mtime = self.server.tick();
        let target = {
            let mut tree = self.server.lock();
            let target = resolve(&tree, &normalize("/", path), false, 0)?;
            let mode = match tree.get(&target) {
                Some(Node {
                    data: Data::Dir, ..
                }) => return Err(AppError::Remote(format!("{path} is a folder"))),
                Some(node) if node.mode & 0o200 == 0 => return Err(denied(path)),
                Some(node) => node.mode,
                None => {
                    check_parent_writable(&tree, &target)?;
                    0o644
                }
            };
            tree.insert(
                target.clone(),
                Node {
                    data: Data::File(Vec::new()),
                    mode,
                    mtime,
                },
            );
            target
        };
        let fail_after = self.server.faults().fail_upload_after;
        let mut buffer = vec![0u8; CHUNK];
        let mut total = 0u64;
        loop {
            if cancel.load(Ordering::SeqCst) {
                return Err(AppError::Cancelled);
            }
            if fail_after.is_some_and(|limit| total >= limit) {
                return Err(AppError::Connection(
                    "connection lost during upload (injected)".into(),
                ));
            }
            self.check()?;
            let read = source.read(&mut buffer).await?;
            if read == 0 {
                break;
            }
            if let Some(delay) = self.server.read_delay() {
                tokio::time::sleep(delay).await;
            }
            let mut tree = self.server.lock();
            if let Some(Node {
                data: Data::File(bytes),
                ..
            }) = tree.get_mut(&target)
            {
                bytes.extend_from_slice(&buffer[..read]);
            }
            drop(tree);
            total += read as u64;
            progress(total);
        }
        Ok(total)
    }

    async fn keepalive(&self) -> AppResult<()> {
        self.check()
    }

    async fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

/// A connection to a [`MemoryServer`].
pub struct MemoryConnection {
    server: Arc<MemoryServer>,
    primary: Arc<MemoryFs>,
    closed: Arc<AtomicBool>,
}

#[async_trait]
impl Connection for MemoryConnection {
    fn fs(&self) -> Arc<dyn RemoteFs> {
        Arc::clone(&self.primary) as Arc<dyn RemoteFs>
    }

    async fn open_secondary(&self) -> AppResult<Arc<dyn RemoteFs>> {
        self.primary.check()?;
        if self.server.take_fault(|f| &mut f.fail_secondaries) {
            return Err(AppError::Connection(
                "could not open a channel (injected)".into(),
            ));
        }
        self.server.secondaries.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(MemoryFs::new(
            Arc::clone(&self.server),
            Arc::clone(&self.closed),
        )))
    }

    fn server_info(&self) -> Option<String> {
        Some("MemoryFS 1.0".into())
    }

    fn shell(&self) -> Option<Arc<dyn RemoteShell>> {
        let enabled = self
            .server
            .shell
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        enabled.then(|| {
            Arc::new(MemoryShell {
                server: Arc::clone(&self.server),
                closed: Arc::clone(&self.closed),
            }) as Arc<dyn RemoteShell>
        })
    }

    fn is_alive(&self) -> bool {
        self.server.alive.load(Ordering::SeqCst) && !self.closed.load(Ordering::SeqCst)
    }

    async fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

/// Counts an open command channel.
struct ExecGuard<'a>(&'a MemoryServer);

impl Drop for ExecGuard<'_> {
    fn drop(&mut self) {
        self.0.open_execs.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The scripted shell of a [`MemoryServer`].
struct MemoryShell {
    server: Arc<MemoryServer>,
    closed: Arc<AtomicBool>,
}

impl MemoryShell {
    fn check(&self) -> AppResult<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(AppError::Disconnected);
        }
        if !self.server.is_up() {
            return Err(AppError::Connection("connection reset by peer".into()));
        }
        Ok(())
    }
}

#[async_trait]
impl RemoteShell for MemoryShell {
    async fn exec(
        &self,
        command: &str,
        output: OutputSink<'_>,
        cancel: &AtomicBool,
    ) -> AppResult<CommandExit> {
        self.check()?;
        let limit = *self
            .server
            .max_execs
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let open = self.server.open_execs.fetch_add(1, Ordering::SeqCst) + 1;
        let _open = ExecGuard(&self.server);
        if limit.is_some_and(|limit| open > limit) {
            // What russh reports when the server refuses the channel.
            return Err(AppError::Connection(
                "SSH error: Failed to open channel (AdministrativelyProhibited)".into(),
            ));
        }
        self.server
            .executed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(command.to_owned());
        let script = self
            .server
            .shell
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .and_then(|scripts| scripts.get(command).cloned())
            .unwrap_or_else(|| Script {
                output: vec![(
                    CommandStream::Stderr,
                    format!("sh: {command}: not found\n").into_bytes(),
                )],
                code: Some(127),
                ..Script::default()
            });
        let stopped = || CommandExit {
            code: None,
            signal: (!script.ignores_term).then(|| "TERM".into()),
            cancelled: true,
        };
        loop {
            for (stream, bytes) in &script.output {
                if cancel.load(Ordering::SeqCst) {
                    return Ok(stopped());
                }
                if let Some(delay) = script.delay {
                    tokio::time::sleep(delay).await;
                } else {
                    tokio::task::yield_now().await;
                }
                self.check()?;
                output(*stream, bytes);
            }
            if !script.endless {
                break;
            }
            if script.output.is_empty() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            if cancel.load(Ordering::SeqCst) {
                return Ok(stopped());
            }
        }
        if let Some(message) = script.fail {
            return Err(AppError::Connection(message));
        }
        Ok(CommandExit {
            code: script.code,
            signal: None,
            cancelled: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fs() -> (Arc<MemoryServer>, Arc<dyn RemoteFs>) {
        let server = MemoryServer::new();
        let fs = server.connect().fs();
        (server, fs)
    }

    #[tokio::test]
    async fn basic_posix_semantics() {
        let (server, fs) = fs();
        fs.mkdir("/home/user/www").await.unwrap();
        assert_eq!(
            fs.mkdir("/home/user/www").await.unwrap_err().code(),
            crate::error::ErrorCode::Conflict
        );
        fs.create_file("/home/user/www/a.txt").await.unwrap();
        assert!(fs.create_file("/home/user/www/a.txt").await.is_err());
        assert_eq!(
            fs.stat("/home/user/nope").await.unwrap_err().code(),
            crate::error::ErrorCode::NotFound
        );
        fs.rename("/home/user/www", "/home/user/site")
            .await
            .unwrap();
        assert!(server.exists("/home/user/site/a.txt"));
        assert!(!server.exists("/home/user/www"));
        assert!(fs.remove_dir("/home/user/site").await.is_err(), "not empty");
        fs.remove_file("/home/user/site/a.txt").await.unwrap();
        fs.remove_dir("/home/user/site").await.unwrap();
    }

    #[tokio::test]
    async fn read_only_directories_and_files() {
        let (server, fs) = fs();
        server.add_dir("/ro");
        server.add_file("/ro/f", b"x");
        server.set_mode("/ro", 0o555);
        let code = |e: AppError| e.code();
        assert_eq!(
            code(fs.create_file("/ro/g").await.unwrap_err()),
            crate::error::ErrorCode::PermissionDenied
        );
        assert_eq!(
            code(
                fs.write_atomic("/ro/f", b"y", None, false)
                    .await
                    .unwrap_err()
            ),
            crate::error::ErrorCode::Unsupported
        );
        let outcome = fs.write_atomic("/ro/f", b"y", None, true).await.unwrap();
        assert!(!outcome.atomic);
        assert_eq!(server.file("/ro/f").unwrap(), b"y");
    }

    #[tokio::test]
    async fn symlinks_are_followed_for_reads_but_replaced_by_writes() {
        let (server, fs) = fs();
        server.add_dir("/etc");
        server.add_file("/etc/real.conf", b"real");
        server.add_link("/etc/link.conf", "real.conf");
        server.add_link("/etc/dirlink", "/home/user");
        assert_eq!(
            fs.read_range("/etc/link.conf", 0, 100).await.unwrap(),
            b"real"
        );
        let listing = fs.list("/etc").await.unwrap();
        let link = listing.iter().find(|e| e.name == "dirlink").unwrap();
        assert_eq!(link.kind, EntryKind::Symlink);
        assert_eq!(link.target_kind, Some(EntryKind::Dir));
        assert_eq!(fs.canonicalize("/etc/dirlink").await.unwrap(), "/home/user");
        fs.write_atomic("/etc/link.conf", b"new", None, false)
            .await
            .unwrap();
        assert_eq!(server.file("/etc/real.conf").unwrap(), b"real");
        assert_eq!(server.file("/etc/link.conf").unwrap(), b"new");
    }

    #[tokio::test]
    async fn write_atomic_leaves_nothing_behind_on_failure() {
        let (server, fs) = fs();
        server.add_file("/home/user/f", b"old");
        server.fail_next_rename();
        assert!(
            fs.write_atomic("/home/user/f", b"new", None, false)
                .await
                .is_err()
        );
        assert_eq!(server.file("/home/user/f").unwrap(), b"old");
        assert!(!server.paths().iter().any(|p| p.contains(".gem-")));
        fs.write_atomic("/home/user/f", b"new", Some(0o600), false)
            .await
            .unwrap();
        assert_eq!(server.mode("/home/user/f"), Some(0o600));
    }

    #[tokio::test]
    async fn replace_swaps_in_one_step_and_can_fail() {
        let (server, fs) = fs();
        server.add_file("/home/user/f", b"old");
        server.add_file("/home/user/.f.gem-1", b"new");
        server.fail_next_replace();
        assert!(
            fs.replace("/home/user/.f.gem-1", "/home/user/f")
                .await
                .is_err()
        );
        assert_eq!(server.file("/home/user/f").unwrap(), b"old");
        let outcome = fs
            .replace("/home/user/.f.gem-1", "/home/user/f")
            .await
            .unwrap();
        assert!(outcome.atomic);
        assert_eq!(server.file("/home/user/f").unwrap(), b"new");
        assert!(!server.exists("/home/user/.f.gem-1"));
        assert_eq!(
            fs.replace("/home/user/missing", "/home/user/f")
                .await
                .unwrap_err()
                .code(),
            crate::error::ErrorCode::NotFound
        );
        server.disable_replace();
        assert_eq!(
            fs.replace("/home/user/f", "/home/user/g")
                .await
                .unwrap_err()
                .code(),
            crate::error::ErrorCode::Unsupported
        );
    }

    #[tokio::test]
    async fn dropped_connections_fail_every_channel() {
        let server = MemoryServer::new();
        let connection = server.connect();
        let secondary = connection.open_secondary().await.unwrap();
        server.drop_connection();
        assert!(!connection.is_alive());
        assert_eq!(
            secondary.keepalive().await.unwrap_err().code(),
            crate::error::ErrorCode::Connection
        );
        connection.close().await;
        assert_eq!(
            connection.fs().home().await.unwrap_err().code(),
            crate::error::ErrorCode::Disconnected
        );
    }
}
