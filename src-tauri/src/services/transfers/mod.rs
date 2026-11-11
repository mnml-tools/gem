//! The transfer queue of a session (docs/adr/0011-transfer-engine.md).
//!
//! - **Workers:** up to `settings.transfer_concurrency` jobs run at once. Each worker slot owns
//!   its own secondary channel (`Connection::open_secondary`), created lazily, reused by the
//!   next job and recreated after a connection error, so transfers never block browsing.
//! - **No partial files, ever:** downloads are written to `<name>.gem-part` and renamed into
//!   place when complete; uploads go to a hidden temp sibling (`.<name>.gem-<random>`), then are
//!   renamed into place, or moved over an existing target with `RemoteFs::replace` (remove +
//!   rename only on backends without it). Failures and cancels remove the partial file.
//! - **Folders** are expanded when queued (local walk / recursive remote listing); their
//!   directories are created first, so file jobs only ever create files.
//! - **Conflicts** follow `settings.conflict_policy`; `ask` parks the job in `conflict` until
//!   `resolve`, whose `apply_to_all` sets the policy for the rest of that batch.
//! - **Progress** reaches the tab as `SessionEvent::Transfers` with only the changed jobs, at
//!   most every ~100 ms. Data is streamed in chunks, so memory stays flat.
//! - **Dropped connection:** running jobs fail (and can be retried); queued jobs wait and
//!   resume after a reconnect.

pub mod local;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use tokio::sync::Notify;

use super::blocking;
use super::sessions::EventSink;
use crate::domain::entry::EntryKind;
use crate::domain::now_ms;
use crate::domain::session::SessionEvent;
use crate::domain::settings::Settings;
use crate::domain::transfer::{ConflictPolicy, Transfer, TransferDirection, TransferState};
use crate::error::{AppError, AppResult};
use crate::remote::path::{file_name, join, normalize, parent};
use crate::remote::{Connection, Logger, RemoteFs};

/// Transfer events are sent at most this often.
const FLUSH_EVERY: Duration = Duration::from_millis(100);

/// Highest number tried for "keep both" names before giving up.
const MAX_RENAME: u32 = 9999;

struct Job {
    transfer: Transfer,
    batch: u64,
    /// The settings' policy when the job was queued.
    policy: ConflictPolicy,
    /// The user's answer to this job's conflict.
    decision: Option<ConflictPolicy>,
    cancel: Arc<AtomicBool>,
    /// Upload source, or download target (before conflict renaming).
    local: PathBuf,
    /// Remote modification time, applied to downloaded files.
    mtime: Option<i64>,
}

/// A target reserved by a running job, so two jobs never write the same file.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Reservation {
    Local(PathBuf),
    Remote(String),
}

#[derive(Default)]
struct Slot {
    busy: bool,
    /// The channel and the connection generation it belongs to.
    fs: Option<(u64, Arc<dyn RemoteFs>)>,
}

struct QueueState {
    jobs: Vec<Job>,
    concurrency: usize,
    running: usize,
    connection: Option<Arc<dyn Connection>>,
    generation: u64,
    slots: Vec<Slot>,
    batch_policies: HashMap<u64, ConflictPolicy>,
    next_batch: u64,
    dirty: HashSet<String>,
    reserved: HashMap<String, Reservation>,
    closed: bool,
    flusher_started: bool,
}

impl QueueState {
    fn job(&mut self, id: &str) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|j| j.transfer.id == id)
    }

    fn is_reserved(&self, reservation: &Reservation) -> bool {
        self.reserved.values().any(|r| r == reservation)
    }
}

/// How a job ended (or why it stopped).
enum Outcome {
    Done(u64),
    Skipped,
    /// Waiting for the user (`ask`).
    Conflict,
    Failed(AppError),
    Cancelled,
    /// No connection right now: back to the queue until a reconnect.
    Requeue,
}

/// What to do about the target of a job.
enum Target<T> {
    Write { target: T, existing: Option<u32> },
    Skip,
    Ask,
}

/// The queue of one session.
pub struct TransferQueue {
    session_id: String,
    events: EventSink,
    /// The session's connection log (for what the user should know about a transfer).
    logger: Logger,
    state: Mutex<QueueState>,
    signal: Arc<Notify>,
}

