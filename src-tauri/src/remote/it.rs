//! Protocol integration tests against the Docker test servers in `dev/servers`.
//!
//! Skipped (with a note) unless `GEM_IT=1`; run them with `pnpm servers:up && pnpm rust:it`.
//! One shared suite ([`suite`]) runs for every backend: SFTP (password, key, encrypted key,
//! agent), FTP, explicit FTPS and implicit FTPS on vsftpd (LIST listings), and FTP and explicit
//! FTPS on ProFTPD (MLSD listings). Focused tests cover trust decisions, authentication
//! failures, killed connections and log redaction.
//!
//! Every suite works in its own scratch folder under `/home/gem/it`; the shared fixtures
//! (`samples/`, `logs/app.log`, …) are only read, except `readonly/editable.txt`, which is
//! rewritten under a process-wide lock and restored.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use super::sftp::common::{is_gem_sibling, random_suffix};
use super::sftp::{self, AgentSource};
use super::{Auth, ConnectParams, Connection, HostDecision, HostVerifier, Logger, RemoteFs};
use crate::domain::entry::{Entry, EntryKind};
use crate::domain::log::LogLine;
use crate::domain::site::{FtpsMode, Protocol};
use crate::error::{AppError, ErrorCode};

const HOST: &str = "127.0.0.1";
const USER: &str = "gem";
const PASSWORD: &str = "gem-test-password";
const PASSPHRASE: &str = "gem-test-passphrase";
const HOME: &str = "/home/gem";
const SFTP_PORT: u16 = 2222;
const VSFTPD_PORT: u16 = 2121;
const VSFTPD_IMPLICIT_PORT: u16 = 2990;
const PROFTPD_PORT: u16 = 2321;
const LOG_SIZE: u64 = 200 * 1024 * 1024;
const BIG: u64 = 100 * 1024 * 1024;

/// `readonly/editable.txt` is shared by every suite, including `services::it`, so both
/// integration modules can run in one `cargo test` process.
pub(crate) static EDITABLE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn enabled(test: &str) -> bool {
    let on = std::env::var("GEM_IT").is_ok_and(|value| value == "1");
    if !on {
        eprintln!("{test}: skipped (set GEM_IT=1 and start the servers with `pnpm servers:up`)");
    }
    on
}

fn servers_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../dev/servers")
}

fn key(name: &str) -> String {
    servers_dir()
        .join("keys")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

fn seed(relative: &str) -> Vec<u8> {
    std::fs::read(servers_dir().join("seed").join(relative)).unwrap()
}

fn params(protocol: Protocol, port: u16, mode: FtpsMode, auth: Auth) -> ConnectParams {
    ConnectParams {
        protocol,
        host: HOST.into(),
        port,
        user: USER.into(),
        auth,
        ftps_mode: mode,
        passive: true,
        timeout_secs: 20,
    }
}

fn password() -> Auth {
    Auth::Password(PASSWORD.into())
}

fn sftp_params(auth: Auth) -> ConnectParams {
    params(Protocol::Sftp, SFTP_PORT, FtpsMode::Explicit, auth)
}

fn ftp_params(port: u16, protocol: Protocol, mode: FtpsMode) -> ConnectParams {
    params(protocol, port, mode, password())
}

/// In-memory trust store. `auto` trusts (and remembers) unknown identities, like a user who
/// clicked "Trust and connect".
struct Verifier {
    auto: bool,
    known: Mutex<HashMap<(String, u16), String>>,
}

impl Verifier {
    fn new(auto: bool) -> Arc<Self> {
        Arc::new(Self {
            auto,
            known: Mutex::new(HashMap::new()),
        })
    }

    fn trust(&self, host: &str, port: u16, fingerprint: &str) {
        self.known
            .lock()
            .unwrap()
            .insert((host.to_owned(), port), fingerprint.to_owned());
    }
}

impl HostVerifier for Verifier {
    fn check(&self, host: &str, port: u16, _key_type: &str, fingerprint: &str) -> HostDecision {
        let mut known = self.known.lock().unwrap();
        match known.get(&(host.to_owned(), port)) {
            Some(stored) if stored == fingerprint => HostDecision::Trusted,
            Some(stored) => HostDecision::Changed {
                previous_fingerprint: stored.clone(),
            },
            None if self.auto => {
                known.insert((host.to_owned(), port), fingerprint.to_owned());
                HostDecision::Trusted
            }
            None => HostDecision::Unknown,
        }
    }
}

/// Collects every log line of a connection.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<LogLine>>>);

impl Capture {
    /// `GEM_IT_LOG=1` also prints every line (to debug a failing suite).
    fn logger(&self) -> Logger {
        let lines = Arc::clone(&self.0);
        let echo = std::env::var("GEM_IT_LOG").is_ok_and(|value| value == "1");
        Logger::new(move |line| {
            if echo {
                eprintln!("  [{:?}] {}", line.kind, line.text);
            }
            lines.lock().unwrap().push(line);
        })
    }

    fn texts(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|line| line.text.clone())
            .collect()
    }

    /// The last `count` lines, for failure messages.
    fn tail(&self, count: usize) -> String {
        let texts = self.texts();
        texts[texts.len().saturating_sub(count)..].join("\n")
    }

    fn assert_no_secrets(&self, name: &str) {
        let texts = self.texts();
        assert!(!texts.is_empty(), "{name}: nothing was logged");
        for text in &texts {
            assert!(!text.contains(PASSWORD), "{name}: password in log: {text}");
            assert!(
                !text.contains(PASSPHRASE),
                "{name}: passphrase in log: {text}"
            );
            assert!(!text.contains("PRIVATE KEY"), "{name}: key material in log");
        }
    }
}

#[derive(Clone)]
struct Target {
    name: &'static str,
    params: ConnectParams,
    agent: Option<AgentSource>,
    /// The server lists with MLSD (ProFTPD) rather than LIST.
    mlsd: bool,
}

impl Target {
    fn is_sftp(&self) -> bool {
        self.params.protocol == Protocol::Sftp
    }

    async fn connect(
        &self,
        verifier: Arc<dyn HostVerifier>,
        logger: Logger,
    ) -> Result<Arc<dyn Connection>, AppError> {
        match &self.agent {
            Some(source) => {
                sftp::connect_with(&self.params, verifier, logger, source.clone()).await
            }
            None => super::connect(&self.params, verifier, logger).await,
        }
    }
}

fn sftp_target(name: &'static str, auth: Auth) -> Target {
    Target {
        name,
        params: sftp_params(auth),
        agent: None,
        mlsd: false,
    }
}

fn ftp_target(
    name: &'static str,
    port: u16,
    protocol: Protocol,
    mode: FtpsMode,
    mlsd: bool,
) -> Target {
    Target {
        name,
        params: ftp_params(port, protocol, mode),
        agent: None,
        mlsd,
    }
}

fn code<T>(result: Result<T, AppError>) -> ErrorCode {
    match result {
        Ok(_) => panic!("expected an error, got success"),
        Err(error) => error.code(),
    }
}

