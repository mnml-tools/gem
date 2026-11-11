//! Transfer engine tests against the in-memory server and a temporary local folder.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::remote::memory::MemoryServer;
use crate::services::test_support::{Events, eventually, recorder};

struct Harness {
    server: Arc<MemoryServer>,
    queue: Arc<TransferQueue>,
    fs: Arc<dyn RemoteFs>,
    events: Events,
    log: Arc<std::sync::Mutex<Vec<String>>>,
    dir: tempfile::TempDir,
}

fn harness() -> Harness {
    let server = MemoryServer::new();
    let connection = server.connect();
    let (events, sink) = recorder();
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let lines = Arc::clone(&log);
    let logger = Logger::new(move |line| lines.lock().unwrap().push(line.text));
    let queue = TransferQueue::new("session".into(), sink, logger);
    queue.set_connection(Some(Arc::clone(&connection) as Arc<dyn Connection>));
    Harness {
        fs: connection.fs(),
        server,
        queue,
        events,
        log,
        dir: tempfile::tempdir().unwrap(),
    }
}

fn settings(policy: ConflictPolicy, concurrency: u8) -> Settings {
    Settings {
        conflict_policy: policy,
        transfer_concurrency: concurrency,
        ..Settings::default()
    }
}

impl Harness {
    fn folder(&self) -> &Path {
        self.dir.path()
    }

    fn get(&self, id: &str) -> Transfer {
        self.queue
            .snapshot()
            .into_iter()
            .find(|t| t.id == id)
            .unwrap()
    }

    async fn wait_state(&self, id: &str, state: TransferState) -> Transfer {
        eventually(&format!("{state:?}"), || self.get(id).state == state).await;
        self.get(id)
    }

    async fn wait_finished(&self) {
        eventually("all transfers finished", || {
            self.queue.snapshot().iter().all(|t| t.state.is_finished())
        })
        .await;
    }

    async fn download(&self, paths: &[&str], settings: &Settings) -> Vec<Transfer> {
        let paths: Vec<String> = paths.iter().map(|p| (*p).to_owned()).collect();
        self.queue
            .enqueue_downloads(&*self.fs, &paths, self.folder(), settings)
            .await
            .unwrap()
    }

    async fn upload(&self, paths: &[&Path], dir: &str, settings: &Settings) -> Vec<Transfer> {
        let paths = paths.iter().map(|p| p.to_path_buf()).collect();
        self.queue
            .enqueue_uploads(&*self.fs, paths, dir, settings)
            .await
            .unwrap()
    }

    fn local_files(&self) -> Vec<String> {
        let mut found = Vec::new();
        let mut stack = vec![self.folder().to_path_buf()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    found.push(
                        path.strip_prefix(self.folder())
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/"),
                    );
                }
            }
        }
        found.sort();
        found
    }

    fn remote_temp_files(&self) -> Vec<String> {
        self.server
            .paths()
            .into_iter()
            .filter(|p| p.contains(".gem-"))
            .collect()
    }
}