impl Drop for TransferQueue {
    fn drop(&mut self) {
        // Lets the flusher task notice the queue is gone and end.
        self.signal.notify_one();
    }
}

impl TransferQueue {
    pub fn new(session_id: String, events: EventSink, logger: Logger) -> Arc<Self> {
        Arc::new(Self {
            session_id,
            events,
            logger,
            state: Mutex::new(QueueState {
                jobs: Vec::new(),
                concurrency: usize::from(Settings::default().transfer_concurrency),
                running: 0,
                connection: None,
                generation: 0,
                slots: (0..usize::from(Settings::MAX_CONCURRENCY))
                    .map(|_| Slot::default())
                    .collect(),
                batch_policies: HashMap::new(),
                next_batch: 0,
                dirty: HashSet::new(),
                reserved: HashMap::new(),
                closed: false,
                flusher_started: false,
            }),
            signal: Arc::new(Notify::new()),
        })
    }

    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn contains(&self, id: &str) -> bool {
        self.lock().jobs.iter().any(|j| j.transfer.id == id)
    }

    /// Every job, in queue order.
    #[cfg(test)]
    pub fn snapshot(&self) -> Vec<Transfer> {
        self.lock()
            .jobs
            .iter()
            .map(|j| j.transfer.clone())
            .collect()
    }

    /// Hands the queue the session's connection (or `None` when it dropped). Channels of an
    /// older connection are discarded; queued jobs start when a connection is available.
    pub fn set_connection(self: &Arc<Self>, connection: Option<Arc<dyn Connection>>) {
        let available = connection.is_some();
        {
            let mut state = self.lock();
            state.connection = connection;
            state.generation += 1;
            for slot in &mut state.slots {
                slot.fs = None;
            }
        }
        if available {
            self.pump();
        }
    }

    /// Cancels everything and closes the worker channels (the tab is closing).
    pub async fn shutdown(&self) {
        let channels: Vec<Arc<dyn RemoteFs>> = {
            let mut state = self.lock();
            state.closed = true;
            state.connection = None;
            let now = now_ms();
            let mut changed = Vec::new();
            for job in &mut state.jobs {
                job.cancel.store(true, Ordering::SeqCst);
                if matches!(
                    job.transfer.state,
                    TransferState::Queued | TransferState::Conflict
                ) {
                    job.transfer.state = TransferState::Cancelled;
                    job.transfer.finished_at = Some(now);
                    changed.push(job.transfer.id.clone());
                }
            }
            state.dirty.extend(changed);
            state
                .slots
                .iter_mut()
                .filter_map(|s| s.fs.take().map(|(_, fs)| fs))
                .collect()
        };
        for fs in channels {
            fs.close().await;
        }
        self.flush();
    }

    // ----- Queueing ------------------------------------------------------------------------

    /// Queues downloads of remote files and folders into `folder`.
    pub async fn enqueue_downloads(
        self: &Arc<Self>,
        fs: &dyn RemoteFs,
        paths: &[String],
        folder: &Path,
        settings: &Settings,
    ) -> AppResult<Vec<Transfer>> {
        let mut plan = Vec::new();
        for path in paths {
            let path = normalize("/", path);
            let entry = fs.stat(&path).await?;
            let local = folder.join(local::sanitize_name(file_name(&path)));
            match entry.kind {
                _ if entry.is_dir_like() => {
                    expand_remote_dir(fs, &path, local, &mut plan).await?;
                }
                EntryKind::File => plan.push(Planned {
                    direction: TransferDirection::Download,
                    remote: path,
                    local,
                    size: entry.size,
                    mtime: entry.mtime,
                }),
                EntryKind::Dir | EntryKind::Symlink | EntryKind::Other => {
                    return Err(AppError::validation(format!(
                        "{} cannot be downloaded",
                        file_name(&path)
                    )));
                }
            }
        }
        Ok(self.add(plan, settings))
    }