/// The bytes of `logs/app.log` in `[offset, offset + len)` (see dev/servers/seeder/seed.sh).
fn log_bytes(offset: u64, len: u64) -> Vec<u8> {
    let end = (offset + len).min(LOG_SIZE);
    let mut out = Vec::with_capacity((end - offset) as usize);
    let mut line_no = offset / 128;
    while line_no * 128 < end {
        let mut line = format!(
            "{line_no:010} INFO  synthetic log line at offset {:012} path=/api/items/{} status=200",
            line_no * 128,
            line_no % 1000
        );
        while line.len() < 127 {
            line.push('.');
        }
        line.push('\n');
        let start = line_no * 128;
        let from = offset.saturating_sub(start) as usize;
        let to = ((end - start) as usize).min(128);
        out.extend_from_slice(&line.as_bytes()[from..to]);
        line_no += 1;
    }
    out
}

/// A deterministic pseudo-random byte stream (xorshift), hashed as it is read.
struct PatternReader {
    remaining: u64,
    state: u64,
    hasher: Sha256,
}

impl PatternReader {
    fn new(len: u64, seed: u64) -> Self {
        Self {
            remaining: len,
            state: seed | 1,
            hasher: Sha256::new(),
        }
    }
}

impl AsyncRead for PatternReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let n = (buf.remaining() as u64).min(self.remaining).min(64 * 1024) as usize;
        let mut chunk = vec![0u8; n];
        for byte in chunk.iter_mut() {
            let mut x = self.state;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.state = x;
            *byte = (x >> 24) as u8;
        }
        self.hasher.update(&chunk);
        self.remaining -= n as u64;
        buf.put_slice(&chunk);
        Poll::Ready(Ok(()))
    }
}

/// A sink that hashes what it receives and remembers the largest single write.
#[derive(Default)]
struct HashSink {
    hasher: Sha256,
    written: u64,
    largest_write: usize,
}

impl AsyncWrite for HashSink {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.hasher.update(buf);
        self.written += buf.len() as u64;
        self.largest_write = self.largest_write.max(buf.len());
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Progress callback state: monotonic check and an optional cancel threshold.
struct Tracker {
    last: AtomicU64,
    monotonic: AtomicBool,
    cancel_after: Option<u64>,
    cancel: AtomicBool,
}

impl Tracker {
    fn new(cancel_after: Option<u64>) -> Self {
        Self {
            last: AtomicU64::new(0),
            monotonic: AtomicBool::new(true),
            cancel_after,
            cancel: AtomicBool::new(false),
        }
    }

    fn record(&self, total: u64) {
        if total < self.last.swap(total, Ordering::AcqRel) {
            self.monotonic.store(false, Ordering::Release);
        }
        if self.cancel_after.is_some_and(|limit| total >= limit) {
            self.cancel.store(true, Ordering::Release);
        }
    }
}

fn find<'a>(entries: &'a [Entry], name: &str) -> &'a Entry {
    entries
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("{name} not listed"))
}

async fn leftovers(fs: &dyn RemoteFs, dir: &str) -> Vec<String> {
    fs.list(dir)
        .await
        .unwrap()
        .into_iter()
        .map(|entry| entry.name)
        .filter(|name| is_gem_sibling(name))
        .collect()
}