fn big(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

#[tokio::test]
async fn downloads_files_and_folders() {
    let h = harness();
    h.server.add_file("/home/user/a.txt", b"alpha");
    h.server.add_dir("/home/user/site");
    h.server.add_dir("/home/user/site/css");
    h.server.add_file("/home/user/site/index.html", b"<p>");
    h.server.add_file("/home/user/site/css/a.css", b"a{}");
    h.server.add_link("/home/user/site/loop", "/home/user/site");
    let queued = h
        .download(
            &["/home/user/a.txt", "/home/user/site"],
            &settings(ConflictPolicy::Ask, 2),
        )
        .await;
    assert_eq!(queued.len(), 3, "the folder link is not followed");
    assert!(queued.iter().all(|t| t.state == TransferState::Queued));
    h.wait_finished().await;
    for t in h.queue.snapshot() {
        assert_eq!(t.state, TransferState::Done, "{t:?}");
        assert_eq!(t.done, t.size.unwrap());
    }
    assert_eq!(
        h.local_files(),
        ["a.txt", "site/css/a.css", "site/index.html"]
    );
    assert_eq!(std::fs::read(h.folder().join("a.txt")).unwrap(), b"alpha");
    // The remote modification time is kept.
    let modified = std::fs::metadata(h.folder().join("a.txt"))
        .unwrap()
        .modified()
        .unwrap();
    let remote = h.fs.stat("/home/user/a.txt").await.unwrap().mtime.unwrap();
    assert_eq!(
        modified
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        remote as u64
    );
}

#[tokio::test]
async fn untrusted_remote_names_are_sanitised() {
    let h = harness();
    h.server.add_dir("/home/user/odd");
    h.server.add_file("/home/user/odd/a\\b\n.txt", b"x");
    h.download(&["/home/user/odd"], &settings(ConflictPolicy::Ask, 1))
        .await;
    h.wait_finished().await;
    assert_eq!(h.local_files(), ["odd/a_b_.txt"]);
}

#[tokio::test]
async fn uploads_files_and_folders_through_temp_files() {
    let h = harness();
    let src = tempfile::tempdir().unwrap();
    let site = src.path().join("site");
    std::fs::create_dir_all(site.join("css")).unwrap();
    std::fs::write(site.join("index.html"), b"<p>").unwrap();
    std::fs::write(site.join("css/a.css"), big(100_000)).unwrap();
    let note = src.path().join("note.txt");
    std::fs::write(&note, b"hi").unwrap();
    h.server.add_dir("/home/user/up");
    h.server.add_dir("/home/user/up/site"); // an existing folder is merged into
    let queued = h
        .upload(
            &[&site, &note],
            "/home/user/up",
            &settings(ConflictPolicy::Ask, 2),
        )
        .await;
    assert_eq!(queued.len(), 3);
    h.wait_finished().await;
    assert!(
        h.queue
            .snapshot()
            .iter()
            .all(|t| t.state == TransferState::Done)
    );
    assert_eq!(h.server.file("/home/user/up/note.txt").unwrap(), b"hi");
    assert_eq!(
        h.server.file("/home/user/up/site/index.html").unwrap(),
        b"<p>"
    );
    assert_eq!(
        h.server.file("/home/user/up/site/css/a.css").unwrap(),
        big(100_000)
    );
    assert!(h.remote_temp_files().is_empty());
}

#[tokio::test]
async fn download_conflict_policies() {
    let h = harness();
    h.server.add_file("/home/user/a.txt", b"remote");
    let existing = h.folder().join("a.txt");
    std::fs::write(&existing, b"local").unwrap();

    let skip = h
        .download(&["/home/user/a.txt"], &settings(ConflictPolicy::Skip, 1))
        .await;
    h.wait_state(&skip[0].id, TransferState::Skipped).await;
    assert_eq!(std::fs::read(&existing).unwrap(), b"local");

    for expected in ["a (1).txt", "a (2).txt"] {
        let renamed = h
            .download(&["/home/user/a.txt"], &settings(ConflictPolicy::Rename, 1))
            .await;
        let done = h.wait_state(&renamed[0].id, TransferState::Done).await;
        assert_eq!(done.name, expected);
        assert!(done.local_path.ends_with(expected));
        assert_eq!(std::fs::read(h.folder().join(expected)).unwrap(), b"remote");
    }
    assert_eq!(std::fs::read(&existing).unwrap(), b"local");

    let asked = h
        .download(&["/home/user/a.txt"], &settings(ConflictPolicy::Ask, 1))
        .await;
    h.wait_state(&asked[0].id, TransferState::Conflict).await;
    assert!(
        h.queue
            .resolve(&asked[0].id, ConflictPolicy::Ask, false)
            .is_err()
    );
    h.queue
        .resolve(&asked[0].id, ConflictPolicy::Overwrite, false)
        .unwrap();
    h.wait_state(&asked[0].id, TransferState::Done).await;
    assert_eq!(std::fs::read(&existing).unwrap(), b"remote");
    assert!(
        !h.local_files()
            .iter()
            .any(|f| f.ends_with(local::PART_SUFFIX))
    );
}

#[tokio::test]
async fn apply_to_all_answers_the_rest_of_the_batch() {
    let h = harness();
    for name in ["a", "b", "c"] {
        h.server.add_file(&format!("/home/user/{name}"), b"remote");
        std::fs::write(h.folder().join(name), b"local").unwrap();
    }
    let queued = h
        .download(
            &["/home/user/a", "/home/user/b", "/home/user/c"],
            &settings(ConflictPolicy::Ask, 1),
        )
        .await;
    h.wait_state(&queued[0].id, TransferState::Conflict).await;
    h.queue
        .resolve(&queued[0].id, ConflictPolicy::Skip, true)
        .unwrap();
    h.wait_finished().await;
    for t in h.queue.snapshot() {
        assert_eq!(t.state, TransferState::Skipped, "{}", t.name);
    }
    // A new batch asks again.
    let next = h
        .download(&["/home/user/a"], &settings(ConflictPolicy::Ask, 1))
        .await;
    h.wait_state(&next[0].id, TransferState::Conflict).await;
}

#[tokio::test]
async fn upload_conflicts_keep_permissions_or_number_the_name() {
    let h = harness();
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("run.sh");
    std::fs::write(&file, b"new").unwrap();
    h.server.add_file("/home/user/run.sh", b"old");
    h.server.set_mode("/home/user/run.sh", 0o750);

    let over = h
        .upload(
            &[&file],
            "/home/user",
            &settings(ConflictPolicy::Overwrite, 1),
        )
        .await;
    h.wait_state(&over[0].id, TransferState::Done).await;
    assert_eq!(h.server.file("/home/user/run.sh").unwrap(), b"new");
    assert_eq!(h.server.mode("/home/user/run.sh"), Some(0o750));

    let renamed = h
        .upload(&[&file], "/home/user", &settings(ConflictPolicy::Rename, 1))
        .await;
    let done = h.wait_state(&renamed[0].id, TransferState::Done).await;
    assert_eq!(done.remote_path, "/home/user/run (1).sh");
    assert_eq!(h.server.file("/home/user/run (1).sh").unwrap(), b"new");

    let asked = h
        .upload(&[&file], "/home/user", &settings(ConflictPolicy::Ask, 1))
        .await;
    h.wait_state(&asked[0].id, TransferState::Conflict).await;
    h.queue
        .resolve(&asked[0].id, ConflictPolicy::Skip, false)
        .unwrap();
    h.wait_state(&asked[0].id, TransferState::Skipped).await;
    assert!(h.remote_temp_files().is_empty());
}

#[tokio::test]
async fn cancelling_a_download_leaves_no_local_file() {
    let h = harness();
    h.server.add_file("/home/user/big.log", &big(1024 * 1024));
    h.server.slow_reads(Some(Duration::from_millis(10)));
    let queued = h
        .download(&["/home/user/big.log"], &settings(ConflictPolicy::Ask, 1))
        .await;
    eventually("progress", || h.get(&queued[0].id).done > 0).await;
    assert_eq!(h.get(&queued[0].id).state, TransferState::Running);
    h.queue.cancel(&queued[0].id).unwrap();
    h.wait_state(&queued[0].id, TransferState::Cancelled).await;
    assert!(h.local_files().is_empty(), "{:?}", h.local_files());
}

#[tokio::test]
async fn cancelling_an_upload_leaves_no_remote_file() {
    let h = harness();
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("big.bin");
    std::fs::write(&file, big(1024 * 1024)).unwrap();
    h.server.slow_reads(Some(Duration::from_millis(10)));
    let queued = h
        .upload(&[&file], "/home/user", &settings(ConflictPolicy::Ask, 1))
        .await;
    eventually("progress", || h.get(&queued[0].id).done > 0).await;
    h.queue.cancel(&queued[0].id).unwrap();
    h.wait_state(&queued[0].id, TransferState::Cancelled).await;
    assert!(h.remote_temp_files().is_empty());
    assert!(!h.server.exists("/home/user/big.bin"));
}

#[tokio::test]
async fn a_broken_upload_is_cleaned_up_and_can_be_retried() {
    let h = harness();
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("data.bin");
    std::fs::write(&file, big(200_000)).unwrap();
    h.server.add_file("/home/user/data.bin", b"original");
    h.server.fail_uploads_after(Some(50_000));
    let queued = h
        .upload(
            &[&file],
            "/home/user",
            &settings(ConflictPolicy::Overwrite, 1),
        )
        .await;
    let failed = h.wait_state(&queued[0].id, TransferState::Failed).await;
    assert!(failed.error.unwrap().contains("connection lost"));
    assert!(h.remote_temp_files().is_empty());
    assert_eq!(h.server.file("/home/user/data.bin").unwrap(), b"original");

    h.server.fail_uploads_after(None);
    h.queue.retry(&queued[0].id).unwrap();
    let done = h.wait_state(&queued[0].id, TransferState::Done).await;
    assert_eq!(done.error, None);
    assert_eq!(h.server.file("/home/user/data.bin").unwrap(), big(200_000));
}

#[tokio::test]
async fn a_dropped_connection_fails_the_download_without_a_partial_file() {
    let h = harness();
    h.server.add_file("/home/user/big.log", &big(512 * 1024));
    h.server.slow_reads(Some(Duration::from_millis(5)));
    let queued = h
        .download(&["/home/user/big.log"], &settings(ConflictPolicy::Ask, 1))
        .await;
    eventually("progress", || h.get(&queued[0].id).done > 0).await;
    h.server.drop_connection();
    h.wait_state(&queued[0].id, TransferState::Failed).await;
    assert!(h.local_files().is_empty());

    h.server.restore();
    h.server.slow_reads(None);
    h.queue.retry(&queued[0].id).unwrap();
    h.wait_state(&queued[0].id, TransferState::Done).await;
    assert_eq!(h.local_files(), ["big.log"]);
    assert_eq!(
        h.server.secondaries_opened(),
        2,
        "the broken channel was replaced"
    );
}

#[tokio::test]
async fn concurrency_is_limited_and_channels_are_reused() {
    let h = harness();
    let paths: Vec<String> = (0..6).map(|i| format!("/home/user/f{i}")).collect();
    for path in &paths {
        h.server.add_file(path, &big(64 * 1024));
    }
    h.server.slow_reads(Some(Duration::from_millis(5)));
    let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
    h.download(&refs, &settings(ConflictPolicy::Ask, 2)).await;
    h.wait_finished().await;
    assert!(
        h.queue
            .snapshot()
            .iter()
            .all(|t| t.state == TransferState::Done)
    );
    assert_eq!(h.server.max_active(), 2);
    assert_eq!(h.server.secondaries_opened(), 2);
}

#[tokio::test]
async fn a_failed_channel_is_recreated_for_the_next_job() {
    let h = harness();
    h.server.add_file("/home/user/a", b"a");
    h.server.fail_next_secondaries(1);
    let queued = h
        .download(&["/home/user/a"], &settings(ConflictPolicy::Ask, 1))
        .await;
    let failed = h.wait_state(&queued[0].id, TransferState::Failed).await;
    assert!(failed.error.unwrap().contains("channel"));
    h.queue.retry(&queued[0].id).unwrap();
    h.wait_state(&queued[0].id, TransferState::Done).await;
    assert_eq!(h.server.secondaries_opened(), 1);
}

#[tokio::test]
async fn retry_and_clear_rules() {
    let h = harness();
    h.server.add_file("/home/user/a", b"a");
    let queued = h
        .download(&["/home/user/a"], &settings(ConflictPolicy::Ask, 1))
        .await;
    h.wait_state(&queued[0].id, TransferState::Done).await;
    assert_eq!(
        h.queue.retry(&queued[0].id).unwrap_err().code(),
        crate::error::ErrorCode::Validation
    );
    // Cancelling a finished job changes nothing.
    h.queue.cancel(&queued[0].id).unwrap();
    assert_eq!(h.get(&queued[0].id).state, TransferState::Done);
    h.queue.clear_finished();
    assert!(h.queue.snapshot().is_empty());
    assert!(!h.queue.contains(&queued[0].id));
}

#[tokio::test]
async fn queued_jobs_wait_for_a_connection() {
    let h = harness();
    h.server.add_file("/home/user/a", b"a");
    h.queue.set_connection(None);
    let queued = h
        .download(&["/home/user/a"], &settings(ConflictPolicy::Ask, 1))
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(h.get(&queued[0].id).state, TransferState::Queued);
    // Cancel works while waiting.
    let other = h
        .download(&["/home/user/a"], &settings(ConflictPolicy::Rename, 1))
        .await;
    h.queue.cancel(&other[0].id).unwrap();
    assert_eq!(h.get(&other[0].id).state, TransferState::Cancelled);

    h.queue
        .set_connection(Some(h.server.connect() as Arc<dyn Connection>));
    h.wait_state(&queued[0].id, TransferState::Done).await;
}

#[tokio::test]
async fn progress_events_are_throttled_and_only_carry_changes() {
    let h = harness();
    h.server.add_file("/home/user/first", b"1");
    h.server.add_file("/home/user/big.log", &big(1024 * 1024));
    let first = h
        .download(&["/home/user/first"], &settings(ConflictPolicy::Ask, 1))
        .await;
    h.wait_state(&first[0].id, TransferState::Done).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let before = h.events.lock().unwrap().len();

    h.server.slow_reads(Some(Duration::from_millis(5)));
    let started = std::time::Instant::now();
    let second = h
        .download(&["/home/user/big.log"], &settings(ConflictPolicy::Ask, 1))
        .await;
    h.wait_state(&second[0].id, TransferState::Done).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let elapsed = started.elapsed();

    let events = h.events.lock().unwrap();
    let batches: Vec<&Vec<Transfer>> = events[before..]
        .iter()
        .filter_map(|e| match e {
            SessionEvent::Transfers { transfers } => Some(transfers),
            _ => None,
        })
        .collect();
    let max = (elapsed.as_millis() / 100 + 2) as usize;
    assert!(
        batches.len() >= 2 && batches.len() <= max,
        "{} batches in {elapsed:?}",
        batches.len()
    );
    assert!(
        batches
            .iter()
            .flat_map(|b| b.iter())
            .all(|t| t.id == second[0].id)
    );
    let last = batches.last().unwrap().last().unwrap();
    assert_eq!(last.state, TransferState::Done);
}

#[tokio::test]
async fn shutdown_cancels_everything() {
    let h = harness();
    h.server.add_file("/home/user/big.log", &big(1024 * 1024));
    h.server.add_file("/home/user/b", b"b");
    h.server.slow_reads(Some(Duration::from_millis(10)));
    let queued = h
        .download(
            &["/home/user/big.log", "/home/user/b"],
            &settings(ConflictPolicy::Ask, 1),
        )
        .await;
    eventually("progress", || h.get(&queued[0].id).done > 0).await;
    h.queue.shutdown().await;
    h.wait_finished().await;
    assert!(
        h.queue
            .snapshot()
            .iter()
            .all(|t| t.state == TransferState::Cancelled)
    );
    assert!(h.local_files().is_empty());
}

#[tokio::test]
async fn overwriting_uploads_finish_with_an_atomic_replace() {
    let h = harness();
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("conf.ini");
    std::fs::write(&file, b"new").unwrap();
    h.server.add_file("/home/user/conf.ini", b"old");
    h.server.set_mode("/home/user/conf.ini", 0o640);

    // A failing replace keeps the old file and removes the upload.
    h.server.fail_next_replace();
    let failed = h
        .upload(
            &[&file],
            "/home/user",
            &settings(ConflictPolicy::Overwrite, 1),
        )
        .await;
    let failed = h.wait_state(&failed[0].id, TransferState::Failed).await;
    assert!(failed.error.unwrap().contains("replace failed"));
    assert_eq!(h.server.file("/home/user/conf.ini").unwrap(), b"old");
    assert!(h.remote_temp_files().is_empty());

    h.queue.retry(&failed.id).unwrap();
    h.wait_state(&failed.id, TransferState::Done).await;
    assert_eq!(h.server.file("/home/user/conf.ini").unwrap(), b"new");
    assert_eq!(h.server.mode("/home/user/conf.ini"), Some(0o640));
    assert!(h.remote_temp_files().is_empty());
    assert!(h.log.lock().unwrap().is_empty(), "no fallback was needed");
}

#[tokio::test]
async fn without_replace_overwrites_fall_back_to_remove_and_rename() {
    let h = harness();
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("conf.ini");
    std::fs::write(&file, b"new").unwrap();
    h.server.add_file("/home/user/conf.ini", b"old");
    h.server.disable_replace();
    let queued = h
        .upload(
            &[&file],
            "/home/user",
            &settings(ConflictPolicy::Overwrite, 1),
        )
        .await;
    h.wait_state(&queued[0].id, TransferState::Done).await;
    assert_eq!(h.server.file("/home/user/conf.ini").unwrap(), b"new");
    assert!(h.remote_temp_files().is_empty());
    let log = h.log.lock().unwrap();
    assert!(
        log.iter()
            .any(|l| l.contains("cannot replace a file in one step")),
        "{log:?}"
    );
}