    /// Queues uploads of local files and folders into `remote_dir`.
    pub async fn enqueue_uploads(
        self: &Arc<Self>,
        fs: &dyn RemoteFs,
        paths: Vec<PathBuf>,
        remote_dir: &str,
        settings: &Settings,
    ) -> AppResult<Vec<Transfer>> {
        let remote_dir = normalize("/", remote_dir);
        let items = blocking(move || local::walk(&paths)).await?;
        let mut plan = Vec::new();
        for item in items {
            let remote = item
                .relative
                .iter()
                .fold(remote_dir.clone(), |dir, name| join(&dir, name));
            if item.is_dir {
                ensure_remote_dir(fs, &remote).await?;
            } else {
                plan.push(Planned {
                    direction: TransferDirection::Upload,
                    remote,
                    local: item.path,
                    size: item.size,
                    mtime: None,
                });
            }
        }
        Ok(self.add(plan, settings))
    }

    fn add(self: &Arc<Self>, plan: Vec<Planned>, settings: &Settings) -> Vec<Transfer> {
        let added: Vec<Transfer> = {
            let mut state = self.lock();
            state.concurrency = usize::from(
                settings
                    .transfer_concurrency
                    .clamp(1, Settings::MAX_CONCURRENCY),
            );
            let batch = state.next_batch;
            state.next_batch += 1;
            let mut added = Vec::with_capacity(plan.len());
            for item in plan {
                let transfer = Transfer {
                    id: uuid::Uuid::now_v7().to_string(),
                    session_id: self.session_id.clone(),
                    direction: item.direction,
                    name: file_name(&item.remote).to_owned(),
                    local_path: item.local.display().to_string(),
                    remote_path: item.remote,
                    size: item.size,
                    done: 0,
                    state: TransferState::Queued,
                    error: None,
                    started_at: None,
                    finished_at: None,
                };
                added.push(transfer.clone());
                state.jobs.push(Job {
                    transfer,
                    batch,
                    policy: settings.conflict_policy,
                    decision: None,
                    cancel: Arc::new(AtomicBool::new(false)),
                    local: item.local,
                    mtime: item.mtime,
                });
            }
            added
        };
        self.pump();
        added
    }

    // ----- User actions --------------------------------------------------------------------

    /// Cancels a job. Queued and waiting jobs stop at once; running ones at the next chunk
    /// (their partial file is removed). Finished jobs are left alone.
    pub fn cancel(self: &Arc<Self>, id: &str) -> AppResult<()> {
        {
            let mut state = self.lock();
            let job = state
                .job(id)
                .ok_or_else(|| AppError::not_found("transfer"))?;
            job.cancel.store(true, Ordering::SeqCst);
            if matches!(
                job.transfer.state,
                TransferState::Queued | TransferState::Conflict
            ) {
                job.transfer.state = TransferState::Cancelled;
                job.transfer.finished_at = Some(now_ms());
                state.dirty.insert(id.to_owned());
            }
        }
        self.signal_flush();
        Ok(())
    }

    /// Queues a failed or cancelled job again, from the start.
    pub fn retry(self: &Arc<Self>, id: &str) -> AppResult<()> {
        {
            let mut state = self.lock();
            let job = state
                .job(id)
                .ok_or_else(|| AppError::not_found("transfer"))?;
            if !matches!(
                job.transfer.state,
                TransferState::Failed | TransferState::Cancelled
            ) {
                return Err(AppError::validation(
                    "only failed or cancelled transfers can be retried",
                ));
            }
            job.cancel = Arc::new(AtomicBool::new(false));
            job.decision = None;
            job.transfer.state = TransferState::Queued;
            job.transfer.done = 0;
            job.transfer.error = None;
            job.transfer.started_at = None;
            job.transfer.finished_at = None;
            state.dirty.insert(id.to_owned());
        }
        self.pump();
        Ok(())
    }

    /// Forgets finished jobs.
    pub fn clear_finished(&self) {
        self.lock().jobs.retain(|j| !j.transfer.state.is_finished());
    }

