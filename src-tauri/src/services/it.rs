//! End-to-end tests of the service layer against the Docker test servers in `dev/servers`.
//!
//! Skipped (with a note) unless `GEM_IT=1`; run them with `pnpm servers:up && pnpm rust:it`.
//! Each test drives the real [`SessionManager`] (with [`RemoteConnector`]) and the services the
//! commands call, with an in-memory database, the in-memory secret store and a recording event
//! sink: sites and secrets, trust on first use, the editor on every encoding fixture and the
//! 200 MiB log, saves (round trips, conflicts, the in-place fallback), file operations, and the
//! transfer queue (folder trees both ways, conflict policies, cancel), then a dropped
//! connection. It runs for SFTP, FTP (vsftpd), explicit FTPS (ProFTPD, MLSD) and implicit FTPS
//! (vsftpd).
//!
//! Everything is created in a per-run folder under `/home/gem/it` and deleted at the end, also
//! when a check fails. `readonly/editable.txt` is rewritten in place and restored, under a lock;
//! `pnpm rust:it` runs this suite after (not alongside) `remote::it`, which does the same.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::commands::{self, CommandSink};
use super::secrets::{MemoryStore, SecretStore};
use super::sessions::{Connector, RemoteConnector, Session, SessionManager};
use super::test_support::{eventually, has_log, log_text, recorder, wait_for};
use super::{editor, files, known_hosts, sites};
use crate::domain::command::{CommandEvent, CommandExit, CommandStream};
use crate::domain::entry::EntryKind;
use crate::domain::file::{LineEnding, OpenedFile, ReadOnlyReason, SaveFile};
use crate::domain::session::{ConnectTarget, SessionEvent, SessionInfo, SessionState};
use crate::domain::settings::Settings;
use crate::domain::site::{AuthMethod, FtpsMode, Protocol, SiteInput};
use crate::domain::transfer::{ConflictPolicy, Transfer, TransferState};
use crate::error::{AppError, ErrorCode};
use crate::remote::RemoteFs;
// `readonly/editable.txt` is shared with `remote::it`: one lock for both modules.
use crate::remote::it::EDITABLE;
use crate::storage::Database;
use crate::text;

const HOST: &str = "127.0.0.1";
const USER: &str = "gem";
const PASSWORD: &str = "gem-test-password";
const HOME: &str = "/home/gem";
const LOG: &str = "/home/gem/logs/app.log";
const LOG_SIZE: u64 = 200 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;
/// The default preview window (`Settings::preview_window_kib`).
const WINDOW_KIB: u32 = 2048;

#[derive(Clone, Copy)]
struct Target {
    name: &'static str,
    protocol: Protocol,
    port: u16,
    mode: FtpsMode,
}

const SFTP: Target = Target {
    name: "sftp",
    protocol: Protocol::Sftp,
    port: 2222,
    mode: FtpsMode::Explicit,
};
const VSFTPD_FTP: Target = Target {
    name: "vsftpd-ftp",
    protocol: Protocol::Ftp,
    port: 2121,
    mode: FtpsMode::Explicit,
};
const PROFTPD_FTPS: Target = Target {
    name: "proftpd-ftps",
    protocol: Protocol::Ftps,
    port: 2321,
    mode: FtpsMode::Explicit,
};
const VSFTPD_FTPS_IMPLICIT: Target = Target {
    name: "vsftpd-ftps-implicit",
    protocol: Protocol::Ftps,
    port: 2990,
    mode: FtpsMode::Implicit,
};

fn enabled(test: &str) -> bool {
    let on = std::env::var("GEM_IT").is_ok_and(|value| value == "1");
    if !on {
        eprintln!("{test}: skipped (set GEM_IT=1 and start the servers with `pnpm servers:up`)");
    }
    on
}

fn seed(relative: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../dev/servers/seed")
        .join(relative);
    std::fs::read(path).unwrap()
}

/// The bytes of `logs/app.log` at `offset` (the format of `dev/servers/seeder/seed.sh`).
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

fn unique() -> String {
    let id = uuid::Uuid::now_v7().simple().to_string();
    id[id.len() - 10..].to_owned()
}

fn site_input(target: Target) -> SiteInput {
    SiteInput {
        name: format!("it {}", target.name),
        protocol: target.protocol,
        host: HOST.into(),
        port: target.port,
        user: USER.into(),
        auth: AuthMethod::Password,
        key_path: None,
        ftps_mode: target.mode,
        passive: true,
        initial_path: None,
        color: None,
    }
}

fn code<T>(result: Result<T, AppError>) -> ErrorCode {
    match result {
        Ok(_) => panic!("expected an error"),
        Err(error) => error.code(),
    }
}

struct World {
    db: Arc<Database>,
    store: Arc<MemoryStore>,
    manager: SessionManager,
}

fn world(keepalive: Option<Duration>) -> World {
    let store = Arc::new(MemoryStore::default());
    let mut manager = SessionManager::new(
        Arc::new(RemoteConnector) as Arc<dyn Connector>,
        Arc::clone(&store) as Arc<dyn SecretStore>,
    );
    if let Some(period) = keepalive {
        manager = manager.with_keepalive_period(period);
    }
    World {
        db: Arc::new(Database::open_in_memory().unwrap()),
        store,
        manager,
    }
}

impl World {
    async fn connect(
        &self,
        target: ConnectTarget,
        secret: Option<&str>,
        remember: bool,
        events: &super::sessions::EventSink,
    ) -> Result<SessionInfo, AppError> {
        self.manager
            .connect(
                Arc::clone(&self.db),
                target,
                secret.map(str::to_owned),
                remember,
                Arc::clone(events),
            )
            .await
    }