/// The shared suite: every `RemoteFs` method, on every backend.
async fn suite(target: Target) {
    let started = Instant::now();
    let capture = Capture::default();
    let verifier = Verifier::new(true);
    let conn = target
        .connect(verifier.clone(), capture.logger())
        .await
        .unwrap_or_else(|error| panic!("{}: connect failed: {error}", target.name));
    let fs = conn.fs();
    let name = target.name;
    assert!(conn.is_alive());
    assert!(conn.server_info().is_some(), "{name}: no server info");
    let caps = fs.capabilities();
    assert!(
        caps.chmod && caps.symlinks && caps.owner,
        "{name}: {caps:?}"
    );
    if target.is_sftp() {
        assert!(caps.atomic_rename, "OpenSSH offers posix-rename");
    }

    // home / canonicalize
    assert_eq!(fs.home().await.unwrap(), HOME);
    assert_eq!(
        fs.canonicalize("/home/gem/link-to-dir").await.unwrap(),
        "/home/gem/samples"
    );
    assert_eq!(
        fs.canonicalize("/home/gem/samples/../logs").await.unwrap(),
        "/home/gem/logs"
    );
    assert_eq!(
        code(fs.canonicalize("/home/gem/nope").await),
        ErrorCode::NotFound
    );

    // list: kinds, symlinks, no dot entries
    let log_before_root = capture.0.lock().unwrap().len();
    let root = fs.list(HOME).await.unwrap();
    assert!(
        root.iter()
            .all(|entry| entry.name != "." && entry.name != "..")
    );
    assert_eq!(find(&root, "samples").kind, EntryKind::Dir);
    assert_eq!(find(&root, "samples").path, "/home/gem/samples");
    assert_eq!(find(&root, "readonly").kind, EntryKind::Dir);
    let to_dir = find(&root, "link-to-dir");
    assert_eq!(to_dir.kind, EntryKind::Symlink, "{name}");
    assert_eq!(to_dir.target_kind, Some(EntryKind::Dir), "{name}");
    assert!(to_dir.is_dir_like());
    let to_file = find(&root, "link-to-file");
    assert_eq!(to_file.target_kind, Some(EntryKind::File), "{name}");
    assert_eq!(
        to_file.size,
        Some(seed("samples/index.php").len() as u64),
        "{name}: a link to a file shows the file's size"
    );
    // Probing what the links are (SIZE/CWD/MLST answered with 550) is not an error.
    let probe_errors: Vec<String> = capture.0.lock().unwrap()[log_before_root..]
        .iter()
        .filter(|line| line.kind == crate::domain::log::LogKind::Error)
        .map(|line| line.text.clone())
        .collect();
    assert!(
        probe_errors.is_empty(),
        "{name}: error lines while listing: {probe_errors:?}"
    );
    if !target.mlsd {
        assert_eq!(to_file.link_target.as_deref(), Some("samples/index.php"));
    }
    // ProFTPD leaves dangling links out of MLSD listings.
    if !target.mlsd {
        let broken = find(&root, "link-broken");
        assert_eq!(broken.kind, EntryKind::Symlink);
        assert_eq!(broken.target_kind, None);
    }

    // list: sizes, modes and owners of the committed samples (hidden files included)
    let samples = fs.list("/home/gem/samples").await.unwrap();
    for sample in [
        "index.php",
        "main.go",
        "docker-compose.yml",
        ".env",
        "composer.json",
        "Dockerfile",
        "deploy.sh",
        "nginx.conf",
        "config.yaml",
        "app.py",
        "lib.rs",
        "style.css",
        "page.html",
        "data.xml",
        "query.sql",
        "README.md",
        "config.toml",
        ".htaccess",
        "Makefile",
        "app.ts",
    ] {
        let entry = find(&samples, sample);
        assert_eq!(entry.kind, EntryKind::File);
        assert_eq!(
            entry.size,
            Some(seed(&format!("samples/{sample}")).len() as u64),
            "{name}: size of {sample}"
        );
        assert!(entry.mtime.is_some());
        assert_eq!(
            entry.owner.as_deref(),
            Some(USER),
            "{name}: owner of {sample}"
        );
    }
    assert_eq!(find(&samples, "deploy.sh").mode, Some(0o755), "{name}");
    assert_eq!(find(&samples, "index.php").mode, Some(0o644), "{name}");

    // list: 10 000 entries, Unicode and spaces
    let timer = Instant::now();
    let many = fs.list("/home/gem/many").await.unwrap();
    let many_time = timer.elapsed();
    assert_eq!(many.len(), 10_000, "{name}");
    assert!(many.iter().any(|entry| entry.name == "file-10000.txt"));
    let names = fs.list("/home/gem/names").await.unwrap();
    for expected in [
        "with space.txt",
        "ação ünïcödé.txt",
        "日本語.txt",
        "emoji 🎉.txt",
        "sub dir",
    ] {
        find(&names, expected);
    }
    assert_eq!(
        fs.list("/home/gem/names/sub dir").await.unwrap()[0].name,
        "inner.txt"
    );
    assert_eq!(code(fs.list("/home/gem/nope").await), ErrorCode::NotFound);

    // stat
    let index = fs.stat("/home/gem/samples/index.php").await.unwrap();
    assert_eq!(index.kind, EntryKind::File);
    assert_eq!(index.name, "index.php");
    assert_eq!(index.size, Some(seed("samples/index.php").len() as u64));
    assert_eq!(index.mode, Some(0o644), "{name}");
    assert!(index.mtime.is_some());
    assert_eq!(
        fs.stat("/home/gem/samples").await.unwrap().kind,
        EntryKind::Dir
    );
    let followed = fs.stat("/home/gem/link-to-file").await.unwrap();
    assert_eq!(followed.kind, EntryKind::File, "{name}: stat follows links");
    assert_eq!(followed.size, index.size);
    assert_eq!(
        fs.stat("/home/gem/names/with space.txt")
            .await
            .unwrap()
            .size,
        Some(6)
    );
    assert_eq!(code(fs.stat("/home/gem/nope").await), ErrorCode::NotFound);

    // read_range: byte-exact encodings and binary data
    for file in [
        "encodings/latin1-crlf.txt",
        "encodings/mixed-eol.txt",
        "encodings/utf16le-bom.txt",
        "samples/Makefile",
    ] {
        let expected = seed(file);
        let path = format!("{HOME}/{file}");
        let got = fs.read_range(&path, 0, 1 << 20).await.unwrap();
        assert_eq!(got, expected, "{name}: {file}");
    }
    let pattern: Vec<u8> = (0..65_536u32).map(|i| (i % 256) as u8).collect();
    let bin = "/home/gem/binary/bytes.bin";
    assert_eq!(fs.read_range(bin, 0, 65_536).await.unwrap(), pattern);
    assert_eq!(
        fs.read_range(bin, 1000, 300).await.unwrap(),
        pattern[1000..1300]
    );
    assert_eq!(
        fs.read_range(bin, 65_530, 100).await.unwrap(),
        pattern[65_530..],
        "{name}: short read at the end"
    );
    assert!(fs.read_range(bin, 65_536, 10).await.unwrap().is_empty());
    assert!(fs.read_range(bin, 70_000, 10).await.unwrap().is_empty());
    assert_eq!(
        code(fs.read_range("/home/gem/nope", 0, 10).await),
        ErrorCode::NotFound
    );

    // read_range on the 200 MiB log: the last 2 MiB, quickly, then other windows and a list
    // (on FTP the control connection must stay in step after each early-closed RETR).
    let log = "/home/gem/logs/app.log";
    assert_eq!(fs.stat(log).await.unwrap().size, Some(LOG_SIZE));
    let window = 2 * 1024 * 1024;
    let timer = Instant::now();
    let tail = fs.read_range(log, LOG_SIZE - window, window).await.unwrap();
    let tail_time = timer.elapsed();
    assert_eq!(tail.len() as u64, window);
    assert!(
        tail == log_bytes(LOG_SIZE - window, window),
        "{name}: tail bytes"
    );
    assert!(
        tail_time < Duration::from_secs(3),
        "{name}: last 2 MiB took {tail_time:?}"
    );
    for (offset, len) in [
        (0, window),
        (123_457, 1000),
        (100 * 1024 * 1024 + 5, 300_001),
    ] {
        let got = fs.read_range(log, offset, len).await.unwrap();
        assert!(got == log_bytes(offset, len), "{name}: window at {offset}");
    }
    assert!(fs.read_range(log, LOG_SIZE - 10, 100).await.unwrap() == log_bytes(LOG_SIZE - 10, 10));
    assert!(!fs.list("/home/gem/logs").await.unwrap().is_empty());
    fs.keepalive().await.unwrap();

    // scratch folder: mkdir, create_file, conflicts
    let scratch = format!("{HOME}/it/{name}-{}", random_suffix().unwrap());
    fs.mkdir(&scratch).await.unwrap();
    assert_eq!(code(fs.mkdir(&scratch).await), ErrorCode::Conflict);
    let a = format!("{scratch}/a.txt");
    fs.create_file(&a).await.unwrap();
    assert_eq!(fs.stat(&a).await.unwrap().size, Some(0));
    assert_eq!(code(fs.create_file(&a).await), ErrorCode::Conflict);

    // write_atomic: new file, then replace, mode applied, no temp left behind
    let b = format!("{scratch}/b.txt");
    let outcome = fs
        .write_atomic(&b, b"first version\n", Some(0o640), false)
        .await
        .unwrap();
    assert!(outcome.atomic);
    assert_eq!(fs.read_range(&b, 0, 100).await.unwrap(), b"first version\n");
    assert_eq!(fs.stat(&b).await.unwrap().mode, Some(0o640), "{name}");
    let second = "second version\r\nwith CRLF\r\n".repeat(1000);
    let outcome = fs
        .write_atomic(&b, second.as_bytes(), Some(0o755), false)
        .await
        .unwrap();
    assert!(outcome.atomic);
    assert_eq!(
        fs.read_range(&b, 0, 1 << 20).await.unwrap(),
        second.as_bytes()
    );
    assert_eq!(fs.stat(&b).await.unwrap().mode, Some(0o755), "{name}");
    assert!(leftovers(fs.as_ref(), &scratch).await.is_empty());

    // stale leftovers of interrupted saves of b.txt (older than an hour) are removed by its
    // next save; fresh ones, other files' and look-alikes are left alone. The servers share
    // one tree, so the ages are set over SFTP for every backend.
    let two_hours_ago = u32::try_from(chrono::Utc::now().timestamp() - 7200).unwrap();
    let stale = [
        format!("{scratch}/.b.txt.gem-Stale123"),
        format!("{scratch}/.b.txt.gem-old-Stale456"),
    ];
    let kept = [
        format!("{scratch}/.b.txt.gem-Fresh789"),
        format!("{scratch}/.c.txt.gem-Stale123"),
        format!("{scratch}/.b.txt.gem-Stale1234"),
        format!("{scratch}/b.txt.gem-Stale123"),
    ];
    for path in stale.iter().chain(&kept) {
        fs.create_file(path).await.unwrap();
    }
    let aged: Vec<&str> = stale.iter().chain(&kept[1..]).map(String::as_str).collect();
    sftp::set_mtime(
        &sftp_params(password()),
        verifier.clone(),
        &aged,
        two_hours_ago,
    )
    .await
    .unwrap();
    fs.write_atomic(&b, second.as_bytes(), Some(0o755), false)
        .await
        .unwrap();
    let after: Vec<String> = fs
        .list(&scratch)
        .await
        .unwrap()
        .into_iter()
        .map(|entry| entry.path)
        .collect();
    for path in &stale {
        assert!(!after.contains(path), "{name}: {path} was not swept");
    }
    for path in &kept {
        assert!(after.contains(path), "{name}: {path} was removed");
        fs.remove_file(path).await.unwrap();
    }
    assert!(
        capture
            .texts()
            .iter()
            .any(|text| text.starts_with("Removed the stale temp file"))
    );
    assert!(leftovers(fs.as_ref(), &scratch).await.is_empty());

    // replace: finish an upload made to a temp sibling, onto a missing then an existing target
    let r = format!("{scratch}/r.txt");
    let first_temp = format!("{scratch}/.r.txt.gem-Upload01");
    let mut first: &[u8] = b"first upload\n";
    fs.upload(&mut first, &first_temp, &|_| {}, &AtomicBool::new(false))
        .await
        .unwrap();
    fs.chmod(&first_temp, 0o750).await.unwrap();
    let outcome = fs.replace(&first_temp, &r).await.unwrap();
    assert!(outcome.atomic, "{name}");
    assert_eq!(fs.read_range(&r, 0, 100).await.unwrap(), b"first upload\n");
    assert_eq!(fs.stat(&r).await.unwrap().mode, Some(0o750), "{name}");
    assert_eq!(code(fs.stat(&first_temp).await), ErrorCode::NotFound);
    let second_temp = format!("{scratch}/.r.txt.gem-Upload02");
    let mut replacement: &[u8] = b"second upload, longer than the first\n";
    fs.upload(
        &mut replacement,
        &second_temp,
        &|_| {},
        &AtomicBool::new(false),
    )
    .await
    .unwrap();
    let outcome = fs.replace(&second_temp, &r).await.unwrap();
    assert!(outcome.atomic, "{name}");
    assert_eq!(
        fs.read_range(&r, 0, 100).await.unwrap(),
        b"second upload, longer than the first\n"
    );
    assert!(leftovers(fs.as_ref(), &scratch).await.is_empty(), "{name}");
    assert_eq!(code(fs.replace(&first_temp, &r).await), ErrorCode::NotFound);
    fs.remove_file(&r).await.unwrap();

    // rename: never overwrites
    assert_eq!(code(fs.rename(&a, &b).await), ErrorCode::Conflict);
    let c = format!("{scratch}/c d.txt");
    fs.rename(&a, &c).await.unwrap();
    assert_eq!(code(fs.stat(&a).await), ErrorCode::NotFound);
    assert_eq!(fs.stat(&c).await.unwrap().size, Some(0));
    assert_eq!(
        code(
            fs.rename("/home/gem/it/nope", &format!("{scratch}/x"))
                .await
        ),
        ErrorCode::NotFound
    );

    // chmod
    fs.chmod(&c, 0o600).await.unwrap();
    assert_eq!(fs.stat(&c).await.unwrap().mode, Some(0o600));

    // write_atomic over a folder fails (rename refused) and leaves no temp file
    let folder = format!("{scratch}/sub");
    fs.mkdir(&folder).await.unwrap();
    assert!(fs.write_atomic(&folder, b"x", None, false).await.is_err());
    assert_eq!(fs.stat(&folder).await.unwrap().kind, EntryKind::Dir);
    assert!(leftovers(fs.as_ref(), &scratch).await.is_empty(), "{name}");

    // an abandoned write_atomic (future dropped half-way, once its temp file exists) keeps the
    // original, its temp file is removed in the background and the connection stays usable
    let big_payload = vec![b'z'; 256 * 1024 * 1024];
    let observer = conn.open_secondary().await.unwrap();
    {
        let write = fs.write_atomic(&b, &big_payload, None, false);
        tokio::pin!(write);
        let mut started = false;
        for _ in 0..500 {
            tokio::select! {
                result = &mut write => panic!("{name}: the write finished first: {:?}", result.err()),
                () = tokio::time::sleep(Duration::from_millis(5)) => {}
            }
            if !leftovers(observer.as_ref(), &scratch).await.is_empty() {
                started = true;
                break;
            }
        }
        assert!(started, "{name}: the temp file never appeared");
    }
    let mut clean = false;
    for _ in 0..50 {
        if leftovers(observer.as_ref(), &scratch).await.is_empty() {
            clean = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        clean,
        "{name}: temp file of the abandoned write is still there; log tail:\n{}",
        capture.tail(60)
    );
    observer.close().await;
    assert_eq!(
        fs.read_range(&b, 0, 1 << 20).await.unwrap(),
        second.as_bytes(),
        "{name}: original changed"
    );
    drop(big_payload);

    // read-only folder
    {
        let _lock = EDITABLE.lock().await;
        assert_eq!(
            code(fs.create_file("/home/gem/readonly/new.txt").await),
            ErrorCode::PermissionDenied,
            "{name}"
        );
        assert_eq!(
            code(fs.mkdir("/home/gem/readonly/new").await),
            ErrorCode::PermissionDenied,
            "{name}"
        );
        let locked = "/home/gem/readonly/locked.txt";
        assert_eq!(
            code(fs.write_atomic(locked, b"x", None, false).await),
            ErrorCode::Unsupported,
            "{name}: no in-place fallback without consent"
        );
        assert_eq!(
            code(fs.write_atomic(locked, b"x", None, true).await),
            ErrorCode::PermissionDenied,
            "{name}: in place still refused"
        );
        assert_eq!(
            code(fs.remove_file(locked).await),
            ErrorCode::PermissionDenied,
            "{name}"
        );
        let editable = "/home/gem/readonly/editable.txt";
        let original = fs.read_range(editable, 0, 4096).await.unwrap();
        let outcome = fs
            .write_atomic(editable, b"changed in place\n", None, true)
            .await
            .unwrap();
        assert!(!outcome.atomic, "{name}");
        assert_eq!(
            fs.read_range(editable, 0, 4096).await.unwrap(),
            b"changed in place\n"
        );
        fs.write_atomic(editable, &original, None, true)
            .await
            .unwrap();
        assert!(
            leftovers(fs.as_ref(), "/home/gem/readonly")
                .await
                .is_empty()
        );
    }

    // upload / download of 100 MiB with progress and constant memory
    let big = format!("{scratch}/big.bin");
    let tracker = Tracker::new(None);
    let mut source = PatternReader::new(BIG, 7);
    let timer = Instant::now();
    let sent = fs
        .upload(
            &mut source,
            &big,
            &|total| tracker.record(total),
            &AtomicBool::new(false),
        )
        .await
        .unwrap();
    let upload_time = timer.elapsed();
    assert_eq!(sent, BIG);
    assert_eq!(tracker.last.load(Ordering::Acquire), BIG);
    assert!(tracker.monotonic.load(Ordering::Acquire));
    let uploaded_hash = source.hasher.finalize();
    assert_eq!(fs.stat(&big).await.unwrap().size, Some(BIG));
    let tracker = Tracker::new(None);
    let mut sink = HashSink::default();
    let timer = Instant::now();
    let received = fs
        .download(
            &big,
            0,
            &mut sink,
            &|total| tracker.record(total),
            &AtomicBool::new(false),
        )
        .await
        .unwrap();
    let download_time = timer.elapsed();
    assert_eq!(received, BIG);
    assert_eq!(sink.written, BIG);
    assert!(tracker.monotonic.load(Ordering::Acquire));
    assert_eq!(sink.hasher.finalize(), uploaded_hash, "{name}: checksum");
    assert!(
        sink.largest_write <= 1024 * 1024,
        "{name}: a {} byte write means the download is buffered",
        sink.largest_write
    );
    // download from an offset
    let mut sink = HashSink::default();
    let tail = fs
        .download(
            &big,
            BIG - 1000,
            &mut sink,
            &|_| {},
            &AtomicBool::new(false),
        )
        .await
        .unwrap();
    assert_eq!(tail, 1000);

    // cancel mid-way, on transfer connections like the transfer service; the primary is not
    // affected (ProFTPD over TLS ends a session whose download is cut short, so the transfer
    // connection may be gone afterwards)
    let canceller = conn.open_secondary().await.unwrap();
    let tracker = Tracker::new(Some(8 * 1024 * 1024));
    let mut sink = HashSink::default();
    let cancelled = canceller
        .download(
            &big,
            0,
            &mut sink,
            &|total| tracker.record(total),
            &tracker.cancel,
        )
        .await;
    assert_eq!(code(cancelled), ErrorCode::Cancelled);
    assert!(sink.written < BIG);
    let after_cancel = canceller.keepalive().await;
    eprintln!("{name}: transfer connection after a cancelled download: {after_cancel:?}");
    canceller.close().await;
    let canceller = conn.open_secondary().await.unwrap();
    let partial = format!("{scratch}/partial.bin");
    let tracker = Tracker::new(Some(8 * 1024 * 1024));
    let cancelled = canceller
        .upload(
            &mut PatternReader::new(BIG, 9),
            &partial,
            &|total| tracker.record(total),
            &tracker.cancel,
        )
        .await;
    assert_eq!(code(cancelled), ErrorCode::Cancelled);
    canceller.keepalive().await.unwrap();
    canceller.close().await;
    assert!(!fs.list(&scratch).await.unwrap().is_empty());

    // a secondary connection transfers while the primary browses
    let secondary = conn.open_secondary().await.unwrap();
    let transfer = {
        let secondary = Arc::clone(&secondary);
        let big = big.clone();
        tokio::spawn(async move {
            let mut sink = HashSink::default();
            let size = secondary
                .download(&big, 0, &mut sink, &|_| {}, &AtomicBool::new(false))
                .await
                .unwrap();
            (size, sink.hasher.finalize())
        })
    };
    let mut listings = 0;
    while !transfer.is_finished() || listings == 0 {
        assert_eq!(
            fs.list("/home/gem/samples").await.unwrap().len(),
            samples.len()
        );
        listings += 1;
    }
    let (size, hash) = transfer.await.unwrap();
    assert_eq!(size, BIG);
    assert_eq!(hash, uploaded_hash);
    secondary.keepalive().await.unwrap();
    secondary.close().await;
    assert_eq!(code(secondary.list(HOME).await), ErrorCode::Disconnected);
    fs.keepalive().await.unwrap();

    // remove_file / remove_dir
    assert_eq!(
        code(fs.remove_file(&format!("{scratch}/nope")).await),
        ErrorCode::NotFound
    );
    assert!(fs.remove_dir(&scratch).await.is_err(), "{name}: not empty");
    for entry in fs.list(&scratch).await.unwrap() {
        match entry.kind {
            EntryKind::Dir => fs.remove_dir(&entry.path).await.unwrap(),
            _ => fs.remove_file(&entry.path).await.unwrap(),
        }
    }
    fs.remove_dir(&scratch).await.unwrap();
    assert_eq!(code(fs.stat(&scratch).await), ErrorCode::NotFound);

    // close: everything fails with Disconnected afterwards
    conn.close().await;
    assert!(!conn.is_alive());
    assert_eq!(code(fs.list(HOME).await), ErrorCode::Disconnected);
    assert_eq!(code(fs.keepalive().await), ErrorCode::Disconnected);
    assert_eq!(code(conn.open_secondary().await), ErrorCode::Disconnected);

    capture.assert_no_secrets(name);
    if !target.is_sftp() {
        let texts = capture.texts();
        assert!(texts.iter().any(|text| text == "PASS ****"));
        assert!(texts.iter().any(|text| text.starts_with("REST ")));
        assert!(texts.iter().any(|text| text.starts_with(if target.mlsd {
            "MLSD "
        } else {
            "LIST -a"
        })));
    }
    eprintln!(
        "{name}: suite ok in {:?} (10k list {many_time:?}, last 2 MiB of 200 MiB {tail_time:?}, \
         100 MiB up {upload_time:?} / down {download_time:?}, {} log lines)",
        started.elapsed(),
        capture.texts().len()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sftp_password_suite() {
    if enabled("sftp_password_suite") {
        suite(sftp_target("sftp-password", password())).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sftp_key_suite() {
    if enabled("sftp_key_suite") {
        let auth = Auth::Key {
            path: key("id_ed25519"),
            passphrase: None,
        };
        suite(sftp_target("sftp-key", auth)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sftp_encrypted_key_suite() {
    if enabled("sftp_encrypted_key_suite") {
        let auth = Auth::Key {
            path: key("id_ed25519_encrypted"),
            passphrase: Some(PASSPHRASE.into()),
        };
        suite(sftp_target("sftp-encrypted-key", auth)).await;
    }
}

/// Starts a private `ssh-agent` holding the test key; `None` when OpenSSH's agent tools are
/// not installed.
struct TestAgent {
    child: std::process::Child,
    socket: PathBuf,
    _dir: tempfile::TempDir,
}

impl TestAgent {
    fn start() -> Option<Self> {
        let dir = tempfile::tempdir().ok()?;
        let socket = dir.path().join("agent.sock");
        let child = std::process::Command::new("ssh-agent")
            .arg("-D")
            .arg("-a")
            .arg(&socket)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let agent = Self {
            child,
            socket,
            _dir: dir,
        };
        for _ in 0..50 {
            if agent.socket.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        // ssh-add refuses keys readable by others; git does not keep 0600.
        let copy = agent._dir.path().join("id_rsa");
        std::fs::copy(key("id_rsa"), &copy).ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o600)).ok()?;
        }
        let added = std::process::Command::new("ssh-add")
            .arg("-q")
            .arg(&copy)
            .env("SSH_AUTH_SOCK", &agent.socket)
            .stderr(std::process::Stdio::null())
            .status()
            .ok()?;
        added.success().then_some(agent)
    }
}

impl Drop for TestAgent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sftp_agent_suite() {
    if !enabled("sftp_agent_suite") {
        return;
    }
    let Some(agent) = TestAgent::start() else {
        eprintln!("sftp_agent_suite: skipped (ssh-agent/ssh-add not available)");
        return;
    };
    // The agent holds the RSA key, which also exercises the rsa-sha2 hash selection.
    let target = Target {
        agent: Some(AgentSource::Socket(agent.socket.clone())),
        ..sftp_target("sftp-agent", Auth::Agent)
    };
    suite(target).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ftp_vsftpd_suite() {
    if enabled("ftp_vsftpd_suite") {
        let target = ftp_target(
            "ftp-vsftpd",
            VSFTPD_PORT,
            Protocol::Ftp,
            FtpsMode::Explicit,
            false,
        );
        suite(target).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ftps_explicit_vsftpd_suite() {
    if enabled("ftps_explicit_vsftpd_suite") {
        let target = ftp_target(
            "ftps-explicit-vsftpd",
            VSFTPD_PORT,
            Protocol::Ftps,
            FtpsMode::Explicit,
            false,
        );
        suite(target).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ftps_implicit_vsftpd_suite() {
    if enabled("ftps_implicit_vsftpd_suite") {
        let target = ftp_target(
            "ftps-implicit-vsftpd",
            VSFTPD_IMPLICIT_PORT,
            Protocol::Ftps,
            FtpsMode::Implicit,
            false,
        );
        suite(target).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ftp_proftpd_mlsd_suite() {
    if enabled("ftp_proftpd_mlsd_suite") {
        let target = ftp_target(
            "ftp-proftpd",
            PROFTPD_PORT,
            Protocol::Ftp,
            FtpsMode::Explicit,
            true,
        );
        suite(target).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ftps_explicit_proftpd_mlsd_suite() {
    if enabled("ftps_explicit_proftpd_mlsd_suite") {
        let target = ftp_target(
            "ftps-explicit-proftpd",
            PROFTPD_PORT,
            Protocol::Ftps,
            FtpsMode::Explicit,
            true,
        );
        suite(target).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sftp_host_key_unknown_trusted_then_changed() {
    if !enabled("sftp_host_key_unknown_trusted_then_changed") {
        return;
    }
    let capture = Capture::default();
    let verifier = Verifier::new(false);
    let params = sftp_params(password());
    let info = match sftp::connect(&params, verifier.clone(), capture.logger()).await {
        Err(AppError::HostKeyUnknown(info)) => info,
        other => panic!("expected HostKeyUnknown, got {:?}", other.err()),
    };
    assert_eq!((info.host.as_str(), info.port), (HOST, SFTP_PORT));
    assert_eq!(info.key_type, "ssh-ed25519");
    assert!(info.fingerprint.starts_with("SHA256:") && !info.fingerprint.ends_with('='));
    assert_eq!(info.previous_fingerprint, None);
    // Nothing was authenticated: the log stops at the host key.
    assert!(
        !capture
            .texts()
            .iter()
            .any(|text| text.starts_with("Authenticat"))
    );

    verifier.trust(HOST, SFTP_PORT, &info.fingerprint);
    let conn = sftp::connect(&params, verifier.clone(), capture.logger())
        .await
        .unwrap();
    assert!(conn.server_info().unwrap().starts_with("SSH-2.0-OpenSSH"));
    conn.close().await;

    let stored = "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    verifier.trust(HOST, SFTP_PORT, stored);
    match sftp::connect(&params, verifier, capture.logger()).await {
        Err(AppError::HostKeyChanged(changed)) => {
            assert_eq!(changed.fingerprint, info.fingerprint);
            assert_eq!(changed.previous_fingerprint.as_deref(), Some(stored));
        }
        other => panic!("expected HostKeyChanged, got {:?}", other.err()),
    }
    capture.assert_no_secrets("host keys");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ftps_untrusted_certificate_then_trusted_then_changed() {
    if !enabled("ftps_untrusted_certificate_then_trusted_then_changed") {
        return;
    }
    for (port, mode) in [
        (VSFTPD_PORT, FtpsMode::Explicit),
        (VSFTPD_IMPLICIT_PORT, FtpsMode::Implicit),
        (PROFTPD_PORT, FtpsMode::Explicit),
    ] {
        let capture = Capture::default();
        let verifier = Verifier::new(false);
        let params = ftp_params(port, Protocol::Ftps, mode);
        let info = match super::connect(&params, verifier.clone(), capture.logger()).await {
            Err(AppError::CertificateUntrusted(info)) => info,
            other => panic!(
                "{port}: expected CertificateUntrusted, got {:?}",
                other.err()
            ),
        };
        assert_eq!(info.key_type, "x509");
        assert!(info.fingerprint.starts_with("SHA256:"));
        assert_eq!(info.previous_fingerprint, None);
        // The password was never sent over the untrusted connection.
        assert!(!capture.texts().iter().any(|text| text.starts_with("PASS")));

        verifier.trust(HOST, port, &info.fingerprint);
        let conn = super::connect(&params, verifier.clone(), capture.logger())
            .await
            .unwrap();
        assert!(
            capture
                .texts()
                .iter()
                .any(|text| text.starts_with("TLS established"))
        );
        conn.fs().list(HOME).await.unwrap();
        conn.close().await;

        let stored = "SHA256:BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
        verifier.trust(HOST, port, stored);
        match super::connect(&params, verifier, capture.logger()).await {
            Err(AppError::CertificateUntrusted(changed)) => {
                assert_eq!(changed.fingerprint, info.fingerprint);
                assert_eq!(changed.previous_fingerprint.as_deref(), Some(stored));
            }
            other => panic!(
                "{port}: expected CertificateUntrusted, got {:?}",
                other.err()
            ),
        }
        capture.assert_no_secrets("certificates");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn authentication_failures() {
    if !enabled("authentication_failures") {
        return;
    }
    let capture = Capture::default();
    let verifier: Arc<dyn HostVerifier> = Verifier::new(true);
    let wrong = Auth::Password("wrong-password-123".into());
    let mut targets = vec![sftp_params(wrong.clone())];
    for (port, protocol, mode) in [
        (VSFTPD_PORT, Protocol::Ftp, FtpsMode::Explicit),
        (VSFTPD_IMPLICIT_PORT, Protocol::Ftps, FtpsMode::Implicit),
        (PROFTPD_PORT, Protocol::Ftps, FtpsMode::Explicit),
    ] {
        targets.push(params(protocol, port, mode, wrong.clone()));
    }
    for params in &targets {
        let result = super::connect(params, Arc::clone(&verifier), capture.logger()).await;
        assert_eq!(code(result), ErrorCode::AuthFailed, "{}", params.port);
    }
    // FTP: the server's 530 reply is in the log as an error line.
    let lines = capture.0.lock().unwrap().clone();
    assert!(lines.iter().any(|line| {
        line.kind == crate::domain::log::LogKind::Error && line.text.starts_with("530")
    }));
    assert!(
        !capture
            .texts()
            .iter()
            .any(|text| text.contains("wrong-password-123"))
    );

    // Encrypted key: no passphrase, wrong passphrase, unreadable file.
    let no_passphrase = Auth::Key {
        path: key("id_ed25519_encrypted"),
        passphrase: None,
    };
    let result = super::connect(
        &sftp_params(no_passphrase),
        Arc::clone(&verifier),
        capture.logger(),
    )
    .await;
    assert_eq!(code(result), ErrorCode::SecretRequired);
    let wrong_passphrase = Auth::Key {
        path: key("id_ed25519_encrypted"),
        passphrase: Some("not-the-passphrase".into()),
    };
    match super::connect(
        &sftp_params(wrong_passphrase),
        Arc::clone(&verifier),
        capture.logger(),
    )
    .await
    {
        Err(AppError::AuthFailed(message)) => assert!(message.contains("wrong passphrase")),
        other => panic!("expected AuthFailed, got {:?}", other.err()),
    }
    let missing = Auth::Key {
        path: key("does-not-exist"),
        passphrase: None,
    };
    let result = super::connect(
        &sftp_params(missing),
        Arc::clone(&verifier),
        capture.logger(),
    )
    .await;
    assert_eq!(code(result), ErrorCode::Validation);
    // The RSA key (rsa-sha2-512 chosen from server-sig-algs) authenticates.
    let rsa = Auth::Key {
        path: key("id_rsa"),
        passphrase: None,
    };
    let conn = super::connect(&sftp_params(rsa), Arc::clone(&verifier), capture.logger())
        .await
        .unwrap();
    conn.close().await;
    // FTP never accepts keys.
    let key_on_ftp = params(Protocol::Ftp, VSFTPD_PORT, FtpsMode::Explicit, Auth::Agent);
    assert_eq!(
        code(super::connect(&key_on_ftp, verifier, capture.logger()).await),
        ErrorCode::Unsupported
    );
    capture.assert_no_secrets("authentication failures");
    assert!(
        !capture
            .texts()
            .iter()
            .any(|text| text.contains("not-the-passphrase"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_errors_are_readable() {
    if !enabled("connection_errors_are_readable") {
        return;
    }
    let verifier: Arc<dyn HostVerifier> = Verifier::new(true);
    // Nothing listens on port 1.
    let refused = ConnectParams {
        port: 1,
        ..sftp_params(password())
    };
    match super::connect(&refused, Arc::clone(&verifier), Logger::noop()).await {
        Err(AppError::Connection(message)) => assert!(message.contains("refused"), "{message}"),
        other => panic!("expected Connection, got {:?}", other.err()),
    }
    let unresolvable = ConnectParams {
        host: "does-not-exist.invalid".into(),
        ..ftp_params(VSFTPD_PORT, Protocol::Ftp, FtpsMode::Explicit)
    };
    match super::connect(&unresolvable, Arc::clone(&verifier), Logger::noop()).await {
        Err(AppError::Connection(message)) => assert!(message.contains("resolve"), "{message}"),
        other => panic!("expected Connection, got {:?}", other.err()),
    }
    // Implicit TLS against the plain port: the handshake fails, it does not hang.
    let wrong_mode = ftp_params(VSFTPD_PORT, Protocol::Ftps, FtpsMode::Implicit);
    let result = super::connect(&wrong_mode, verifier, Logger::noop()).await;
    assert_eq!(code(result), ErrorCode::Connection);
}

/// A connection closed in the middle of `write_atomic` (the user closes the tab, the network
/// drops) must leave the original file untouched. The temp file cannot be removed over a dead
/// connection; it is reported here and removed through a new connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn killed_connection_during_write_keeps_the_original() {
    if !enabled("killed_connection_during_write_keeps_the_original") {
        return;
    }
    let targets = [
        sftp_target("kill-sftp", password()),
        ftp_target(
            "kill-ftp",
            VSFTPD_PORT,
            Protocol::Ftp,
            FtpsMode::Explicit,
            false,
        ),
        ftp_target(
            "kill-ftps",
            PROFTPD_PORT,
            Protocol::Ftps,
            FtpsMode::Explicit,
            true,
        ),
    ];
    for target in targets {
        let verifier = Verifier::new(true);
        let capture = Capture::default();
        let conn = target
            .connect(verifier.clone(), capture.logger())
            .await
            .unwrap();
        let fs = conn.fs();
        let scratch = format!("{HOME}/it/{}-{}", target.name, random_suffix().unwrap());
        fs.mkdir(&scratch).await.unwrap();
        let file = format!("{scratch}/config.yaml");
        fs.write_atomic(&file, b"original: true\n", Some(0o644), false)
            .await
            .unwrap();
        let payload = vec![b'x'; 512 * 1024 * 1024];
        let writer = {
            let fs = Arc::clone(&fs);
            let file = file.clone();
            tokio::spawn(async move { fs.write_atomic(&file, &payload, None, false).await })
        };
        // Watch from a second connection until the temp file exists: the write is under way.
        let observer = target
            .connect(verifier.clone(), capture.logger())
            .await
            .unwrap();
        let mut seen = Vec::new();
        for _ in 0..500 {
            seen = leftovers(observer.fs().as_ref(), &scratch).await;
            if !seen.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            !seen.is_empty(),
            "{}: the temp file never appeared",
            target.name
        );
        assert!(
            !writer.is_finished(),
            "{}: write finished too soon",
            target.name
        );
        observer.close().await;
        conn.close().await;
        let result = tokio::time::timeout(Duration::from_secs(30), writer)
            .await
            .expect("the write did not stop after close")
            .unwrap();
        assert!(result.is_err(), "{}: write survived close", target.name);

        let again = target.connect(verifier, capture.logger()).await.unwrap();
        let fs = again.fs();
        assert_eq!(
            fs.read_range(&file, 0, 100).await.unwrap(),
            b"original: true\n",
            "{}: original changed",
            target.name
        );
        let stale = leftovers(fs.as_ref(), &scratch).await;
        eprintln!(
            "{}: write interrupted by close ({}); stale temp files: {stale:?}",
            target.name,
            result
                .err()
                .map(|error| error.to_string())
                .unwrap_or_default()
        );
        for entry in fs.list(&scratch).await.unwrap() {
            fs.remove_file(&entry.path).await.unwrap();
        }
        fs.remove_dir(&scratch).await.unwrap();
        again.close().await;
        capture.assert_no_secrets(target.name);
    }
}

#[test]
fn log_generator_matches_the_seed_format() {
    let first = log_bytes(0, 128);
    assert_eq!(first.len(), 128);
    assert!(first.starts_with(b"0000000000 INFO  synthetic log line at offset 000000000000 "));
    assert_eq!(first[127], b'\n');
    assert_eq!(log_bytes(130, 4), b"0000");
    assert_eq!(log_bytes(LOG_SIZE - 1, 10), b"\n");
}

/// What one remote command printed, and how it ended.
#[derive(Default)]
struct Ran {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_bytes: u64,
    largest_chunk: usize,
}

/// Runs `command`, keeping its output (up to 1 MiB per stream; beyond that only counted) and
/// setting `cancel` after `cancel_after`.
async fn run_command(
    shell: &Arc<dyn super::RemoteShell>,
    command: &str,
    cancel_after: Option<Duration>,
) -> (
    Ran,
    Result<crate::domain::command::CommandExit, AppError>,
    Duration,
) {
    use crate::domain::command::CommandStream;
    let ran = Mutex::new(Ran::default());
    let cancel = Arc::new(AtomicBool::new(false));
    if let Some(after) = cancel_after {
        let cancel = Arc::clone(&cancel);
        tokio::spawn(async move {
            tokio::time::sleep(after).await;
            cancel.store(true, Ordering::Release);
        });
    }
    let sink = |stream: CommandStream, bytes: &[u8]| {
        let mut ran = ran.lock().unwrap();
        ran.largest_chunk = ran.largest_chunk.max(bytes.len());
        let kept = match stream {
            CommandStream::Stdout => {
                ran.stdout_bytes += bytes.len() as u64;
                &mut ran.stdout
            }
            CommandStream::Stderr => &mut ran.stderr,
        };
        if kept.len() < 1 << 20 {
            kept.extend_from_slice(bytes);
        }
    };
    let started = Instant::now();
    let exit = shell.exec(command, &sink, &cancel).await;
    (ran.into_inner().unwrap(), exit, started.elapsed())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sftp_remote_commands() {
    if !enabled("sftp_remote_commands") {
        return;
    }
    let capture = Capture::default();
    let conn = sftp::connect(
        &sftp_params(password()),
        Verifier::new(true),
        capture.logger(),
    )
    .await
    .unwrap();
    let shell = conn.shell().expect("SFTP connections offer a shell");

    // stdout and stderr apart, the exit code
    let (ran, exit, _) = run_command(&shell, "echo hi; echo err >&2; exit 3", None).await;
    let exit = exit.unwrap();
    assert_eq!(ran.stdout, b"hi\n");
    assert_eq!(ran.stderr, b"err\n");
    assert_eq!(
        (exit.code, exit.signal.as_deref(), exit.cancelled),
        (Some(3), None, false)
    );

    // the user's shell, as `ssh host command`: variables expand, unknown commands give 127
    let (ran, exit, _) = run_command(&shell, "echo $HOME; printf 'mark%s\\n' er-91x", None).await;
    assert_eq!(ran.stdout, b"/home/gem\nmarker-91x\n");
    assert_eq!(exit.unwrap().code, Some(0));
    let (ran, exit, _) = run_command(&shell, "no-such-command-gem", None).await;
    assert_eq!(exit.unwrap().code, Some(127));
    assert!(!ran.stderr.is_empty());
    // standard input is closed: `cat` ends at once
    let (_, exit, elapsed) = run_command(&shell, "cat", None).await;
    assert_eq!(exit.unwrap().code, Some(0));
    assert!(elapsed < Duration::from_secs(5));

    // killed by a signal
    let (_, exit, _) = run_command(&shell, "kill -TERM $$", None).await;
    let exit = exit.unwrap();
    assert_eq!(exit.signal.as_deref(), Some("TERM"), "{exit:?}");
    assert!(!exit.cancelled);

    // 20 MiB of output arrives whole, in small chunks
    let size = 20 * 1024 * 1024;
    let (ran, exit, elapsed) = run_command(
        &shell,
        &format!("yes 0123456789abcdef | head -c {size}"),
        None,
    )
    .await;
    assert_eq!(exit.unwrap().code, Some(0));
    assert_eq!(ran.stdout_bytes, size);
    assert!(
        ran.largest_chunk <= 1024 * 1024,
        "a {} byte chunk means the output is buffered",
        ran.largest_chunk
    );
    eprintln!("sftp_remote_commands: 20 MiB of output in {elapsed:?}");

    // Stop: TERM, back within about two seconds
    let (_, exit, elapsed) =
        run_command(&shell, "sleep 30", Some(Duration::from_millis(300))).await;
    let exit = exit.unwrap();
    assert!(exit.cancelled);
    assert!(elapsed < Duration::from_secs(3), "stop took {elapsed:?}");
    // a command that ignores TERM is cut off when its channel closes
    let (_, exit, elapsed) = run_command(
        &shell,
        "trap '' TERM; sleep 30",
        Some(Duration::from_millis(300)),
    )
    .await;
    assert!(exit.unwrap().cancelled);
    assert!(elapsed < Duration::from_secs(3), "stop took {elapsed:?}");

    // five commands at once while a download runs on a transfer channel; browsing still works
    let secondary = conn.open_secondary().await.unwrap();
    let download = tokio::spawn(async move {
        let mut sink = HashSink::default();
        let size = secondary
            .download(
                "/home/gem/logs/app.log",
                0,
                &mut sink,
                &|_| {},
                &AtomicBool::new(false),
            )
            .await
            .unwrap();
        (size, sink.hasher.finalize())
    });
    let mut runs = Vec::new();
    for n in 0..5 {
        let shell = Arc::clone(&shell);
        runs.push(tokio::spawn(async move {
            let (ran, exit, _) =
                run_command(&shell, &format!("sleep 0.3; echo run-{n}"), None).await;
            (n, ran.stdout, exit.unwrap().code)
        }));
    }
    for run in runs {
        let (n, stdout, code) = run.await.unwrap();
        assert_eq!(stdout, format!("run-{n}\n").as_bytes());
        assert_eq!(code, Some(0));
    }
    assert!(!conn.fs().list(HOME).await.unwrap().is_empty());
    let (size, hash) = download.await.unwrap();
    assert_eq!(size, LOG_SIZE);
    let mut expected = Sha256::new();
    for offset in (0..LOG_SIZE).step_by(8 * 1024 * 1024) {
        expected.update(log_bytes(offset, 8 * 1024 * 1024));
    }
    assert_eq!(hash, expected.finalize());

    // the log has the commands and how they ended, never their output
    let texts = capture.texts();
    assert!(
        texts
            .iter()
            .any(|text| text == "exec echo hi; echo err >&2; exit 3")
    );
    for expected in ["exit 3", "exit 127", "signal TERM", "cancelled"] {
        assert!(
            texts.iter().any(|text| text == expected),
            "{expected} not logged"
        );
    }
    for text in &texts {
        assert!(!text.contains("marker-91x"), "output in the log: {text}");
        assert!(!text.contains("0123456789abcdef0123"), "output in the log");
        assert!(
            !text.contains("run-0\n") && text != "hi",
            "output in the log: {text}"
        );
    }
    capture.assert_no_secrets("remote commands");

    // after close: Disconnected
    conn.close().await;
    let (_, exit, _) = run_command(&shell, "echo late", None).await;
    assert_eq!(code(exit), ErrorCode::Disconnected);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ftp_has_no_shell() {
    if !enabled("ftp_has_no_shell") {
        return;
    }
    let params = ftp_params(VSFTPD_PORT, Protocol::Ftp, FtpsMode::Explicit);
    let conn = super::connect(&params, Verifier::new(true), Logger::noop())
        .await
        .unwrap();
    assert!(conn.shell().is_none());
    assert!(!conn.fs().capabilities().exec);
    conn.close().await;
}