    /// Answers a job waiting in `conflict`; `apply_to_all` answers the rest of its batch too.
    pub fn resolve(
        self: &Arc<Self>,
        id: &str,
        decision: ConflictPolicy,
        apply_to_all: bool,
    ) -> AppResult<()> {
        if decision == ConflictPolicy::Ask {
            return Err(AppError::validation("choose overwrite, skip or rename"));
        }
        {
            let mut state = self.lock();
            let job = state
                .job(id)
                .ok_or_else(|| AppError::not_found("transfer"))?;
            if job.transfer.state != TransferState::Conflict {
                return Err(AppError::validation(
                    "this transfer is not waiting for a decision",
                ));
            }
            let batch = job.batch;
            let answer = |job: &mut Job| {
                job.decision = Some(decision);
                job.transfer.state = TransferState::Queued;
                job.transfer.id.clone()
            };
            let mut changed = vec![answer(job)];
            if apply_to_all {
                state.batch_policies.insert(batch, decision);
                for job in state
                    .jobs
                    .iter_mut()
                    .filter(|j| j.batch == batch && j.transfer.state == TransferState::Conflict)
                {
                    changed.push(answer(job));
                }
            }
            state.dirty.extend(changed);
        }
        self.pump();
        Ok(())
    }

    // ----- Scheduling ----------------------------------------------------------------------

    /// Starts queued jobs while there are free worker slots.
    fn pump(self: &Arc<Self>) {
        let mut starts = Vec::new();
        {
            let mut state = self.lock();
            if !state.closed && state.connection.is_some() {
                let now = now_ms();
                while state.running < state.concurrency {
                    let Some(index) = state
                        .jobs
                        .iter()
                        .position(|j| j.transfer.state == TransferState::Queued)
                    else {
                        break;
                    };
                    let Some(slot) = state.slots.iter().position(|s| !s.busy) else {
                        break;
                    };
                    state.slots[slot].busy = true;
                    state.running += 1;
                    let job = &mut state.jobs[index];
                    job.transfer.state = TransferState::Running;
                    job.transfer.started_at = Some(now);
                    let id = job.transfer.id.clone();
                    state.dirty.insert(id.clone());
                    starts.push((id, slot));
                }
            }
        }
        for (id, slot) in starts {
            let queue = Arc::clone(self);
            tokio::spawn(async move {
                let outcome = queue.execute(&id, slot).await;
                queue.finish(&id, slot, outcome);
                queue.pump();
            });
        }
        self.signal_flush();
    }

    /// The worker slot's channel, opened on first use (or after a connection change).
    async fn channel(&self, slot: usize) -> AppResult<Arc<dyn RemoteFs>> {
        let (generation, existing, connection) = {
            let state = self.lock();
            (
                state.generation,
                state.slots[slot].fs.clone(),
                state.connection.clone(),
            )
        };
        if let Some((owner, fs)) = existing
            && owner == generation
        {
            return Ok(fs);
        }
        let connection = connection.ok_or(AppError::Disconnected)?;
        let fs = connection.open_secondary().await?;
        let mut state = self.lock();
        if state.generation == generation {
            state.slots[slot].fs = Some((generation, Arc::clone(&fs)));
        }
        Ok(fs)
    }

    async fn execute(&self, id: &str, slot: usize) -> Outcome {
        let Some(job) = self.plan_of(id) else {
            return Outcome::Cancelled;
        };
        if job.cancel.load(Ordering::SeqCst) {
            return Outcome::Cancelled;
        }
        let fs = match self.channel(slot).await {
            Ok(fs) => fs,
            Err(AppError::Disconnected) => return Outcome::Requeue,
            Err(error) => return Outcome::Failed(error),
        };
        let progress = |done: u64| self.progress(id, done);
        let result = match job.direction {
            TransferDirection::Download => self.download(&*fs, &job, &progress).await,
            TransferDirection::Upload => self.upload(&*fs, &job, &progress).await,
        };
        match result {
            Ok(outcome) => outcome,
            Err(_) if job.cancel.load(Ordering::SeqCst) => Outcome::Cancelled,
            Err(AppError::Cancelled) => Outcome::Cancelled,
            Err(error) => Outcome::Failed(error),
        }
    }

    fn plan_of(&self, id: &str) -> Option<JobPlan> {
        let mut state = self.lock();
        let override_policy = {
            let job = state.job(id)?;
            job.batch
        };
        let batch_policy = state.batch_policies.get(&override_policy).copied();
        let job = state.job(id)?;
        Some(JobPlan {
            id: id.to_owned(),
            direction: job.transfer.direction,
            remote: job.transfer.remote_path.clone(),
            local: job.local.clone(),
            policy: job.decision.or(batch_policy).unwrap_or(job.policy),
            cancel: Arc::clone(&job.cancel),
            mtime: job.mtime,
        })
    }