    /// Connects ad hoc, trusting the server's key or certificate on first use.
    async fn connect_trusting(
        &self,
        target: Target,
        events: &super::sessions::EventSink,
    ) -> SessionInfo {
        let adhoc = || ConnectTarget::Adhoc {
            site: site_input(target),
        };
        match self.connect(adhoc(), Some(PASSWORD), false, events).await {
            Ok(info) => info,
            Err(error) => {
                let key = error
                    .host_key()
                    .unwrap_or_else(|| panic!("{}: connect failed: {error}", target.name))
                    .clone();
                self.db.write(|tx| known_hosts::trust(tx, &key)).unwrap();
                self.connect(adhoc(), Some(PASSWORD), false, events)
                    .await
                    .unwrap()
            }
        }
    }
}

fn save_request(opened: &OpenedFile, text: &str) -> SaveFile {
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

fn settings(policy: ConflictPolicy) -> Settings {
    Settings {
        conflict_policy: policy,
        transfer_concurrency: 2,
        ..Settings::default()
    }
}

/// Every file under `dir`, relative, with its bytes (local side).
fn local_tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let relative = path
                    .strip_prefix(dir)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                found.push((relative.replace('\\', "/"), std::fs::read(&path).unwrap()));
            }
        }
    }
    found.sort();
    found
}

/// Remote paths under `dir` (recursively) whose name marks a temp or partial file.
async fn remote_leftovers(fs: &dyn RemoteFs, dir: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(current) = stack.pop() {
        for entry in fs.list(&current).await.unwrap() {
            if entry.name.contains(".gem-") {
                found.push(entry.path.clone());
            }
            if entry.kind == EntryKind::Dir {
                stack.push(entry.path);
            }
        }
    }
    found
}