    fn progress(&self, id: &str, done: u64) {
        {
            let mut state = self.lock();
            if let Some(job) = state.job(id) {
                job.transfer.done = done;
                state.dirty.insert(id.to_owned());
            }
        }
        self.signal.notify_one();
    }

    /// Reserves `reservation` for job `id`, unless another job holds it.
    fn reserve(&self, id: &str, reservation: Reservation) -> bool {
        let mut state = self.lock();
        if state.is_reserved(&reservation) {
            return false;
        }
        state.reserved.insert(id.to_owned(), reservation);
        true
    }

    fn reserved_by_other(&self, reservation: &Reservation) -> bool {
        self.lock().is_reserved(reservation)
    }

    /// Updates the job after conflict renaming.
    fn retarget(&self, id: &str, local: Option<&Path>, remote: Option<&str>) {
        let mut state = self.lock();
        if let Some(job) = state.job(id) {
            if let Some(local) = local {
                job.transfer.local_path = local.display().to_string();
                if job.transfer.direction == TransferDirection::Download {
                    job.transfer.name = local
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                }
            }
            if let Some(remote) = remote {
                job.transfer.remote_path = remote.to_owned();
                job.transfer.name = file_name(remote).to_owned();
            }
            state.dirty.insert(id.to_owned());
        }
    }

    fn finish(&self, id: &str, slot: usize, outcome: Outcome) {
        {
            let mut state = self.lock();
            state.slots[slot].busy = false;
            state.running = state.running.saturating_sub(1);
            state.reserved.remove(id);
            if let Outcome::Failed(AppError::Connection(_) | AppError::Disconnected) = &outcome {
                // Recreate this worker's channel for the next job.
                state.slots[slot].fs = None;
            }
            let now = now_ms();
            if let Some(job) = state.job(id) {
                let transfer = &mut job.transfer;
                match outcome {
                    Outcome::Done(bytes) => {
                        transfer.state = TransferState::Done;
                        transfer.done = bytes;
                        transfer.size = Some(transfer.size.unwrap_or(bytes).max(bytes));
                        transfer.finished_at = Some(now);
                    }
                    Outcome::Skipped => {
                        transfer.state = TransferState::Skipped;
                        transfer.finished_at = Some(now);
                    }
                    Outcome::Conflict => transfer.state = TransferState::Conflict,
                    Outcome::Failed(error) => {
                        transfer.state = TransferState::Failed;
                        transfer.error = Some(error.to_string());
                        transfer.finished_at = Some(now);
                    }
                    Outcome::Cancelled => {
                        transfer.state = TransferState::Cancelled;
                        transfer.finished_at = Some(now);
                    }
                    Outcome::Requeue => {
                        transfer.state = TransferState::Queued;
                        transfer.started_at = None;
                    }
                }
                state.dirty.insert(id.to_owned());
            }
        }
        self.signal.notify_one();
    }

    // ----- Events --------------------------------------------------------------------------

    fn signal_flush(self: &Arc<Self>) {
        let start = {
            let mut state = self.lock();
            !std::mem::replace(&mut state.flusher_started, true)
        };
        if start {
            let weak = Arc::downgrade(self);
            let signal = Arc::clone(&self.signal);
            tokio::spawn(flusher(weak, signal));
        }
        self.signal.notify_one();
    }

    /// Sends the jobs that changed since the last flush.
    fn flush(&self) {
        let changed: Vec<Transfer> = {
            let mut state = self.lock();
            if state.dirty.is_empty() {
                return;
            }
            let dirty = std::mem::take(&mut state.dirty);
            state
                .jobs
                .iter()
                .filter(|j| dirty.contains(&j.transfer.id))
                .map(|j| j.transfer.clone())
                .collect()
        };
        if !changed.is_empty() {
            (self.events)(SessionEvent::Transfers { transfers: changed });
        }
    }

    // ----- Downloads -----------------------------------------------------------------------

    async fn download(
        &self,
        fs: &dyn RemoteFs,
        job: &JobPlan,
        progress: &(dyn Fn(u64) + Send + Sync),
    ) -> AppResult<Outcome> {
        let target = match self.local_target(job).await? {
            Target::Write { target, .. } => target,
            Target::Skip => return Ok(Outcome::Skipped),
            Target::Ask => return Ok(Outcome::Conflict),
        };
        if target != job.local {
            self.retarget(&job.id, Some(&target), None);
        }
        if let Some(dir) = target.parent() {
            tokio::fs::create_dir_all(dir).await?;
        }
        let part = local::part_path(&target);
        let result = async {
            let mut file = tokio::fs::File::create(&part).await?;
            let bytes = fs
                .download(&job.remote, 0, &mut file, progress, &job.cancel)
                .await?;
            file.sync_all().await?;
            drop(file);
            if let Some(mtime) = job.mtime {
                set_mtime(&part, mtime).await;
            }
            tokio::fs::rename(&part, &target).await?;
            Ok::<u64, AppError>(bytes)
        }
        .await;
        match result {
            Ok(bytes) => Ok(Outcome::Done(bytes)),
            Err(error) => {
                if let Err(cleanup) = tokio::fs::remove_file(&part).await
                    && cleanup.kind() != std::io::ErrorKind::NotFound
                {
                    log::warn!("could not remove a partial download: {cleanup}");
                }
                Err(error)
            }
        }
    }

    /// Applies the conflict policy to a download target.
    async fn local_target(&self, job: &JobPlan) -> AppResult<Target<PathBuf>> {
        let wanted = job.local.clone();
        let exists = {
            let path = wanted.clone();
            blocking(move || Ok(path.symlink_metadata().is_ok())).await?
        };
        let busy = self.reserved_by_other(&Reservation::Local(wanted.clone()));
        if !exists && !busy {
            if self.reserve(&job.id, Reservation::Local(wanted.clone())) {
                return Ok(Target::Write {
                    target: wanted,
                    existing: None,
                });
            }
            return Err(AppError::Conflict(
                "another transfer is writing this file".into(),
            ));
        }
        match job.policy {
            ConflictPolicy::Ask => Ok(Target::Ask),
            ConflictPolicy::Skip => Ok(Target::Skip),
            ConflictPolicy::Overwrite => {
                if busy || !self.reserve(&job.id, Reservation::Local(wanted.clone())) {
                    return Err(AppError::Conflict(
                        "another transfer is writing this file".into(),
                    ));
                }
                if wanted.is_dir() {
                    return Err(AppError::Conflict(format!(
                        "a folder named {} is in the way",
                        wanted.display()
                    )));
                }
                Ok(Target::Write {
                    target: wanted,
                    existing: Some(0),
                })
            }
            ConflictPolicy::Rename => {
                let name = wanted
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                for n in 1..=MAX_RENAME {
                    let candidate = wanted.with_file_name(local::numbered_name(&name, n));
                    let taken = {
                        let path = candidate.clone();
                        blocking(move || Ok(path.symlink_metadata().is_ok())).await?
                    };
                    if !taken && self.reserve(&job.id, Reservation::Local(candidate.clone())) {
                        return Ok(Target::Write {
                            target: candidate,
                            existing: None,
                        });
                    }
                }
                Err(AppError::Conflict("no free name for the download".into()))
            }
        }
    }

    // ----- Uploads -------------------------------------------------------------------------