async fn wait_transfers(session: &Session, ids: &[String], state: TransferState) -> Vec<Transfer> {
    let snapshot = || -> Vec<Transfer> {
        session
            .transfers
            .snapshot()
            .into_iter()
            .filter(|t| ids.contains(&t.id))
            .collect()
    };
    for _ in 0..12_000 {
        let now = snapshot();
        if now.iter().all(|t| t.state == state) {
            return now;
        }
        if let Some(stuck) = now
            .iter()
            .find(|t| t.state.is_finished() && t.state != state)
        {
            panic!(
                "transfer ended as {:?} instead of {state:?}: {stuck:?}",
                stuck.state
            );
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("transfers never reached {state:?}: {:?}", snapshot());
}

async fn read_file(fs: &dyn RemoteFs, dir: &str, name: &str) -> Vec<u8> {
    fs.read_range(&format!("{dir}/{name}"), 0, MIB)
        .await
        .unwrap()
}

fn ids(transfers: &[Transfer]) -> Vec<String> {
    transfers.iter().map(|t| t.id.clone()).collect()
}

// ----- The suite ---------------------------------------------------------------------------

async fn suite(target: Target) {
    let name = target.name;
    let w = Arc::new(world(None));
    let (events, sink) = recorder();

    // A saved site without a secret: connecting asks for one.
    let site = sites::create(&w.db, w.store.as_ref(), site_input(target), None).unwrap();
    let saved = || ConnectTarget::Site {
        id: site.id.clone(),
    };
    assert_eq!(
        code(w.connect(saved(), None, true, &sink).await),
        ErrorCode::SecretRequired,
        "{name}"
    );

    // First contact: the key or certificate is unknown; nothing is remembered yet.
    if target.protocol != Protocol::Ftp {
        let error = w
            .connect(saved(), Some(PASSWORD), true, &sink)
            .await
            .unwrap_err();
        let expected = match target.protocol {
            Protocol::Sftp => ErrorCode::HostKeyUnknown,
            _ => ErrorCode::CertificateUntrusted,
        };
        assert_eq!(error.code(), expected, "{name}: {error}");
        let key = error.host_key().unwrap().clone();
        assert_eq!((key.host.as_str(), key.port), (HOST, target.port));
        assert!(key.fingerprint.starts_with("SHA256:"), "{name}");
        assert_eq!(w.store.get(&site.id).unwrap(), None, "{name}");
        w.db.write(|tx| known_hosts::trust(tx, &key)).unwrap();
    }

    // A wrong password with "remember": refused, still nothing stored.
    assert_eq!(
        code(
            w.connect(saved(), Some("wrong-password"), true, &sink)
                .await
        ),
        ErrorCode::AuthFailed,
        "{name}"
    );
    assert_eq!(w.store.get(&site.id).unwrap(), None, "{name}");
    assert!(!w.db.read(|c| sites::get(c, &site.id)).unwrap().has_secret);

    // The right one is remembered once it worked.
    let first = w
        .connect(saved(), Some(PASSWORD), true, &sink)
        .await
        .unwrap();
    assert_eq!(
        w.store.get(&site.id).unwrap().as_deref(),
        Some(PASSWORD),
        "{name}"
    );
    assert!(w.db.read(|c| sites::get(c, &site.id)).unwrap().has_secret);
    assert_eq!(first.home, HOME, "{name}");
    assert_eq!(first.start_path, HOME, "{name}");
    assert_eq!(first.insecure, target.protocol == Protocol::Ftp, "{name}");
    assert_eq!(first.site_id.as_deref(), Some(site.id.as_str()));
    w.manager.disconnect(&first.id).await.unwrap();

    // From now on the stored secret is used.
    let info = w.connect(saved(), None, false, &sink).await.unwrap();
    let session = w.manager.get(&info.id).unwrap();
    let fs = session.fs().unwrap();
    let scratch = format!("{HOME}/it/services-{name}-{}", unique());
    files::make_dir(&*fs, &scratch).await.unwrap();

    let checks = tokio::spawn(checks(target, Arc::clone(&session), scratch.clone()));
    let outcome = checks.await;

    // Clean up whatever happened.
    let fs = session.fs().unwrap_or(fs);
    let report = files::delete_paths(&*fs, HOME, std::slice::from_ref(&scratch)).await;
    let gone = fs.stat(&scratch).await;
    w.manager.disconnect(&info.id).await.unwrap();
    if let Err(error) = outcome {
        std::panic::resume_unwind(error.into_panic());
    }
    assert!(
        report.failed.is_empty(),
        "{name}: cleanup {:?}",
        report.failed
    );
    assert_eq!(
        code(gone),
        ErrorCode::NotFound,
        "{name}: scratch folder left"
    );

    // The log streamed to the tab, and no line carries the password.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let seen = events.lock().unwrap().clone();
    assert!(
        seen.iter().any(|e| matches!(e, SessionEvent::Log { .. })),
        "{name}: no log"
    );
    assert!(seen.iter().any(|e| matches!(
        e,
        SessionEvent::State {
            state: SessionState::Connected,
            ..
        }
    )));
    let log = log_text(&seen);
    eprintln!(
        "{name}: overwriting uploads finished with {}",
        if log.contains("cannot replace a file in one step") {
            "the remove + rename fallback (RemoteFs::replace unsupported)"
        } else {
            "RemoteFs::replace"
        }
    );
    assert!(
        !log.contains(PASSWORD),
        "{name}: the password reached the log"
    );
    assert!(
        !log.contains("wrong-password"),
        "{name}: a password reached the log"
    );
}

async fn checks(target: Target, session: Arc<Session>, scratch: String) {
    let fs = session.fs().unwrap();
    let name = target.name;
    browse(name, &*fs).await;
    fixtures(name, &*fs).await;
    big_log(name, &*fs).await;
    saves(name, &*fs, &scratch).await;
    read_only_folder(name, &*fs).await;
    file_operations(name, &*fs, &scratch).await;
    transfers(name, &session, &*fs, &scratch).await;
}

async fn browse(name: &str, fs: &dyn RemoteFs) {
    let listing = files::list(fs, &format!("{HOME}/./encodings/.."))
        .await
        .unwrap();
    assert_eq!(listing.path, HOME, "{name}");
    for expected in ["encodings", "logs", "samples", "readonly", "it"] {
        let entry = listing
            .entries
            .iter()
            .find(|e| e.name == expected)
            .unwrap_or_else(|| panic!("{name}: {expected} not listed"));
        assert_eq!(entry.kind, EntryKind::Dir, "{name}: {expected}");
    }
}

async fn fixtures(name: &str, fs: &dyn RemoteFs) {
    use LineEnding::{Crlf, Lf};
    // path, encoding, BOM, line ending, mixed, read-only reason
    type Expected = (
        &'static str,
        &'static str,
        bool,
        LineEnding,
        bool,
        Option<ReadOnlyReason>,
    );
    let table: &[Expected] = &[
        ("encodings/utf8.txt", "utf-8", false, Lf, false, None),
        ("encodings/utf8-bom.txt", "utf-8", true, Lf, false, None),
        (
            "encodings/no-final-newline.txt",
            "utf-8",
            false,
            Lf,
            false,
            None,
        ),
        (
            "encodings/latin1.txt",
            "windows-1252",
            false,
            Lf,
            false,
            None,
        ),
        (
            "encodings/latin1-crlf.txt",
            "windows-1252",
            false,
            Crlf,
            false,
            None,
        ),
        ("encodings/crlf.txt", "utf-8", false, Crlf, false, None),
        ("encodings/mixed-eol.txt", "utf-8", false, Lf, true, None),
        (
            "encodings/utf16le-bom.txt",
            "utf-16le",
            true,
            Crlf,
            false,
            Some(ReadOnlyReason::UnsupportedEncoding),
        ),
    ];
    for (path, encoding, bom, ending, mixed, read_only) in table {
        let opened = editor::open_file(fs, &format!("{HOME}/{path}"), false, WINDOW_KIB)
            .await
            .unwrap_or_else(|e| panic!("{name}: {path}: {e}"));
        let local = seed(path);
        assert!(opened.complete && !opened.binary, "{name}: {path}");
        assert_eq!(opened.size, local.len() as u64, "{name}: {path}");
        assert_eq!(opened.next_offset, opened.size, "{name}: {path}");
        assert_eq!(opened.encoding, *encoding, "{name}: {path}");
        assert_eq!(opened.bom, *bom, "{name}: {path}");
        assert_eq!(opened.line_ending, *ending, "{name}: {path}");
        assert_eq!(opened.mixed_line_endings, *mixed, "{name}: {path}");
        assert_eq!(opened.read_only, *read_only, "{name}: {path}");
        assert_eq!(opened.text, text::open(&local, true).text, "{name}: {path}");
    }
    let plain = editor::open_file(
        fs,
        &format!("{HOME}/encodings/no-final-newline.txt"),
        false,
        64,
    )
    .await
    .unwrap();
    assert_eq!(plain.text, "no final newline");
    for path in ["binary/bytes.bin", "binary/pixel.png"] {
        let opened = editor::open_file(fs, &format!("{HOME}/{path}"), false, WINDOW_KIB)
            .await
            .unwrap();
        assert!(opened.binary, "{name}: {path}");
        assert_eq!(
            opened.read_only,
            Some(ReadOnlyReason::Binary),
            "{name}: {path}"
        );
        assert!(opened.text.is_empty(), "{name}: {path}");
    }
    assert_eq!(
        code(editor::open_file(fs, &format!("{HOME}/samples"), false, WINDOW_KIB).await),
        ErrorCode::Validation,
        "{name}: folders are not opened"
    );
}

async fn big_log(name: &str, fs: &dyn RemoteFs) {
    let window = u64::from(WINDOW_KIB) * 1024;
    let text_of = |offset: u64, len: u64| String::from_utf8(log_bytes(offset, len)).unwrap();
    let opened = editor::open_file(fs, LOG, false, WINDOW_KIB).await.unwrap();
    assert_eq!(opened.size, LOG_SIZE, "{name}");
    assert!(!opened.complete, "{name}");
    assert_eq!(opened.read_only, Some(ReadOnlyReason::Partial), "{name}");
    assert_eq!(opened.encoding, "utf-8", "{name}");
    assert_eq!(opened.next_offset, window, "{name}");
    assert!(opened.text == text_of(0, window), "{name}: first window");

    let more = |offset: u64, to_end: bool, kib: u32| {
        editor::read_more(
            fs,
            LOG,
            offset,
            &opened.encoding,
            opened.size,
            opened.mtime,
            to_end,
            kib,
        )
    };
    let second = more(opened.next_offset, false, WINDOW_KIB).await.unwrap();
    assert!(!second.complete, "{name}");
    assert_eq!(second.next_offset, 2 * window, "{name}");
    assert!(
        second.text == text_of(window, window),
        "{name}: second window"
    );

    // A window that does not end on a line: the text still matches byte for byte.
    let odd = more(second.next_offset + 50, false, 100).await.unwrap();
    assert_eq!(
        odd.next_offset,
        second.next_offset + 50 + 100 * 1024,
        "{name}"
    );
    assert!(
        odd.text == text_of(second.next_offset + 50, 100 * 1024),
        "{name}: odd window"
    );

    let tail_start = LOG_SIZE - MIB;
    let tail = more(tail_start, true, WINDOW_KIB).await.unwrap();
    assert!(tail.complete, "{name}");
    assert_eq!(tail.next_offset, LOG_SIZE, "{name}");
    assert!(tail.text == text_of(tail_start, MIB), "{name}: last window");

    // A changed file is refused instead of mixing two versions.
    assert_eq!(
        code(
            editor::read_more(
                fs,
                LOG,
                window,
                &opened.encoding,
                opened.size - 1,
                opened.mtime,
                false,
                WINDOW_KIB,
            )
            .await
        ),
        ErrorCode::Conflict,
        "{name}"
    );
}

async fn saves(name: &str, fs: &dyn RemoteFs, scratch: &str) {
    // Latin-1 + CRLF: an edit changes only the edited bytes.
    let original = seed("encodings/latin1-crlf.txt");
    let path = format!("{scratch}/latin1-crlf.txt");
    fs.write_atomic(&path, &original, None, false)
        .await
        .unwrap();
    let opened = editor::open_file(fs, &path, true, WINDOW_KIB)
        .await
        .unwrap();
    assert_eq!(opened.read_only, None, "{name}");
    assert_eq!(
        (opened.encoding.as_str(), opened.line_ending),
        ("windows-1252", LineEnding::Crlf)
    );
    let edited = opened
        .text
        .replace("cidade=São João da Madeira", "cidade=Funchal");
    assert_ne!(edited, opened.text, "{name}: fixture changed?");
    let saved = editor::save_file(fs, &path, save_request(&opened, &edited))
        .await
        .unwrap();
    assert!(saved.atomic, "{name}");
    let expected = {
        let needle: &[u8] = b"cidade=S\xe3o Jo\xe3o da Madeira";
        let at = original
            .windows(needle.len())
            .position(|w| w == needle)
            .unwrap();
        let mut bytes = original[..at].to_vec();
        bytes.extend_from_slice(b"cidade=Funchal");
        bytes.extend_from_slice(&original[at + needle.len()..]);
        bytes
    };
    let on_server = fs.read_range(&path, 0, MIB).await.unwrap();
    assert_eq!(on_server, expected, "{name}: Latin-1 CRLF round trip");
    assert_eq!(saved.size, expected.len() as u64, "{name}");

    // Saving unchanged keeps every byte (UTF-8 BOM, no final newline).
    for fixture in ["utf8-bom.txt", "no-final-newline.txt", "utf8.txt"] {
        let bytes = seed(&format!("encodings/{fixture}"));
        let copy = format!("{scratch}/{fixture}");
        fs.write_atomic(&copy, &bytes, None, false).await.unwrap();
        let opened = editor::open_file(fs, &copy, true, WINDOW_KIB)
            .await
            .unwrap();
        editor::save_file(fs, &copy, save_request(&opened, &opened.text))
            .await
            .unwrap();
        assert_eq!(
            fs.read_range(&copy, 0, MIB).await.unwrap(),
            bytes,
            "{name}: {fixture}"
        );
    }

    // Mixed line endings: saving normalises to the dominant one (LF here).
    let mixed = format!("{scratch}/mixed-eol.txt");
    fs.write_atomic(&mixed, &seed("encodings/mixed-eol.txt"), None, false)
        .await
        .unwrap();
    let opened = editor::open_file(fs, &mixed, true, WINDOW_KIB)
        .await
        .unwrap();
    editor::save_file(fs, &mixed, save_request(&opened, &opened.text))
        .await
        .unwrap();
    assert_eq!(
        fs.read_range(&mixed, 0, MIB).await.unwrap(),
        b"unix line\nwindows line\nold mac line\nlast line without newline"
    );

    // Someone else changes the file: conflict; "Overwrite" forces it.
    let opened = editor::open_file(fs, &path, true, WINDOW_KIB)
        .await
        .unwrap();
    fs.write_atomic(&path, b"changed by someone else\r\n", None, false)
        .await
        .unwrap();
    let mine = opened.text.replace("Funchal", "Porto Santo");
    assert_eq!(
        code(editor::save_file(fs, &path, save_request(&opened, &mine)).await),
        ErrorCode::Conflict,
        "{name}"
    );
    assert_eq!(
        fs.read_range(&path, 0, MIB).await.unwrap(),
        b"changed by someone else\r\n"
    );
    let forced = SaveFile {
        force: true,
        ..save_request(&opened, &mine)
    };
    editor::save_file(fs, &path, forced).await.unwrap();
    let on_server = fs.read_range(&path, 0, MIB).await.unwrap();
    assert!(on_server.windows(11).any(|w| w == b"Porto Santo"), "{name}");
    assert!(on_server.ends_with(b"\r\n"), "{name}");

    // A character windows-1252 cannot hold: refused, nothing written.
    let before = fs.read_range(&path, 0, MIB).await.unwrap();
    let current = editor::open_file(fs, &path, true, WINDOW_KIB)
        .await
        .unwrap();
    assert_eq!(
        code(editor::save_file(fs, &path, save_request(&current, "✓\n")).await),
        ErrorCode::Validation
    );
    assert_eq!(fs.read_range(&path, 0, MIB).await.unwrap(), before);
}

async fn read_only_folder(name: &str, fs: &dyn RemoteFs) {
    let _guard = EDITABLE.lock().await;
    let path = format!("{HOME}/readonly/editable.txt");
    let original = fs.read_range(&path, 0, MIB).await.unwrap();
    let opened = editor::open_file(fs, &path, true, WINDOW_KIB)
        .await
        .unwrap();
    assert_eq!(opened.read_only, None, "{name}");
    let text = "edited in place by services::it\n";
    assert_eq!(
        code(editor::save_file(fs, &path, save_request(&opened, text)).await),
        ErrorCode::Unsupported,
        "{name}: no temp file possible, the user must agree first"
    );
    assert_eq!(fs.read_range(&path, 0, MIB).await.unwrap(), original);
    let in_place = SaveFile {
        allow_in_place: true,
        ..save_request(&opened, text)
    };
    let saved = editor::save_file(fs, &path, in_place).await.unwrap();
    assert!(!saved.atomic, "{name}");
    assert_eq!(fs.read_range(&path, 0, MIB).await.unwrap(), text.as_bytes());
    let restore = SaveFile {
        force: true,
        allow_in_place: true,
        ..save_request(&opened, &opened.text)
    };
    editor::save_file(fs, &path, restore).await.unwrap();
    assert_eq!(fs.read_range(&path, 0, MIB).await.unwrap(), original);

    let locked = format!("{HOME}/readonly/locked.txt");
    let opened = editor::open_file(fs, &locked, true, WINDOW_KIB)
        .await
        .unwrap();
    let attempt = SaveFile {
        allow_in_place: true,
        ..save_request(&opened, "x\n")
    };
    assert_eq!(
        code(editor::save_file(fs, &locked, attempt).await),
        ErrorCode::PermissionDenied,
        "{name}"
    );
    let report = files::delete_paths(fs, HOME, std::slice::from_ref(&locked)).await;
    assert_eq!(report.deleted, 0, "{name}");
    assert_eq!(report.failed.len(), 1, "{name}");
    assert_eq!(
        code(files::make_file(fs, &format!("{HOME}/readonly/new.txt")).await),
        ErrorCode::PermissionDenied,
        "{name}"
    );
}

async fn file_operations(name: &str, fs: &dyn RemoteFs, scratch: &str) {
    let ops = format!("{scratch}/ops");
    assert_eq!(
        files::make_dir(fs, &ops).await.unwrap().kind,
        EntryKind::Dir
    );
    let a = files::make_file(fs, &format!("{ops}/a.txt")).await.unwrap();
    assert_eq!((a.kind, a.size), (EntryKind::File, Some(0)), "{name}");
    assert_eq!(
        code(files::make_file(fs, &format!("{ops}/a.txt")).await),
        ErrorCode::Conflict
    );
    assert_eq!(code(files::make_dir(fs, &ops).await), ErrorCode::Conflict);
    assert_eq!(
        code(files::make_dir(fs, &format!("{ops}/../escape")).await),
        ErrorCode::Validation
    );
    files::make_dir(fs, &format!("{ops}/sub")).await.unwrap();
    files::make_file(fs, &format!("{ops}/sub/b.txt"))
        .await
        .unwrap();
    files::make_dir(fs, &format!("{ops}/sub/deeper"))
        .await
        .unwrap();
    files::make_file(fs, &format!("{ops}/sub/deeper/c.txt"))
        .await
        .unwrap();
    assert_eq!(
        code(files::rename(fs, &format!("{ops}/a.txt"), &format!("{ops}/sub/b.txt")).await),
        ErrorCode::Conflict,
        "{name}: rename never overwrites"
    );
    let renamed = files::rename(fs, &format!("{ops}/a.txt"), &format!("{ops}/a b.txt"))
        .await
        .unwrap();
    assert_eq!(renamed.name, "a b.txt", "{name}");
    if fs.capabilities().chmod {
        let entry = files::chmod(fs, &format!("{ops}/a b.txt"), 0o600)
            .await
            .unwrap();
        assert_eq!(entry.mode, Some(0o600), "{name}");
    }
    let report = files::delete_paths(fs, HOME, std::slice::from_ref(&ops)).await;
    assert!(report.failed.is_empty(), "{name}: {:?}", report.failed);
    // a b.txt, sub/b.txt, sub/deeper/c.txt, deeper, sub, ops
    assert_eq!(report.deleted, 6, "{name}");
    assert_eq!(code(fs.stat(&ops).await), ErrorCode::NotFound);
}

async fn transfers(name: &str, session: &Session, fs: &dyn RemoteFs, scratch: &str) {
    let queue = &session.transfers;
    let local = tempfile::tempdir().unwrap();
    let tree = local.path().join("tree");
    std::fs::create_dir_all(tree.join("sub/deeper")).unwrap();
    std::fs::create_dir_all(tree.join("empty")).unwrap();
    std::fs::write(tree.join("a.txt"), b"alpha\n").unwrap();
    std::fs::write(tree.join("with space.txt"), b"spaced\n").unwrap();
    let pattern: Vec<u8> = (0..300 * 1024u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    std::fs::write(tree.join("sub/b.bin"), &pattern).unwrap();
    std::fs::write(tree.join("sub/deeper/c.txt"), b"deep\r\n").unwrap();

    // Upload the tree, then download it back: same bytes.
    let up = queue
        .enqueue_uploads(
            fs,
            vec![tree.clone()],
            scratch,
            &settings(ConflictPolicy::Ask),
        )
        .await
        .unwrap();
    assert_eq!(up.len(), 4, "{name}");
    wait_transfers(session, &ids(&up), TransferState::Done).await;
    let remote_tree = format!("{scratch}/tree");
    assert!(
        fs.list(&format!("{remote_tree}/empty"))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fs.read_range(&format!("{remote_tree}/sub/b.bin"), 0, MIB)
            .await
            .unwrap(),
        pattern,
        "{name}"
    );

    let downloads = tempfile::tempdir().unwrap();
    let down = queue
        .enqueue_downloads(
            fs,
            std::slice::from_ref(&remote_tree),
            downloads.path(),
            &settings(ConflictPolicy::Ask),
        )
        .await
        .unwrap();
    wait_transfers(session, &ids(&down), TransferState::Done).await;
    assert_eq!(
        local_tree(&downloads.path().join("tree")),
        local_tree(&tree),
        "{name}: round trip"
    );
    assert!(downloads.path().join("tree/empty").is_dir(), "{name}");

    // Conflicts: ask, answered once for the whole batch (overwrite → replace).
    std::fs::write(tree.join("a.txt"), b"alpha v2\n").unwrap();
    std::fs::write(tree.join("with space.txt"), b"spaced v2\n").unwrap();
    let asked = queue
        .enqueue_uploads(
            fs,
            vec![tree.join("a.txt"), tree.join("with space.txt")],
            &remote_tree,
            &settings(ConflictPolicy::Ask),
        )
        .await
        .unwrap();
    wait_transfers(session, &ids(&asked), TransferState::Conflict).await;
    queue
        .resolve(&asked[0].id, ConflictPolicy::Overwrite, true)
        .unwrap();
    wait_transfers(session, &ids(&asked), TransferState::Done).await;
    assert_eq!(
        read_file(fs, &remote_tree, "a.txt").await,
        b"alpha v2\n",
        "{name}"
    );
    assert_eq!(
        read_file(fs, &remote_tree, "with space.txt").await,
        b"spaced v2\n",
        "{name}"
    );

    // Skip, then "keep both" numbering on the server.
    std::fs::write(tree.join("a.txt"), b"alpha v3\n").unwrap();
    let skipped = queue
        .enqueue_uploads(
            fs,
            vec![tree.join("a.txt")],
            &remote_tree,
            &settings(ConflictPolicy::Skip),
        )
        .await
        .unwrap();
    wait_transfers(session, &ids(&skipped), TransferState::Skipped).await;
    assert_eq!(
        read_file(fs, &remote_tree, "a.txt").await,
        b"alpha v2\n",
        "{name}"
    );
    for expected in ["a (1).txt", "a (2).txt"] {
        let renamed = queue
            .enqueue_uploads(
                fs,
                vec![tree.join("a.txt")],
                &remote_tree,
                &settings(ConflictPolicy::Rename),
            )
            .await
            .unwrap();
        let done = wait_transfers(session, &ids(&renamed), TransferState::Done).await;
        assert_eq!(
            done[0].remote_path,
            format!("{remote_tree}/{expected}"),
            "{name}"
        );
        assert_eq!(
            read_file(fs, &remote_tree, expected).await,
            b"alpha v3\n",
            "{name}"
        );
    }
    let local_renamed = queue
        .enqueue_downloads(
            fs,
            &[format!("{remote_tree}/a.txt")],
            &downloads.path().join("tree"),
            &settings(ConflictPolicy::Rename),
        )
        .await
        .unwrap();
    wait_transfers(session, &ids(&local_renamed), TransferState::Done).await;
    assert_eq!(
        std::fs::read(downloads.path().join("tree/a (1).txt")).unwrap(),
        b"alpha v2\n"
    );
    assert!(remote_leftovers(fs, scratch).await.is_empty(), "{name}");

    // Cancel a download mid-way: no file, not even a partial one.
    let cancel_dir = tempfile::tempdir().unwrap();
    let download = queue
        .enqueue_downloads(
            fs,
            &[LOG.to_owned()],
            cancel_dir.path(),
            &settings(ConflictPolicy::Ask),
        )
        .await
        .unwrap();
    let id = download[0].id.clone();
    eventually("download progress", || {
        queue.snapshot().iter().any(|t| t.id == id && t.done > 0)
    })
    .await;
    queue.cancel(&id).unwrap();
    let cancelled = wait_transfers(session, &ids(&download), TransferState::Cancelled).await;
    assert!(
        cancelled[0].done < LOG_SIZE,
        "{name}: finished before the cancel"
    );
    assert!(
        std::fs::read_dir(cancel_dir.path())
            .unwrap()
            .next()
            .is_none(),
        "{name}: a partial download was left"
    );

    // Cancel an upload mid-way: no file on the server, not even its temp file. A sparse
    // 1 GiB file is read as fast as the disk cache allows and uses no space.
    let big = local.path().join("big.bin");
    std::fs::File::create(&big)
        .unwrap()
        .set_len(1024 * MIB)
        .unwrap();
    let upload = queue
        .enqueue_uploads(fs, vec![big], scratch, &settings(ConflictPolicy::Ask))
        .await
        .unwrap();
    let id = upload[0].id.clone();
    eventually("upload progress", || {
        queue.snapshot().iter().any(|t| t.id == id && t.done > 0)
    })
    .await;
    queue.cancel(&id).unwrap();
    wait_transfers(session, &ids(&upload), TransferState::Cancelled).await;
    assert_eq!(
        code(fs.stat(&format!("{scratch}/big.bin")).await),
        ErrorCode::NotFound,
        "{name}"
    );
    assert!(remote_leftovers(fs, scratch).await.is_empty(), "{name}");
    queue.clear_finished();
}

/// Closing the connection from our side is noticed by the keepalive: the tab gets
/// `disconnected` with a reason, operations fail with `Disconnected`, reconnect restores it.
async fn dropped_connection(target: Target) {
    let name = target.name;
    let w = world(Some(Duration::from_millis(200)));
    let (events, sink) = recorder();
    let info = w.connect_trusting(target, &sink).await;
    let session = w.manager.get(&info.id).unwrap();
    // Forget the events of the trust-on-first-use attempt (a `disconnected` with a reason).
    events.lock().unwrap().clear();
    session.fs().unwrap().close().await;
    let seen = wait_for(&events, |e| {
        matches!(
            e,
            SessionEvent::State {
                state: SessionState::Disconnected,
                reason: Some(_)
            }
        )
    })
    .await;
    assert!(!seen.is_empty());
    assert_eq!(session.state(), SessionState::Disconnected, "{name}");
    assert_eq!(code(session.fs()), ErrorCode::Disconnected, "{name}");
    wait_for(&events, |e| has_log(e, "Connection lost")).await;

    let again = w
        .manager
        .reconnect(Arc::clone(&w.db), &info.id, Some(PASSWORD.into()))
        .await
        .unwrap();
    assert_eq!(again.id, info.id, "{name}");
    assert_eq!(session.state(), SessionState::Connected, "{name}");
    files::list(&*session.fs().unwrap(), HOME).await.unwrap();
    w.manager.disconnect(&info.id).await.unwrap();
    let last = events.lock().unwrap().last().cloned();
    assert!(
        matches!(
            last,
            Some(SessionEvent::State {
                state: SessionState::Disconnected,
                reason: None
            })
        ),
        "{name}: {last:?}"
    );
    assert!(
        !log_text(&events.lock().unwrap()).contains(PASSWORD),
        "{name}"
    );
}

/// "Test connection" reports failures with the conversation (and the key to trust).
async fn test_connection_results(target: Target) {
    let name = target.name;
    let w = world(None);
    let wrong = w
        .manager
        .test_connection(
            Arc::clone(&w.db),
            site_input(target),
            Some("wrong-password".into()),
            None,
        )
        .await
        .unwrap();
    assert!(!wrong.ok, "{name}");
    let error = wrong.error.unwrap();
    let expected = match target.protocol {
        Protocol::Sftp => ErrorCode::HostKeyUnknown,
        Protocol::Ftps => ErrorCode::CertificateUntrusted,
        Protocol::Ftp => ErrorCode::AuthFailed,
    };
    assert_eq!(error.code, expected, "{name}: {}", error.message);
    assert!(!wrong.log.is_empty(), "{name}");
    if let Some(key) = error.host_key {
        w.db.write(|tx| known_hosts::trust(tx, &key)).unwrap();
        let wrong = w
            .manager
            .test_connection(
                Arc::clone(&w.db),
                site_input(target),
                Some("wrong-password".into()),
                None,
            )
            .await
            .unwrap();
        assert_eq!(wrong.error.unwrap().code, ErrorCode::AuthFailed, "{name}");
        assert!(wrong.log.iter().all(|l| !l.text.contains("wrong-password")));
    }
    let ok = w
        .manager
        .test_connection(
            Arc::clone(&w.db),
            site_input(target),
            Some(PASSWORD.into()),
            None,
        )
        .await
        .unwrap();
    assert!(ok.ok, "{name}: {:?}", ok.error);
    assert_eq!(ok.home.as_deref(), Some(HOME), "{name}");
    assert!(ok.log.iter().all(|l| !l.text.contains(PASSWORD)), "{name}");
}

async fn all(target: Target) {
    test_connection_results(target).await;
    suite(target).await;
    dropped_connection(target).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sftp_services_end_to_end() {
    if enabled("sftp_services_end_to_end") {
        all(SFTP).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ftp_vsftpd_services_end_to_end() {
    if enabled("ftp_vsftpd_services_end_to_end") {
        all(VSFTPD_FTP).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ftps_explicit_proftpd_services_end_to_end() {
    if enabled("ftps_explicit_proftpd_services_end_to_end") {
        all(PROFTPD_FTPS).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ftps_implicit_vsftpd_services_end_to_end() {
    if enabled("ftps_implicit_vsftpd_services_end_to_end") {
        all(VSFTPD_FTPS_IMPLICIT).await;
    }
}

// ----- Remote commands (ADR 0014) --------------------------------------------------------

type CommandEvents = Arc<std::sync::Mutex<Vec<CommandEvent>>>;

fn command_recorder() -> (CommandEvents, CommandSink) {
    let events: CommandEvents = Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    (events, Arc::new(move |e| sink.lock().unwrap().push(e)))
}

/// Waits for the run's terminal event and checks it is the only one and the last.
async fn command_finished(events: &CommandEvents) -> Vec<CommandEvent> {
    let terminal =
        |e: &CommandEvent| matches!(e, CommandEvent::Exit { .. } | CommandEvent::Failed { .. });
    for _ in 0..2000 {
        if events.lock().unwrap().iter().any(terminal) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    let seen = events.lock().unwrap().clone();
    assert_eq!(seen.iter().filter(|e| terminal(e)).count(), 1, "{seen:?}");
    assert!(seen.last().is_some_and(terminal), "{seen:?}");
    seen
}

fn command_text(events: &[CommandEvent], wanted: CommandStream) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            CommandEvent::Output { stream, text } if *stream == wanted => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn command_exit(events: &[CommandEvent]) -> CommandExit {
    match events.last() {
        Some(CommandEvent::Exit { exit, .. }) => exit.clone(),
        other => panic!("expected an exit, got {other:?}"),
    }
}

async fn run_and_wait(
    session: &Arc<Session>,
    command: &str,
    dir: Option<&str>,
) -> Vec<CommandEvent> {
    let (events, sink) = command_recorder();
    commands::run(session, command, dir, sink).unwrap();
    command_finished(&events).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sftp_commands_end_to_end() {
    if !enabled("sftp_commands_end_to_end") {
        return;
    }
    let w = world(None);
    let (session_events, sink) = recorder();
    let info = w.connect_trusting(SFTP, &sink).await;
    assert!(info.capabilities.exec);
    let session = w.manager.get(&info.id).unwrap();
    let fs = session.fs().unwrap();
    let scratch = format!("{HOME}/it/services-commands-{}", unique());
    files::make_dir(&*fs, &scratch).await.unwrap();
    let dir = format!("{scratch}/it's a dir");
    files::make_dir(&*fs, &dir).await.unwrap();

    let checks = {
        let session = Arc::clone(&session);
        let dir = dir.clone();
        tokio::spawn(async move {
            // Plain output and exit status.
            let seen = run_and_wait(&session, "echo hi; printf 'olá 日本\\n'", None).await;
            assert_eq!(command_text(&seen, CommandStream::Stdout), "hi\nolá 日本\n");
            assert_eq!(command_exit(&seen).code, Some(0));

            // {dir}: a folder with a space and a quote, quoted for the shell.
            let seen = run_and_wait(&session, "cd {dir} && pwd && ls -d {dir}", Some(&dir)).await;
            assert_eq!(
                command_text(&seen, CommandStream::Stdout),
                format!("{dir}\n{dir}\n")
            );
            assert_eq!(command_exit(&seen).code, Some(0));

            // stderr and a failing exit code; ANSI colours stripped.
            let seen = run_and_wait(
                &session,
                "printf '\\033[31mred\\033[0m\\n'; echo oops >&2; exit 3",
                None,
            )
            .await;
            assert_eq!(command_text(&seen, CommandStream::Stdout), "red\n");
            assert_eq!(command_text(&seen, CommandStream::Stderr), "oops\n");
            assert_eq!(command_exit(&seen).code, Some(3));

            // Stop a long command.
            let (events, sink) = command_recorder();
            let started = commands::run(&session, "sleep 30", None, sink).unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
            let stopped_at = std::time::Instant::now();
            session.runs.cancel(&started.run_id);
            let seen = command_finished(&events).await;
            let exit = command_exit(&seen);
            assert!(exit.cancelled, "{exit:?}");
            assert!(stopped_at.elapsed() < Duration::from_secs(10));

            // More runs than the server's MaxSessions: a clear failure, the session survives.
            let mut runs = Vec::new();
            for _ in 0..40 {
                let (events, sink) = command_recorder();
                let started = commands::run(&session, "sleep 20", None, sink).unwrap();
                runs.push((started.run_id, events));
            }
            let mut refused = 0;
            for _ in 0..500 {
                refused = runs
                    .iter()
                    .filter(|(_, events)| {
                        events.lock().unwrap().iter().any(|e| {
                            matches!(e, CommandEvent::Failed { error }
                                if error.message == commands::NO_MORE_CHANNELS)
                        })
                    })
                    .count();
                if refused > 0 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            for (run_id, _) in &runs {
                session.runs.cancel(run_id);
            }
            for (_, events) in &runs {
                command_finished(events).await;
            }
            assert!(refused > 0, "no run was refused by MaxSessions");
            assert_eq!(session.state(), SessionState::Connected);
            let seen = run_and_wait(&session, "echo still here", None).await;
            assert_eq!(command_text(&seen, CommandStream::Stdout), "still here\n");
        })
    };
    let outcome = checks.await;
    let report = files::delete_paths(&*fs, HOME, std::slice::from_ref(&scratch)).await;
    w.manager.disconnect(&info.id).await.unwrap();
    if let Err(error) = outcome {
        std::panic::resume_unwind(error.into_panic());
    }
    assert!(report.failed.is_empty(), "cleanup {:?}", report.failed);
    // The connection log has the commands and their exit status, never their output.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let log = log_text(&session_events.lock().unwrap());
    assert!(log.contains("exec echo hi"), "{log}");
    assert!(!log.contains("olá 日本\n"));
    assert!(!log.contains(PASSWORD));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ftp_sessions_cannot_run_commands() {
    if !enabled("ftp_sessions_cannot_run_commands") {
        return;
    }
    let w = world(None);
    let (_, sink) = recorder();
    for target in [VSFTPD_FTP, PROFTPD_FTPS] {
        let info = w.connect_trusting(target, &sink).await;
        assert!(!info.capabilities.exec, "{}", target.name);
        let session = w.manager.get(&info.id).unwrap();
        let (_, commands_sink) = command_recorder();
        assert_eq!(
            code(commands::run(&session, "uptime", None, commands_sink)),
            ErrorCode::Unsupported,
            "{}",
            target.name
        );
        w.manager.disconnect(&info.id).await.unwrap();
    }
}