    async fn upload(
        &self,
        fs: &dyn RemoteFs,
        job: &JobPlan,
        progress: &(dyn Fn(u64) + Send + Sync),
    ) -> AppResult<Outcome> {
        let (target, existing_mode) = match self.remote_target(fs, job).await? {
            Target::Write { target, existing } => (target, existing),
            Target::Skip => return Ok(Outcome::Skipped),
            Target::Ask => return Ok(Outcome::Conflict),
        };
        let replacing = existing_mode.is_some();
        if target != job.remote {
            self.retarget(&job.id, None, Some(&target));
        }
        let temp = temp_sibling(&target);
        let mut source = tokio::fs::File::open(&job.local)
            .await
            .map_err(|e| AppError::Platform(format!("cannot read {}: {e}", job.local.display())))?;
        let uploaded = match fs.upload(&mut source, &temp, progress, &job.cancel).await {
            Ok(bytes) => bytes,
            Err(error) => {
                discard_remote(fs, &temp).await;
                return Err(error);
            }
        };
        if job.cancel.load(Ordering::SeqCst) {
            discard_remote(fs, &temp).await;
            return Err(AppError::Cancelled);
        }
        if let Some(mode) = existing_mode.filter(|m| *m != 0)
            && fs.capabilities().chmod
            && let Err(error) = fs.chmod(&temp, mode).await
        {
            log::warn!("could not keep the permissions of an uploaded file: {error}");
        }
        if replacing {
            match fs.replace(&temp, &target).await {
                Ok(_) => return Ok(Outcome::Done(uploaded)),
                Err(AppError::Unsupported(_)) => {
                    // Older backends: a short window without the target (ADR 0011).
                    self.logger.status(format!(
                        "{}: this server cannot replace a file in one step; removing the old \
                         one, then renaming the upload",
                        file_name(&target)
                    ));
                    if let Err(error) = fs.remove_file(&target).await {
                        discard_remote(fs, &temp).await;
                        return Err(error);
                    }
                }
                Err(error) => {
                    discard_remote(fs, &temp).await;
                    return Err(error);
                }
            }
        }
        if let Err(error) = fs.rename(&temp, &target).await {
            if replacing {
                // The old file is gone; the new one is complete under its temp name: keep it.
                return Err(AppError::Remote(format!(
                    "{error}; the uploaded file was kept as {temp}"
                )));
            }
            discard_remote(fs, &temp).await;
            return Err(error);
        }
        Ok(Outcome::Done(uploaded))
    }

    /// Applies the conflict policy to an upload target. `existing` carries the mode of the
    /// file being replaced (0 when unknown).
    async fn remote_target(&self, fs: &dyn RemoteFs, job: &JobPlan) -> AppResult<Target<String>> {
        let wanted = job.remote.clone();
        let existing = remote_entry(fs, &wanted).await?;
        let busy = self.reserved_by_other(&Reservation::Remote(wanted.clone()));
        if existing.is_none() && !busy {
            if self.reserve(&job.id, Reservation::Remote(wanted.clone())) {
                return Ok(Target::Write {
                    target: wanted,
                    existing: None,
                });
            }
            return Err(AppError::Conflict(
                "another transfer is writing this file".into(),
            ));
        }
        match job.policy {
            ConflictPolicy::Ask => Ok(Target::Ask),
            ConflictPolicy::Skip => Ok(Target::Skip),
            ConflictPolicy::Overwrite => {
                let Some(entry) = existing else {
                    return Err(AppError::Conflict(
                        "another transfer is writing this file".into(),
                    ));
                };
                if entry.kind == EntryKind::Dir {
                    return Err(AppError::Conflict(format!(
                        "a folder named {} is in the way",
                        entry.name
                    )));
                }
                if busy || !self.reserve(&job.id, Reservation::Remote(wanted.clone())) {
                    return Err(AppError::Conflict(
                        "another transfer is writing this file".into(),
                    ));
                }
                Ok(Target::Write {
                    target: wanted,
                    existing: Some(entry.mode.unwrap_or(0)),
                })
            }
            ConflictPolicy::Rename => {
                let dir = parent(&wanted);
                let name = file_name(&wanted).to_owned();
                for n in 1..=MAX_RENAME {
                    let candidate = join(&dir, &local::numbered_name(&name, n));
                    if remote_entry(fs, &candidate).await?.is_none()
                        && self.reserve(&job.id, Reservation::Remote(candidate.clone()))
                    {
                        return Ok(Target::Write {
                            target: candidate,
                            existing: None,
                        });
                    }
                }
                Err(AppError::Conflict("no free name for the upload".into()))
            }
        }
    }
}

/// What a job needs while it runs (copied out of the queue so no lock is held).
struct JobPlan {
    id: String,
    direction: TransferDirection,
    remote: String,
    local: PathBuf,
    policy: ConflictPolicy,
    cancel: Arc<AtomicBool>,
    mtime: Option<i64>,
}

/// One file to transfer, found while expanding the user's selection.
struct Planned {
    direction: TransferDirection,
    remote: String,
    local: PathBuf,
    size: Option<u64>,
    mtime: Option<i64>,
}

async fn flusher(queue: Weak<TransferQueue>, signal: Arc<Notify>) {
    loop {
        signal.notified().await;
        tokio::time::sleep(FLUSH_EVERY).await;
        let Some(queue) = queue.upgrade() else {
            break;
        };
        queue.flush();
    }
}

/// Lists a remote folder recursively, creating the local folders as it goes.
async fn expand_remote_dir(
    fs: &dyn RemoteFs,
    remote_root: &str,
    local_root: PathBuf,
    plan: &mut Vec<Planned>,
) -> AppResult<()> {
    let mut stack = vec![(remote_root.to_owned(), local_root)];
    while let Some((remote, local_dir)) = stack.pop() {
        ensure_local_dir(&local_dir).await?;
        let mut entries = fs.list(&remote).await?;
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        for entry in entries {
            let child_remote = join(&remote, &entry.name);
            let child_local = local_dir.join(local::sanitize_name(&entry.name));
            match entry.kind {
                EntryKind::Dir => stack.push((child_remote, child_local)),
                EntryKind::File => plan.push(Planned {
                    direction: TransferDirection::Download,
                    remote: child_remote,
                    local: child_local,
                    size: entry.size,
                    mtime: entry.mtime,
                }),
                // Links to files are downloaded as files; links to folders are not followed
                // (no loops), and devices or sockets are never downloaded.
                EntryKind::Symlink if entry.target_kind == Some(EntryKind::File) => {
                    plan.push(Planned {
                        direction: TransferDirection::Download,
                        remote: child_remote,
                        local: child_local,
                        size: None,
                        mtime: None,
                    });
                }
                EntryKind::Symlink | EntryKind::Other => {}
            }
        }
    }
    Ok(())
}

async fn ensure_local_dir(dir: &Path) -> AppResult<()> {
    let dir = dir.to_owned();
    blocking(move || {
        if dir.exists() && !dir.is_dir() {
            return Err(AppError::Conflict(format!(
                "{} exists and is not a folder",
                dir.display()
            )));
        }
        std::fs::create_dir_all(&dir)?;
        Ok(())
    })
    .await
}

/// Creates a remote folder for an upload; an existing folder is merged into.
async fn ensure_remote_dir(fs: &dyn RemoteFs, path: &str) -> AppResult<()> {
    match fs.mkdir(path).await {
        Ok(()) => Ok(()),
        Err(AppError::Conflict(_)) => match fs.stat(path).await?.kind {
            EntryKind::Dir => Ok(()),
            _ => Err(AppError::Conflict(format!(
                "{} exists and is not a folder",
                file_name(path)
            ))),
        },
        Err(error) => Err(error),
    }
}

/// The entry at `path`, or `None` when nothing is there.
async fn remote_entry(
    fs: &dyn RemoteFs,
    path: &str,
) -> AppResult<Option<crate::domain::entry::Entry>> {
    match fs.stat(path).await {
        Ok(entry) => Ok(Some(entry)),
        Err(AppError::NotFound { .. }) => Ok(None),
        Err(error) => Err(error),
    }
}

/// A hidden sibling for an upload in progress: `.<name>.gem-<random>`.
fn temp_sibling(target: &str) -> String {
    let random = uuid::Uuid::now_v7().simple().to_string();
    let suffix = &random[random.len() - 10..];
    join(
        &parent(target),
        &format!(".{}.gem-{suffix}", file_name(target)),
    )
}

/// Best-effort removal of a partial remote file.
async fn discard_remote(fs: &dyn RemoteFs, temp: &str) {
    if let Err(error) = fs.remove_file(temp).await
        && !matches!(error, AppError::NotFound { .. })
    {
        log::warn!("could not remove a partial upload: {error}");
    }
}

/// Gives a downloaded file the remote modification time (best effort).
async fn set_mtime(path: &Path, mtime: i64) {
    let Ok(secs) = u64::try_from(mtime) else {
        return;
    };
    let path = path.to_owned();
    let result = blocking(move || {
        let time = std::time::UNIX_EPOCH + Duration::from_secs(secs);
        std::fs::File::options()
            .write(true)
            .open(&path)?
            .set_modified(time)?;
        Ok(())
    })
    .await;
    if let Err(error) = result {
        log::warn!("could not set the time of a downloaded file: {error}");
    }
}

#[cfg(test)]
mod tests;
