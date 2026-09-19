//! TaskExecutor — async task queue + state machine + progress callbacks + retry.
//!
//! Rust port of RAGFlow's `rag/svr/task_executor.py`:
//!
//! | RAGFlow (Python)        | This module                          |
//! |-------------------------|--------------------------------------|
//! | `handle_task`           | [`TaskExecutor::spawn`] queue loop   |
//! | `task_manager`          | [`TaskExecutor::spawn`] + semaphore  |
//! | `do_handle_task`        | [`run_one`] state machine            |
//! | `set_progress`          | [`ProgressCallback::set_progress`]   |
//! | `report_status`         | [`TaskExecutor::status`]             |
//! | `catch`/retry semantics | retryable [`TaskError`] re-queue     |
//!
//! Task lifecycle mirrors RAGFlow's document status column:
//! `UNSTARTED → RUNNING → DONE | FAILED`, with retryable failures returning to
//! the queue (pending) up to `max_retries` before failing permanently.
//!
//! The executor is a self-contained engine: handlers are plain async closures
//! `(ExecTask, ProgressCallback) -> Result<usize, TaskError>`, so the queue,
//! state machine, progress formatting and retry policy are unit-testable with
//! mock chunk/embedding handlers (see `#[cfg(test)]`).

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tokio::sync::{Semaphore, mpsc};

/// RAGFlow-compatible document `run` status values.
pub mod doc_status {
    /// 0 — document uploaded but not yet processed.
    pub const UNSTARTED: &str = "UNSTARTED";
    /// 1 — a worker is parsing / chunking / embedding.
    pub const RUNNING: &str = "RUNNING";
    /// 3 — indexing finished successfully.
    pub const DONE: &str = "DONE";
    /// 2 — processing failed after retries were exhausted.
    pub const FAILED: &str = "FAILED";
}

/// Task execution state machine.
///
/// Mirrors RAGFlow's task lifecycle: a task is created `Pending`, flips to
/// `Processing` while a worker runs it, and lands on `Done` or `Failed`.
/// Retryable failures (the RAGFlow `catch` path) return to `Pending` and are
/// re-queued until `max_retries` is exhausted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum TaskState {
    Pending,
    Processing,
    Done,
    Failed,
}

impl TaskState {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskState::Pending => "pending",
            TaskState::Processing => "processing",
            TaskState::Done => "done",
            TaskState::Failed => "failed",
        }
    }

    /// Map to the RAGFlow document `run` status column.
    pub fn doc_run(self) -> &'static str {
        match self {
            TaskState::Pending => doc_status::UNSTARTED,
            TaskState::Processing => doc_status::RUNNING,
            TaskState::Done => doc_status::DONE,
            TaskState::Failed => doc_status::FAILED,
        }
    }
}

/// Error raised by a task handler.
///
/// `retryable` mirrors the RAGFlow `handle_task` catch semantics: a retryable
/// error re-enters the queue (with the `[ERROR]` progress message recorded),
/// while a permanent error fails the task immediately.
#[derive(Debug, Clone)]
pub struct TaskError {
    retryable: bool,
    message: String,
}

impl TaskError {
    pub fn retryable(message: impl Into<String>) -> Self {
        Self {
            retryable: true,
            message: message.into(),
        }
    }

    pub fn permanent(message: impl Into<String>) -> Self {
        Self {
            retryable: false,
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn is_retryable(&self) -> bool {
        self.retryable
    }
}

impl std::fmt::Display for TaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for TaskError {}

/// A task queued on the executor.
///
/// Field semantics are aligned with RAGFlow's task record: `from_page` /
/// `to_page` drive the `Page(a~b)` progress prefix (`to_page == -1` means the
/// whole document), `attempts` counts executions, `max_retries` bounds retries.
#[derive(Debug, Clone, Serialize)]
pub struct ExecTask {
    pub id: String,
    pub doc_id: String,
    pub kb_id: String,
    pub name: String,
    /// 0 = low, 1 = high (RAGFlow queue priority).
    pub priority: i32,
    /// First page to parse, 0-based.
    pub from_page: usize,
    /// Last page to parse; -1 means the whole document.
    pub to_page: i64,
    /// Number of executions so far (0 = first attempt).
    pub attempts: usize,
    /// Maximum executions before a retryable task fails permanently.
    pub max_retries: usize,
    pub state: TaskState,
    pub progress: f32,
    pub message: String,
    pub created_at: u64,
}

impl ExecTask {
    pub fn new(
        id: impl Into<String>,
        doc_id: impl Into<String>,
        kb_id: impl Into<String>,
        name: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            doc_id: doc_id.into(),
            kb_id: kb_id.into(),
            name: name.into(),
            priority: 0,
            from_page: 0,
            to_page: -1,
            attempts: 0,
            max_retries: 2,
            state: TaskState::Pending,
            progress: 0.0,
            message: "Queued".into(),
            created_at: now_ms(),
        }
    }
}

/// Handler executed for each task: `(task, progress) -> chunk count`.
pub type TaskHandler = Arc<
    dyn Fn(
            ExecTask,
            ProgressCallback,
        ) -> Pin<Box<dyn Future<Output = Result<usize, TaskError>> + Send>>
        + Send
        + Sync,
>;

/// Progress callback with RAGFlow `set_progress` semantics:
/// - `prog < 0` prefixes the message with `[ERROR]`;
/// - when `from_page < to_page` the message is prefixed with `Page(a~b): `;
/// - non-empty messages are prefixed with a `HH:MM:SS ` timestamp.
#[derive(Clone)]
pub struct ProgressCallback {
    inner: Arc<dyn Fn(f32, String) + Send + Sync>,
}

impl ProgressCallback {
    pub fn new(handler: impl Fn(f32, String) + Send + Sync + 'static) -> Self {
        Self {
            inner: Arc::new(handler),
        }
    }

    pub fn noop() -> Self {
        Self::new(|_, _| {})
    }

    /// Format a progress report exactly like RAGFlow's `set_progress`:
    /// error prefix, page range, timestamp.
    pub fn format_progress_message(task: &ExecTask, prog: f32, msg: &str) -> (f32, String) {
        let mut message = msg.to_string();
        if prog < 0.0 {
            message = format!("[ERROR]{message}");
        }
        if task.to_page > 0 && task.from_page < task.to_page as usize
            && !message.is_empty() {
                message = format!(
                    "Page({}~{}): {message}",
                    task.from_page + 1,
                    task.to_page + 1
                );
            }
        if !message.is_empty() {
            message = format!("{} {message}", hhmmss());
        }
        (prog, message)
    }

    /// Report progress for `task`, mirroring `set_progress(task_id, from_page,
    /// to_page, prog, msg)`.
    pub fn set_progress(&self, task: &ExecTask, prog: f32, msg: &str) {
        let (prog, message) = Self::format_progress_message(task, prog, msg);
        (self.inner)(prog, message);
    }
}

/// Snapshot of executor counters (the `report_status` heartbeat fields).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutorStats {
    pub pending: u64,
    pub running: u64,
    pub done: u64,
    pub failed: u64,
}

/// `report_status`-style heartbeat record for the executor.
#[derive(Debug, Clone, Serialize)]
pub struct ExecutorStatus {
    pub name: String,
    pub boot_at: u64,
    pub now: u64,
    pub pending: u64,
    pub running: u64,
    pub done: u64,
    pub failed: u64,
    /// Tasks currently claimed by a worker (`CURRENT_TASKS` equivalent).
    pub current: Vec<ExecTask>,
}

/// Async task executor: an unbounded queue (`tokio::mpsc`) drained by a
/// management loop that dispatches each task to a worker with bounded
/// concurrency (`task_manager` + semaphore in RAGFlow).
pub struct TaskExecutor {
    name: String,
    boot_at: u64,
    tx: Mutex<Option<mpsc::UnboundedSender<ExecTask>>>,
    rx: Mutex<Option<mpsc::UnboundedReceiver<ExecTask>>>,
    semaphore: Arc<Semaphore>,
    max_concurrent: usize,
    pending: AtomicU64,
    running: AtomicU64,
    done: AtomicU64,
    failed: AtomicU64,
    current: Mutex<HashMap<String, ExecTask>>,
}

impl TaskExecutor {
    /// Create an executor with the given worker concurrency. The management
    /// loop is not started until [`TaskExecutor::spawn`] is called.
    pub fn new(name: &str, max_concurrent: usize) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self {
            name: name.to_string(),
            boot_at: now_ms(),
            tx: Mutex::new(Some(tx)),
            rx: Mutex::new(Some(rx)),
            semaphore: Arc::new(Semaphore::new(max_concurrent.max(1))),
            max_concurrent: max_concurrent.max(1),
            pending: AtomicU64::new(0),
            running: AtomicU64::new(0),
            done: AtomicU64::new(0),
            failed: AtomicU64::new(0),
            current: Mutex::new(HashMap::new()),
        }
    }

    /// Enqueue a task. Returns `Err` if the executor has been shut down.
    pub fn submit(&self, task: ExecTask) -> Result<(), TaskError> {
        let sender = self
            .tx
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| TaskError::permanent("Task executor has been shut down"))?
            .clone();
        sender
            .send(task)
            .map_err(|_| TaskError::permanent("Task executor has been shut down"))?;
        self.pending.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn stats(&self) -> ExecutorStats {
        ExecutorStats {
            pending: self.pending.load(Ordering::Relaxed),
            running: self.running.load(Ordering::Relaxed),
            done: self.done.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
        }
    }

    /// `report_status`-style heartbeat.
    pub fn status(&self) -> ExecutorStatus {
        ExecutorStatus {
            name: self.name.clone(),
            boot_at: self.boot_at,
            now: now_ms(),
            pending: self.pending.load(Ordering::Relaxed),
            running: self.running.load(Ordering::Relaxed),
            done: self.done.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            current: self.current.lock().unwrap().values().cloned().collect(),
        }
    }

    pub fn boot_at(&self) -> u64 {
        self.boot_at
    }

    pub fn max_concurrent(&self) -> usize {
        self.max_concurrent
    }

    /// Start the management loop (`handle_task`/`task_manager`): drain the
    /// queue, acquire a concurrency permit per task and run the handler in a
    /// spawned worker. Returns the loop's join handle.
    pub fn spawn(
        self: &Arc<Self>,
        handler: TaskHandler,
        retry_backoff: Duration,
    ) -> tokio::task::JoinHandle<()> {
        let Some(rx) = self.rx.lock().unwrap().take() else {
            tracing::warn!(executor = %self.name, "TaskExecutor::spawn called twice; loop already running");
            return tokio::spawn(async {});
        };
        let this = self.clone();
        tokio::spawn(async move {
            this.management_loop(rx, handler, retry_backoff).await;
        })
    }

    /// Shut the executor down: drop the queue sender (the management loop
    /// exits once queued tasks are drained) and wait for the loop. In-flight
    /// worker tasks are allowed to finish.
    pub async fn shutdown(self: &Arc<Self>, handle: tokio::task::JoinHandle<()>) {
        self.tx.lock().unwrap().take();
        let _ = handle.await;
    }

    /// Look up a task's live state (used by tests and status tooling).
    pub fn current(&self, id: &str) -> Option<ExecTask> {
        self.current.lock().unwrap().get(id).cloned()
    }

    async fn management_loop(
        self: Arc<Self>,
        mut rx: mpsc::UnboundedReceiver<ExecTask>,
        handler: TaskHandler,
        retry_backoff: Duration,
    ) {
        while let Some(task) = rx.recv().await {
            let permit = match self.semaphore.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => break,
            };
            let this = self.clone();
            let handler = handler.clone();
            tokio::spawn(async move {
                this.run_one(task, handler, retry_backoff, permit).await;
            });
        }
        tracing::info!(executor = %self.name, "Task executor management loop exited");
    }

    /// One task execution: pending → processing → done | failed, with the
    /// RAGFlow catch semantics (retryable errors re-enter the queue).
    async fn run_one(
        self: Arc<Self>,
        task: ExecTask,
        handler: TaskHandler,
        retry_backoff: Duration,
        _permit: tokio::sync::OwnedSemaphorePermit,
    ) {
        self.pending.fetch_sub(1, Ordering::Relaxed);
        self.running.fetch_add(1, Ordering::Relaxed);
        let progress = {
            let this = self.clone();
            let task_id = task.id.clone();
            ProgressCallback::new(move |prog, message| {
                if let Some(current) = this.current.lock().unwrap().get_mut(&task_id) {
                    current.progress = prog;
                    current.message = message;
                }
            })
        };
        self.current.lock().unwrap().insert(
            task.id.clone(),
            task.clone_with_state(TaskState::Processing),
        );

        let result = handler(task.clone(), progress).await;
        let retry = match result {
            Ok(chunk_count) => {
                let mut done = task.clone();
                done.state = TaskState::Done;
                done.progress = 1.0;
                done.message = format!("Indexed {chunk_count} chunks");
                self.current.lock().unwrap().insert(done.id.clone(), done);
                self.done.fetch_add(1, Ordering::Relaxed);
                None
            }
            Err(error) if error.is_retryable() && task.attempts < task.max_retries => {
                let mut retry = task.clone();
                retry.attempts += 1;
                retry.state = TaskState::Pending;
                retry.progress = 0.0;
                retry.message = format!("Attempt {} failed: {}; retrying", retry.attempts, error);
                self.current
                    .lock()
                    .unwrap()
                    .insert(retry.id.clone(), retry.clone());
                Some(retry)
            }
            Err(error) => {
                let mut failed = task.clone();
                failed.state = TaskState::Failed;
                failed.progress = 1.0;
                failed.message = error.to_string();
                self.current
                    .lock()
                    .unwrap()
                    .insert(failed.id.clone(), failed);
                self.failed.fetch_add(1, Ordering::Relaxed);
                tracing::error!(task_id = %task.id, %error, "Task failed");
                None
            }
        };
        self.running.fetch_sub(1, Ordering::Relaxed);
        // Release the concurrency permit before the retry backoff so a waiting
        // task can start immediately.
        drop(_permit);
        if let Some(retry) = retry {
            tokio::time::sleep(retry_backoff * retry.attempts as u32).await;
            let _ = self.submit(retry);
        }
    }
}

impl ExecTask {
    fn clone_with_state(&self, state: TaskState) -> ExecTask {
        let mut task = self.clone();
        task.state = state;
        task
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// `HH:MM:SS` local time, matching RAGFlow's `datetime.now().strftime("%H:%M:%S")`.
fn hhmmss() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

// ============================================================================
// Sync tasks — RAGFlow `rag/svr/sync_data_source.py` port (SyncBase).
//
// | RAGFlow (Python)            | This module                          |
// |-----------------------------|--------------------------------------|
// | `SyncBase.__call__`         | [`execute_sync`] (timeout + outcome) |
// | `SyncBase._run_task_logic`  | [`run_sync_task`]                    |
// | `SyncBase.window_info`      | [`sync_window_info`]                 |
// | `SyncBase._format_window_boundary` | [`format_window_boundary`]     |
// | `_BlobLikeBase` fingerprint | fingerprint bypass in [`run_sync_task`] |
// | `SyncLogsService.schedule`  | caller advances `poll_range_start`   |
//
// A sync task polls an external data source (S3/GitLab/Notion/...), upserts
// changed documents into a KB and reconciles deletions against the remote
// snapshot. The state machine mirrors `SyncBase`: run under a timeout, mark
// FAIL on timeout/exception, otherwise report the summary and advance the
// poll window (`poll_range_start` = latest `doc_updated_at` seen).
// ============================================================================

/// A data-source synchronization task (`SyncBase` task dict).
#[derive(Debug, Clone, Serialize)]
pub struct SyncTask {
    pub id: String,
    pub connector_id: String,
    pub kb_id: String,
    pub tenant_id: String,
    /// Source label used in sync windows/logs (`SyncBase.SOURCE_NAME`).
    pub source_name: String,
    /// Inclusive start of the previous sync window (RFC3339); `None` = full
    /// history. Advanced by the caller to `next_poll_range_start` after each
    /// successful run (`SyncLogsService.schedule`).
    pub poll_range_start: Option<String>,
    /// `__call__` timeout in seconds (`asyncio.wait_for`).
    pub timeout_secs: u64,
    /// `duplicate_and_parse(auto_parse)` flag.
    pub auto_parse: bool,
    /// `task["reindex"] != "1"` guard: a reindex re-pulls everything.
    pub reindex: bool,
    /// `conf["sync_deleted_files"]`: only then is stale-doc reconciliation
    /// performed against the remote snapshot.
    pub sync_deleted_files: bool,
}

impl SyncTask {
    pub fn new(
        id: impl Into<String>,
        connector_id: impl Into<String>,
        kb_id: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            connector_id: connector_id.into(),
            kb_id: kb_id.into(),
            tenant_id: String::new(),
            source_name: String::new(),
            poll_range_start: None,
            timeout_secs: 3600,
            auto_parse: true,
            reindex: false,
            sync_deleted_files: false,
        }
    }
}

/// A connector-produced document ready for KB upsert (mirrors the dict built
/// in `SyncBase._run_task_logic` — `id` is already `hash128(connector_id:doc_id)`).
#[derive(Debug, Clone)]
pub struct SyncDoc {
    pub id: String,
    pub semantic_identifier: String,
    pub extension: String,
    pub size_bytes: usize,
    /// RFC3339 timestamp of the last remote change (strings compare
    /// lexicographically == chronologically for RFC3339).
    pub doc_updated_at: String,
    /// Text payload (RAGFlow `blob`).
    pub blob: String,
    /// Optional content hash used by the `_BlobLikeBase` fingerprint bypass.
    pub fingerprint: Option<String>,
    pub metadata: std::collections::HashMap<String, String>,
}

/// Output of a connector's `_generate`: batched documents plus an optional
/// full remote snapshot for stale-doc reconciliation.
#[derive(Debug, Clone, Default)]
pub struct SyncFeed {
    pub batches: Vec<Vec<SyncDoc>>,
    /// Full remote doc-id snapshot; when present (and deletion sync is
    /// enabled) KB documents missing from it are counted as removed.
    pub file_list: Option<Vec<String>>,
}

/// Result of upserting one document into the KB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncChange {
    /// Document id was not previously in the KB (`added_docs`).
    Added,
    /// Document id already existed (`updated_docs`).
    Updated,
}

/// Per-run counters, mirroring the `sync summary` log line of
/// `SyncBase._run_task_logic`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SyncSummary {
    /// `added + updated + deleted` (RAGFlow `total_changed_docs`).
    pub total: usize,
    pub added: usize,
    pub updated: usize,
    /// Documents reconciled as deleted against the remote snapshot.
    pub deleted: usize,
    /// Documents bypassed by the fingerprint filter (unchanged blobs).
    pub skipped: usize,
    /// Documents whose upsert failed (batch exceptions in RAGFlow).
    pub failed: usize,
    /// New `poll_range_start` for the next scheduled run.
    pub next_poll_range_start: Option<String>,
}

/// Terminal outcome of [`execute_sync`] (`SyncBase.__call__` branches).
#[derive(Debug, Clone)]
pub enum SyncOutcome {
    /// Run finished; summary carries the next poll window.
    Success(SyncSummary),
    /// `asyncio.wait_for` fired → RAGFlow marks the sync log FAIL.
    Timeout(String),
    /// Top-level exception → RAGFlow marks the sync log FAIL.
    Failed(String),
}

/// Data source connector implementing the `_generate` side of a sync task.
pub trait SyncConnector: Send + Sync {
    fn source_name(&self) -> &str;
    /// Produce the batched document feed (and optional remote snapshot).
    /// Async so a slow/polling connector can be cancelled by `execute_sync`'s
    /// timeout, exactly like `asyncio.wait_for(self._run_task_logic(...))`.
    fn generate(
        &self,
        task: &SyncTask,
    ) -> Pin<Box<dyn Future<Output = Result<SyncFeed, String>> + Send + '_>>;
}

/// Parse an RFC3339 or `"%Y-%m-%d %H:%M:%S"` timestamp for display.
fn parse_datetime(raw: &str) -> Option<chrono::DateTime<chrono::FixedOffset>> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(dt);
    }
    if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(raw, "%Y-%m-%d %H:%M:%S") {
        use chrono::TimeZone;
        return chrono::Local
            .from_local_datetime(&naive)
            .single()
            .map(|dt| dt.fixed_offset());
    }
    None
}

/// `SyncBase._format_window_boundary`: `None` → "beginning", otherwise
/// `"%Y-%m-%d %H:%M:%S %Z"` in local time; unparseable values pass through.
pub fn format_window_boundary(value: Option<&str>) -> String {
    match value {
        None => "beginning".to_string(),
        Some(raw) => parse_datetime(raw)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S %Z").to_string())
            .unwrap_or_else(|| raw.to_string()),
    }
}

/// `SyncBase.window_info`: `"sync window: <start> -> <end>"`. A reindex or a
/// missing `poll_range_start` means the window starts at "beginning".
pub fn sync_window_info(task: &SyncTask) -> String {
    let start = if task.reindex {
        None
    } else {
        task.poll_range_start.as_deref()
    };
    let end = chrono::Utc::now().to_rfc3339();
    format!(
        "sync window: {} -> {}",
        format_window_boundary(start),
        format_window_boundary(Some(&end))
    )
}

/// `SyncBase._run_task_logic`: iterate the connector feed, bypass unchanged
/// blobs via fingerprints, upsert changed documents, then reconcile stale
/// documents against the remote snapshot. `upsert` is the caller's
/// `duplicate_and_parse` equivalent; it receives the change classification
/// (Added vs Updated, computed against `existing_doc_ids`, which is extended
/// as the run progresses like RAGFlow's `existing_doc_ids.update(...)`).
pub async fn run_sync_task(
    task: &SyncTask,
    connector: &dyn SyncConnector,
    existing_doc_ids: &std::collections::HashSet<String>,
    existing_fingerprints: &std::collections::HashMap<String, String>,
    mut upsert: impl FnMut(&SyncDoc, SyncChange) -> Result<(), String>,
) -> Result<SyncSummary, String> {
    let feed = connector.generate(task).await?;
    let mut summary = SyncSummary::default();
    let mut known: std::collections::HashSet<String> = existing_doc_ids.clone();
    // RAGFlow starts from `task["poll_range_start"]` (epoch 1970 when unset).
    let mut next_update: Option<String> = task.poll_range_start.clone();

    for batch in &feed.batches {
        if batch.is_empty() {
            continue;
        }
        for doc in batch {
            // `_BlobLikeBase._fingerprint_filtered_generator`: skip objects
            // whose listing fingerprint matches the persisted content hash.
            if let Some(fp) = &doc.fingerprint
                && existing_fingerprints
                    .get(&doc.id)
                    .is_some_and(|stored| stored == fp)
                {
                    summary.skipped += 1;
                    continue;
                }
            if let Some(ts) = next_update.as_ref() {
                if doc.doc_updated_at > *ts {
                    next_update = Some(doc.doc_updated_at.clone());
                }
            } else {
                next_update = Some(doc.doc_updated_at.clone());
            }
            let change = if known.contains(&doc.id) {
                SyncChange::Updated
            } else {
                SyncChange::Added
            };
            match upsert(doc, change) {
                Ok(()) => {
                    match change {
                        SyncChange::Added => summary.added += 1,
                        SyncChange::Updated => summary.updated += 1,
                    }
                    known.insert(doc.id.clone());
                }
                Err(_) => summary.failed += 1,
            }
        }
    }

    // Stale-document reconciliation (`ConnectorService.cleanup_stale_documents_for_task`):
    // only on incremental runs with the snapshot flag enabled.
    let expects_deleted_file_snapshot =
        !task.reindex && task.poll_range_start.is_some() && task.sync_deleted_files;
    if expects_deleted_file_snapshot
        && let Some(file_list) = &feed.file_list {
            let snapshot: std::collections::HashSet<String> = file_list.iter().cloned().collect();
            summary.deleted = known.iter().filter(|id| !snapshot.contains(*id)).count();
        }

    summary.total = summary.added + summary.updated + summary.deleted;
    summary.next_poll_range_start = next_update;
    Ok(summary)
}

/// `SyncBase.__call__`: run the sync logic under `timeout_secs` and classify
/// the outcome — Timeout / Failed are permanent failures in RAGFlow (the sync
/// log status is set to FAIL), Success carries the next poll window.
pub async fn execute_sync(
    task: &SyncTask,
    connector: &dyn SyncConnector,
    existing_doc_ids: &std::collections::HashSet<String>,
    existing_fingerprints: &std::collections::HashMap<String, String>,
    upsert: impl FnMut(&SyncDoc, SyncChange) -> Result<(), String>,
) -> SyncOutcome {
    let timeout = Duration::from_secs(task.timeout_secs.max(1));
    match tokio::time::timeout(
        timeout,
        run_sync_task(
            task,
            connector,
            existing_doc_ids,
            existing_fingerprints,
            upsert,
        ),
    )
    .await
    {
        Ok(Ok(summary)) => SyncOutcome::Success(summary),
        Ok(Err(message)) => SyncOutcome::Failed(message),
        Err(_) => SyncOutcome::Timeout(format!("Task timeout after {} seconds", task.timeout_secs)),
    }
}

/// Adapt a sync run to the executor's [`TaskHandler`] contract so sync tasks
/// flow through the same queue/state-machine/retry machinery as document
/// tasks. `sink` is the caller's `duplicate_and_parse` equivalent.
pub fn sync_task_handler<C>(
    sync_task: SyncTask,
    connector: Arc<C>,
    existing_doc_ids: std::collections::HashSet<String>,
    existing_fingerprints: std::collections::HashMap<String, String>,
    sink: Arc<dyn Fn(&SyncDoc, SyncChange) -> Result<(), String> + Send + Sync>,
) -> TaskHandler
where
    C: SyncConnector + 'static,
{
    Arc::new(move |task, progress| {
        let sync_task = sync_task.clone();
        let connector = connector.clone();
        let existing_doc_ids = existing_doc_ids.clone();
        let existing_fingerprints = existing_fingerprints.clone();
        let sink = sink.clone();
        Box::pin(async move {
            let outcome = execute_sync(
                &sync_task,
                connector.as_ref(),
                &existing_doc_ids,
                &existing_fingerprints,
                move |doc, change| sink(doc, change),
            )
            .await;
            match outcome {
                SyncOutcome::Success(summary) => {
                    progress.set_progress(
                        &task,
                        1.0,
                        &format!(
                            "sync summary: total={}, added={}, updated={}, deleted={}",
                            summary.total, summary.added, summary.updated, summary.deleted
                        ),
                    );
                    Ok(summary.total)
                }
                SyncOutcome::Timeout(message) | SyncOutcome::Failed(message) => {
                    progress.set_progress(&task, -1.0, &message);
                    Err(TaskError::permanent(message))
                }
            }
        })
    })
}

// ============================================================================
// build_chunks enrichment — RAGFlow `task_executor.build_chunks` tail phases.
//
// After chunking (and before embedding) RAGFlow optionally runs, per chunk:
//   - `auto_keywords`  → LLM keyword extraction, cached by
//     `get_llm_cache`/`set_llm_cache`, stored as `important_kwd` (+ `important_tks`);
//   - `auto_questions` → LLM question proposal, same cache, stored as
//     `question_kwd` (newline-joined) (+ `question_tks`).
//
// This port provides both phases with the same cache semantics. When no LLM
// hook is configured (`ExtractionFn`), the offline statistical extractor
// (`crate::extractor`, the existing `auto_keywords` backend) and a
// deterministic topic-anchored question generator are used, so the phase
// remains functional without external services.
// ============================================================================

/// One `get_llm_cache`/`set_llm_cache` entry (TTL bookkeeping included).
#[derive(Debug, Clone)]
pub struct LlmCacheEntry {
    pub value: String,
    pub expires_at: u64,
}

/// In-memory LLM cache with RAGFlow `get_llm_cache`/`set_llm_cache` semantics:
/// key = `(model, content-hash, kind, params)`; entries expire after `ttl`.
pub struct LlmCache {
    entries: Mutex<HashMap<String, LlmCacheEntry>>,
    ttl: Duration,
}

impl LlmCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            ttl,
        }
    }

    fn cache_key(model: &str, content: &str, kind: &str, params: &str) -> String {
        let content_hash = xxhash_rust::xxh3::xxh3_64(content.as_bytes());
        format!("{model}:{content_hash:x}:{kind}:{params}")
    }

    /// `get_llm_cache(llm_name, content, kind, params)` — `None` on miss or
    /// expiry (RAGFlow evicts by TTL, so an expired entry is a miss).
    pub fn get(
        &self,
        model: &str,
        content: &str,
        kind: &str,
        params: &str,
        now: u64,
    ) -> Option<String> {
        let key = Self::cache_key(model, content, kind, params);
        let entries = self.entries.lock().unwrap();
        let entry = entries.get(&key)?;
        (entry.expires_at > now).then(|| entry.value.clone())
    }

    /// `set_llm_cache(llm_name, content, value, kind, params)`.
    pub fn set(
        &self,
        model: &str,
        content: &str,
        kind: &str,
        params: &str,
        value: impl Into<String>,
        now: u64,
    ) {
        let key = Self::cache_key(model, content, kind, params);
        let entry = LlmCacheEntry {
            value: value.into(),
            expires_at: now.saturating_add(self.ttl.as_millis() as u64),
        };
        self.entries.lock().unwrap().insert(key, entry);
    }

    /// Evict expired entries (TTL sweep). Returns the number evicted.
    pub fn sweep_expired(&self, now: u64) -> usize {
        let mut entries = self.entries.lock().unwrap();
        let before = entries.len();
        entries.retain(|_, entry| entry.expires_at > now);
        before - entries.len()
    }

    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Optional LLM-backed extraction hook: `(content, topn) -> comma/newline
/// separated result`. `None` selects the offline fallback.
pub type ExtractionFn = Arc<dyn Fn(&str, usize) -> String + Send + Sync>;

/// Counters produced by [`ChunkEnrichment::enrich`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnrichReport {
    /// Total keyword terms written across chunks.
    pub keywords: usize,
    /// Total questions written across chunks.
    pub questions: usize,
    /// Chunks that received at least one annotation.
    pub annotated: usize,
}

/// `build_chunks` tail phases: `auto_keywords` + `auto_questions` with LLM
/// cache semantics and offline fallbacks.
pub struct ChunkEnrichment {
    /// RAGFlow `parser_config["auto_keywords"]` (topn; 0 disables).
    pub auto_keywords: usize,
    /// RAGFlow `parser_config["auto_questions"]` (topn; 0 disables).
    pub auto_questions: usize,
    /// LLM hooks; `None` → statistical / template fallbacks.
    pub llm_keywords: Option<ExtractionFn>,
    pub llm_questions: Option<ExtractionFn>,
    pub cache: LlmCache,
}

impl ChunkEnrichment {
    /// Default TTL for the LLM cache (RAGFlow `LLM_CACHE_TTL`-style).
    pub const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(3600);

    pub fn new(auto_keywords: usize, auto_questions: usize) -> Self {
        Self {
            auto_keywords,
            auto_questions,
            llm_keywords: None,
            llm_questions: None,
            cache: LlmCache::new(Self::DEFAULT_CACHE_TTL),
        }
    }

    /// Deterministic offline question fallback: topic-anchored questions from
    /// the chunk's key terms ("What is {term}?"), capped at `topn`; a short
    /// content fallback keeps the phase functional for term-less chunks.
    fn offline_questions(content: &str, topn: usize) -> Vec<String> {
        let mut questions: Vec<String> = Vec::new();
        for term in crate::extractor::extract_key_terms_with_top_n(content, topn) {
            if !term.is_empty() {
                questions.push(format!("What is {term}?"));
            }
        }
        if questions.is_empty() && !content.trim().is_empty() {
            let head: String = content
                .split_whitespace()
                .take(12)
                .collect::<Vec<_>>()
                .join(" ");
            questions.push(format!("What is the main topic of \"{head}\"?"));
        }
        questions.truncate(topn.max(1));
        questions
    }

    /// Keyword extraction with `get_llm_cache` semantics; comma-joined result
    /// like RAGFlow (`important_kwd = cached.split(",")`).
    fn keywords_for(&self, content: &str, topn: usize) -> String {
        if content.trim().is_empty() {
            return String::new();
        }
        let params = format!("{{\"topn\":{topn}}}");
        let now = now_ms();
        if let Some(cached) = self.cache.get("local", content, "keywords", &params, now) {
            return cached;
        }
        let value = match &self.llm_keywords {
            Some(f) => f(content, topn),
            None => crate::extractor::extract_key_terms_with_top_n(content, topn).join(","),
        };
        self.cache
            .set("local", content, "keywords", &params, value.clone(), now);
        value
    }

    /// Question proposal with `get_llm_cache` semantics; newline-joined result
    /// like RAGFlow (`question_kwd = cached.split("\n")`).
    fn questions_for(&self, content: &str, topn: usize) -> Vec<String> {
        if content.trim().is_empty() {
            return Vec::new();
        }
        let params = format!("{{\"topn\":{topn}}}");
        let now = now_ms();
        if let Some(cached) = self.cache.get("local", content, "question", &params, now) {
            return cached
                .split('\n')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
        }
        let value = match &self.llm_questions {
            Some(f) => f(content, topn),
            None => Self::offline_questions(content, topn).join("\n"),
        };
        self.cache
            .set("local", content, "question", &params, value.clone(), now);
        value
            .split('\n')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// Run the `auto_keywords` / `auto_questions` phases over `chunks`,
    /// mirroring the `build_chunks` tail: progress messages ("Start to
    /// generate keywords for every chunk ...", "... completed in {:.2}s"),
    /// per-chunk `important_kwd` / `question_kwd` (+ `_tks`) metadata.
    pub fn enrich(
        &self,
        chunks: &mut [crate::Chunk],
        task: &ExecTask,
        progress: &ProgressCallback,
    ) -> EnrichReport {
        let mut report = EnrichReport::default();
        // `annotated` counts chunks touched by *either* phase (unique chunks).
        let mut annotated_indices: std::collections::HashSet<usize> =
            std::collections::HashSet::new();

        if self.auto_keywords > 0 {
            progress.set_progress(task, 0.0, "Start to generate keywords for every chunk ...");
            let started = std::time::Instant::now();
            for (index, chunk) in chunks.iter_mut().enumerate() {
                if chunk.content.trim().is_empty() {
                    continue;
                }
                let raw = self.keywords_for(&chunk.content, self.auto_keywords);
                let terms: Vec<String> = raw
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
                if terms.is_empty() {
                    continue;
                }
                chunk
                    .metadata
                    .insert("important_kwd".to_string(), terms.join(" "));
                // RAGFlow `important_tks = rag_tokenizer.tokenize(" ".join(...))`.
                chunk
                    .metadata
                    .insert("important_tks".to_string(), terms.join(" "));
                report.keywords += terms.len();
                annotated_indices.insert(index);
            }
            progress.set_progress(
                task,
                0.0,
                &format!(
                    "Keywords generation {} chunks completed in {:.2}s",
                    annotated_indices.len(),
                    started.elapsed().as_secs_f64()
                ),
            );
        }

        if self.auto_questions > 0 {
            progress.set_progress(task, 0.0, "Start to generate questions for every chunk ...");
            let started = std::time::Instant::now();
            for (index, chunk) in chunks.iter_mut().enumerate() {
                if chunk.content.trim().is_empty() {
                    continue;
                }
                let questions = self.questions_for(&chunk.content, self.auto_questions);
                if questions.is_empty() {
                    continue;
                }
                // RAGFlow `question_kwd = cached.split("\n")`,
                // `question_tks = tokenize("\n".join(question_kwd))`.
                chunk
                    .metadata
                    .insert("question_kwd".to_string(), questions.join("\n"));
                chunk
                    .metadata
                    .insert("question_tks".to_string(), questions.join(" "));
                report.questions += questions.len();
                annotated_indices.insert(index);
            }
            progress.set_progress(
                task,
                0.0,
                &format!(
                    "Question generation {} chunks completed in {:.2}s",
                    annotated_indices.len(),
                    started.elapsed().as_secs_f64()
                ),
            );
        }

        report.annotated = annotated_indices.len();
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    async fn wait_until(mut condition: impl FnMut() -> bool, timeout: Duration) {
        let deadline = tokio::time::Instant::now() + timeout;
        while !condition() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "condition not met within {timeout:?}"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Mock chunk + embedding handler: simulates the RAGFlow progress curve of
    /// `do_handle_task` (parse → embedding batches → indexing done) and
    /// records every progress event plus the produced chunk count.
    fn mock_chunk_embedding_handler(
        events: Arc<Mutex<Vec<(f32, String)>>>,
        chunk_count: usize,
    ) -> TaskHandler {
        Arc::new(move |task, progress| {
            let events = events.clone();
            Box::pin(async move {
                progress.set_progress(&task, 0.05, "Parsing document");
                progress.set_progress(&task, 0.7, "Embedding chunks");
                progress.set_progress(&task, 0.9, "Embedding chunks");
                progress.set_progress(&task, 1.0, "Indexing done");
                events.lock().unwrap().extend_from_slice(&[
                    (0.05, String::new()),
                    (0.7, String::new()),
                    (0.9, String::new()),
                    (1.0, String::new()),
                ]);
                Ok(chunk_count)
            })
        })
    }

    #[tokio::test]
    async fn queue_drains_through_state_machine_with_progress() {
        let executor = Arc::new(TaskExecutor::new("test-executor", 2));
        let events: Arc<Mutex<Vec<(f32, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = events.clone();
        let handle = executor.spawn(
            mock_chunk_embedding_handler(events, 3),
            Duration::from_millis(5),
        );

        let mut task = ExecTask::new("t-1", "doc-1", "kb-1", "a.txt");
        task.max_retries = 0;
        task.created_at = 1;
        executor.submit(task).unwrap();

        wait_until(|| executor.stats().done == 1, Duration::from_secs(5)).await;
        assert_eq!(executor.stats().failed, 0);
        assert_eq!(executor.stats().pending, 0);
        assert_eq!(executor.stats().running, 0);
        // The handler emitted 4 progress callbacks (parse → embed → done).
        assert_eq!(recorded.lock().unwrap().len(), 4);
        let terminal = executor.current("t-1").expect("terminal state recorded");
        assert_eq!(terminal.state, TaskState::Done);
        assert_eq!(terminal.state.doc_run(), "DONE");
        assert!(terminal.progress >= 1.0);
        // report_status-style snapshot reflects the completed run.
        let status = executor.status();
        assert_eq!(status.name, "test-executor");
        assert_eq!(status.done, 1);
        assert_eq!(status.failed, 0);
        assert_eq!(status.boot_at, executor.boot_at());

        executor.shutdown(handle).await;
    }

    #[tokio::test]
    async fn retryable_failure_requeues_then_succeeds() {
        let executor = Arc::new(TaskExecutor::new("retry-executor", 2));
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_observed = attempts.clone();
        let handler: TaskHandler = Arc::new(move |_task, _progress| {
            let attempts = attempts_observed.clone();
            Box::pin(async move {
                let attempt = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                if attempt == 1 {
                    // First attempt blows up; RAGFlow catch → re-queue.
                    Err(TaskError::retryable("chunking exploded"))
                } else {
                    Ok(2)
                }
            })
        });
        let handle = executor.spawn(handler, Duration::from_millis(5));

        let mut task = ExecTask::new("t-retry", "doc-2", "kb-2", "b.txt");
        task.max_retries = 3;
        executor.submit(task).unwrap();

        wait_until(|| executor.stats().done == 1, Duration::from_secs(5)).await;
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        assert_eq!(executor.stats().failed, 0);
        let terminal = executor.current("t-retry").unwrap();
        assert_eq!(terminal.state, TaskState::Done);
        assert_eq!(terminal.attempts, 1); // one retry was consumed
        executor.shutdown(handle).await;
    }

    #[tokio::test]
    async fn retries_exhaust_into_failed_state() {
        let executor = Arc::new(TaskExecutor::new("fail-executor", 2));
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_observed = attempts.clone();
        let handler: TaskHandler = Arc::new(move |_task, progress| {
            let attempts = attempts_observed.clone();
            Box::pin(async move {
                attempts.fetch_add(1, Ordering::SeqCst);
                progress.set_progress(&_task, -1.0, "chunking exploded");
                Err(TaskError::retryable("chunking exploded"))
            })
        });
        let handle = executor.spawn(handler, Duration::from_millis(5));

        let mut task = ExecTask::new("t-fail", "doc-3", "kb-3", "c.txt");
        task.max_retries = 2; // initial attempt + 2 retries = 3 executions
        executor.submit(task).unwrap();

        wait_until(|| executor.stats().failed == 1, Duration::from_secs(5)).await;
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
        assert_eq!(executor.stats().done, 0);
        let terminal = executor.current("t-fail").unwrap();
        assert_eq!(terminal.state, TaskState::Failed);
        assert_eq!(terminal.state.doc_run(), "FAILED");
        assert!(terminal.message.contains("chunking exploded"));
        executor.shutdown(handle).await;
    }

    #[tokio::test]
    async fn management_loop_respects_concurrency_limit() {
        let executor = Arc::new(TaskExecutor::new("concurrent-executor", 2));
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let running_gauge = running.clone();
        let peak_gauge = peak.clone();
        let handler: TaskHandler = Arc::new(move |_task, _progress| {
            let running = running_gauge.clone();
            let peak = peak_gauge.clone();
            Box::pin(async move {
                let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(80)).await;
                running.fetch_sub(1, Ordering::SeqCst);
                Ok(0)
            })
        });
        let handle = executor.spawn(handler, Duration::from_millis(5));

        for i in 0..6 {
            executor
                .submit(ExecTask::new(
                    format!("t-{i}"),
                    format!("doc-{i}"),
                    "kb-4",
                    "d.txt",
                ))
                .unwrap();
        }
        wait_until(|| executor.stats().done == 6, Duration::from_secs(10)).await;
        assert_eq!(executor.stats().failed, 0);
        assert!(
            peak.load(Ordering::SeqCst) <= 2,
            "peak concurrency {peak:?} exceeded limit"
        );
        assert_eq!(
            peak.load(Ordering::SeqCst),
            2,
            "concurrency limit was never reached"
        );

        // Shutdown closes the queue; further submissions are rejected.
        executor.shutdown(handle).await;
        assert!(
            executor
                .submit(ExecTask::new("t-late", "doc-late", "kb-4", "e.txt"))
                .is_err()
        );
    }

    #[test]
    fn progress_callback_matches_ragflow_set_progress_semantics() {
        let events: Arc<Mutex<Vec<(f32, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = events.clone();
        let callback = ProgressCallback::new(move |prog, message| {
            recorded.lock().unwrap().push((prog, message));
        });

        let mut task = ExecTask::new("t-p", "doc-p", "kb-p", "f.txt");
        task.from_page = 0;
        task.to_page = 9;

        // Normal progress: timestamp + page range prefix (RAGFlow set_progress
        // prepends "HH:MM:SS " to the whole message).
        callback.set_progress(&task, 0.5, "Embedding chunks");
        let (prog, message) = events.lock().unwrap()[0].clone();
        assert_eq!(prog, 0.5);
        assert!(message.contains("Page(1~10): "), "unexpected: {message}");
        assert!(message.contains("Embedding chunks"));
        // RAGFlow prefixes every progress message with a "HH:MM:SS " timestamp.
        let head: String = message.chars().take(8).collect();
        assert!(
            head.len() == 8
                && head.as_bytes()[2] == b':'
                && head.as_bytes()[5] == b':'
                && head.as_bytes()[..2].iter().all(u8::is_ascii_digit)
                && head.as_bytes()[3..5].iter().all(u8::is_ascii_digit)
                && head.as_bytes()[6..8].iter().all(u8::is_ascii_digit),
            "missing HH:MM:SS timestamp in {message}"
        );

        // Error progress: [ERROR] prefix, message kept.
        callback.set_progress(&task, -1.0, "chunking exploded");
        let message = events.lock().unwrap()[1].1.clone();
        assert!(
            message.contains("[ERROR]chunking exploded"),
            "unexpected: {message}"
        );

        // to_page == -1 (whole document): no page range prefix.
        let mut whole = task.clone();
        whole.to_page = -1;
        callback.set_progress(&whole, 1.0, "Indexing done");
        let message = events.lock().unwrap()[2].1.clone();
        assert!(!message.contains("Page("), "unexpected: {message}");
        assert!(message.contains("Indexing done"));

        // to_page == from_page: no page range prefix (RAGFlow: from_page < to_page).
        let mut single = task.clone();
        single.from_page = 3;
        single.to_page = 3;
        callback.set_progress(&single, 0.1, "Parsing");
        let message = events.lock().unwrap()[3].1.clone();
        assert!(!message.contains("Page("), "unexpected: {message}");
    }

    // ---- sync task (sync_data_source.py port) ----------------------------

    fn sync_doc(id: &str, updated: &str) -> SyncDoc {
        SyncDoc {
            id: id.into(),
            semantic_identifier: format!("{id}.md"),
            extension: "md".into(),
            size_bytes: 1,
            doc_updated_at: updated.into(),
            blob: format!("content of {id}"),
            fingerprint: None,
            metadata: HashMap::new(),
        }
    }

    struct TestConnector {
        source: &'static str,
        feed: SyncFeed,
        delay: Option<Duration>,
    }

    impl SyncConnector for TestConnector {
        fn source_name(&self) -> &str {
            self.source
        }

        fn generate<'a>(
            &'a self,
            _task: &SyncTask,
        ) -> Pin<Box<dyn Future<Output = Result<SyncFeed, String>> + Send + 'a>> {
            let feed = self.feed.clone();
            let delay = self.delay;
            Box::pin(async move {
                if let Some(delay) = delay {
                    tokio::time::sleep(delay).await;
                }
                Ok(feed)
            })
        }
    }

    #[tokio::test]
    async fn sync_run_tracks_added_updated_skipped_and_window_advance() {
        let mut task = SyncTask::new("sync-1", "conn-1", "kb-1");
        task.source_name = "gitlab".into();
        task.poll_range_start = Some("2026-08-05T08:00:00Z".into());

        let doc_1 = sync_doc("doc-1", "2026-08-05T09:00:00Z");
        let doc_2 = sync_doc("doc-2", "2026-08-05T10:00:00Z");
        let mut doc_3 = sync_doc("doc-3", "2026-08-05T11:00:00Z");
        doc_3.fingerprint = Some("fp-3".into());
        // Same id as doc_3, but a *changed* fingerprint: must be re-fetched.
        let mut doc_3_changed = sync_doc("doc-3", "2026-08-05T11:30:00Z");
        doc_3_changed.fingerprint = Some("fp-3-new".into());

        let feed = SyncFeed {
            batches: vec![
                vec![doc_1.clone(), doc_2.clone()],
                vec![doc_3.clone(), doc_3_changed.clone()],
            ],
            file_list: None,
        };
        let connector = TestConnector {
            source: "gitlab",
            feed,
            delay: None,
        };

        let existing_ids: std::collections::HashSet<String> =
            ["doc-2"].into_iter().map(str::to_string).collect();
        let existing_fps: std::collections::HashMap<String, String> =
            [("doc-3".to_string(), "fp-3".to_string())]
                .into_iter()
                .collect();

        let changes: Arc<Mutex<Vec<(String, SyncChange)>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = changes.clone();
        let summary = run_sync_task(
            &task,
            &connector,
            &existing_ids,
            &existing_fps,
            move |doc, change| {
                recorded.lock().unwrap().push((doc.id.clone(), change));
                Ok(())
            },
        )
        .await
        .expect("sync run succeeds");

        // doc-1 added, doc-2 updated, doc-3 bypassed by fingerprint,
        // doc-3_changed added (new fingerprint, new id in the run's `known`).
        assert_eq!(summary.added, 2);
        assert_eq!(summary.updated, 1);
        assert_eq!(summary.skipped, 1);
        assert_eq!(summary.failed, 0);
        assert_eq!(summary.total, 3);
        // Window advances to the max doc_updated_at of *fetched* docs —
        // fingerprint-bypassed docs never enter the window (RAGFlow generator).
        assert_eq!(
            summary.next_poll_range_start.as_deref(),
            Some("2026-08-05T11:30:00Z")
        );
        let changes = changes.lock().unwrap();
        assert_eq!(changes.len(), 3);
        assert!(changes.contains(&("doc-1".into(), SyncChange::Added)));
        assert!(changes.contains(&("doc-2".into(), SyncChange::Updated)));
        assert!(changes.contains(&("doc-3".into(), SyncChange::Added)));
    }

    #[tokio::test]
    async fn sync_reconciles_deleted_files_against_remote_snapshot() {
        let mut task = SyncTask::new("sync-2", "conn-1", "kb-1");
        task.poll_range_start = Some("2026-08-05T08:00:00Z".into());
        task.sync_deleted_files = true;

        let feed = SyncFeed {
            batches: vec![vec![sync_doc("doc-1", "2026-08-05T09:00:00Z")]],
            // Remote snapshot no longer contains doc-2 (deleted upstream).
            file_list: Some(vec!["doc-1".into()]),
        };
        let connector = TestConnector {
            source: "s3",
            feed,
            delay: None,
        };
        let existing_ids: std::collections::HashSet<String> =
            ["doc-1", "doc-2"].into_iter().map(str::to_string).collect();

        let summary = run_sync_task(
            &task,
            &connector,
            &existing_ids,
            &std::collections::HashMap::new(),
            |_doc, _change| Ok(()),
        )
        .await
        .expect("sync run succeeds");

        assert_eq!(summary.added, 0);
        assert_eq!(summary.updated, 1); // doc-1 already in the KB
        assert_eq!(summary.deleted, 1); // doc-2 reconciled as deleted
        assert_eq!(summary.total, 2);

        // Deletion reconciliation requires the snapshot flag + incremental run.
        task.sync_deleted_files = false;
        let connector = TestConnector {
            source: "s3",
            feed: SyncFeed {
                batches: vec![],
                file_list: Some(vec!["doc-1".into()]),
            },
            delay: None,
        };
        let summary = run_sync_task(
            &task,
            &connector,
            &existing_ids,
            &std::collections::HashMap::new(),
            |_doc, _change| Ok(()),
        )
        .await
        .expect("sync run succeeds");
        assert_eq!(summary.deleted, 0);
    }

    #[tokio::test]
    async fn sync_timeout_and_failure_are_permanent_outcomes() {
        // asyncio.wait_for: a connector that hangs past timeout_secs → Timeout.
        let task = SyncTask {
            id: "sync-3".into(),
            connector_id: "conn-1".into(),
            kb_id: "kb-1".into(),
            tenant_id: String::new(),
            source_name: "slow".into(),
            poll_range_start: None,
            timeout_secs: 1,
            auto_parse: true,
            reindex: false,
            sync_deleted_files: false,
        };
        let slow = TestConnector {
            source: "slow",
            feed: SyncFeed::default(),
            delay: Some(Duration::from_secs(30)),
        };
        match execute_sync(
            &task,
            &slow,
            &std::collections::HashSet::new(),
            &std::collections::HashMap::new(),
            |_doc, _change| Ok(()),
        )
        .await
        {
            SyncOutcome::Timeout(message) => {
                assert!(message.contains("Task timeout after 1 seconds"));
            }
            other => panic!("expected Timeout, got {other:?}"),
        }

        // A connector error surfaces as a permanent Failed outcome.
        struct Failing;
        impl SyncConnector for Failing {
            fn source_name(&self) -> &str {
                "failing"
            }
            fn generate<'a>(
                &'a self,
                _task: &SyncTask,
            ) -> Pin<Box<dyn Future<Output = Result<SyncFeed, String>> + Send + 'a>> {
                Box::pin(async move { Err("connector exploded".into()) })
            }
        }
        match execute_sync(
            &task,
            &Failing,
            &std::collections::HashSet::new(),
            &std::collections::HashMap::new(),
            |_doc, _change| Ok(()),
        )
        .await
        {
            SyncOutcome::Failed(message) => assert_eq!(message, "connector exploded"),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn sync_window_info_formats_boundaries_like_ragflow() {
        // `_format_window_boundary(None)` → "beginning".
        assert_eq!(format_window_boundary(None), "beginning");
        // RFC3339 boundary renders as "%Y-%m-%d %H:%M:%S %Z".
        let formatted = format_window_boundary(Some("2026-08-05T10:00:00Z"));
        assert!(
            formatted.starts_with("2026-08-05 10:00:00"),
            "unexpected: {formatted}"
        );
        // Unparseable values pass through untouched.
        assert_eq!(format_window_boundary(Some("garbage")), "garbage");

        let mut task = SyncTask::new("sync-w", "conn-1", "kb-1");
        let info = sync_window_info(&task);
        assert!(info.starts_with("sync window: beginning -> "), "{info}");

        task.poll_range_start = Some("2026-08-05T10:00:00Z".into());
        let info = sync_window_info(&task);
        assert!(
            info.starts_with("sync window: 2026-08-05 10:00:00 "),
            "{info}"
        );

        // A reindex ignores the poll window (`window_info` uses None start).
        task.reindex = true;
        let info = sync_window_info(&task);
        assert!(info.starts_with("sync window: beginning -> "), "{info}");
    }

    // ---- build_chunks enrichment (auto_keywords / auto_questions) ---------

    fn enrich_chunk(text: &str) -> crate::Chunk {
        crate::Chunk {
            id: uuid::Uuid::new_v4().to_string(),
            content: text.into(),
            content_type: "text".into(),
            doc_id: uuid::Uuid::new_v4(),
            position: 0,
            token_count: 0,
            embedding: None,
            metadata: HashMap::new(),
        }
    }

    #[test]
    fn enrich_annotates_keywords_and_questions_with_cache_semantics() {
        let text = "Vector search indexes vectors; vector search over vectors is fast.";
        let mut chunks = vec![enrich_chunk(text), enrich_chunk("   \n  ")];
        let task = ExecTask::new("t-enrich", "doc-e", "kb-e", "e.txt");
        let events: Arc<Mutex<Vec<(f32, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = events.clone();
        let progress = ProgressCallback::new(move |prog, message| {
            recorded.lock().unwrap().push((prog, message));
        });

        let enrichment = ChunkEnrichment::new(3, 2);
        let report = enrichment.enrich(&mut chunks, &task, &progress);

        // Non-empty chunk annotated; empty chunk skipped.
        assert_eq!(report.annotated, 1);
        assert_eq!(report.keywords, 3);
        assert_eq!(report.questions, 2);

        let meta = &chunks[0].metadata;
        let keywords = meta.get("important_kwd").expect("important_kwd set");
        assert!(keywords.contains("vector"));
        assert!(meta.get("important_tks").is_some());
        let questions = meta
            .get("question_kwd")
            .expect("question_kwd set")
            .split('\n')
            .filter(|s| !s.is_empty())
            .count();
        assert_eq!(questions, 2);
        assert!(meta.get("question_tks").is_some());
        assert!(chunks[1].metadata.get("important_kwd").is_none());
        assert!(chunks[1].metadata.get("question_kwd").is_none());

        // build_chunks progress messages (start + completion) are emitted.
        let messages: Vec<String> = events
            .lock()
            .unwrap()
            .iter()
            .map(|(_, m)| m.clone())
            .collect();
        assert!(
            messages
                .iter()
                .any(|m| m.contains("Start to generate keywords"))
        );
        assert!(
            messages
                .iter()
                .any(|m| m.contains("Keywords generation 1 chunks completed"))
        );
        assert!(
            messages
                .iter()
                .any(|m| m.contains("Start to generate questions"))
        );
        assert!(
            messages
                .iter()
                .any(|m| m.contains("Question generation 1 chunks completed"))
        );

        // Second pass reuses the LLM cache: identical results, no re-extraction.
        let mut chunks2 = vec![enrich_chunk(text)];
        let report2 = enrichment.enrich(&mut chunks2, &task, &progress);
        assert_eq!(report2.keywords, report.keywords);
        assert_eq!(report2.questions, report.questions);
        assert_eq!(
            chunks2[0].metadata.get("important_kwd"),
            chunks[0].metadata.get("important_kwd")
        );
        assert_eq!(
            chunks2[0].metadata.get("question_kwd"),
            chunks[0].metadata.get("question_kwd")
        );
        assert_eq!(
            enrichment.cache.len(),
            2,
            "keywords + question entries cached"
        );
    }

    #[test]
    fn llm_cache_get_set_ttl_and_sweep() {
        let cache = LlmCache::new(Duration::from_secs(60));
        let now = 1_000_000u64;
        assert!(cache.is_empty());

        cache.set(
            "chat-1",
            "some content",
            "keywords",
            "{\"topn\":2}",
            "a,b",
            now,
        );
        assert_eq!(
            cache
                .get("chat-1", "some content", "keywords", "{\"topn\":2}", now)
                .as_deref(),
            Some("a,b")
        );
        // Different kind / content / model → distinct cache keys.
        assert!(
            cache
                .get("chat-1", "some content", "question", "{\"topn\":2}", now)
                .is_none()
        );
        assert!(
            cache
                .get("chat-1", "other content", "keywords", "{\"topn\":2}", now)
                .is_none()
        );
        assert!(
            cache
                .get("chat-2", "some content", "keywords", "{\"topn\":2}", now)
                .is_none()
        );
        assert_eq!(cache.len(), 1);

        // TTL expiry: expired entries read as a miss and are swept.
        let later = now + 61_000;
        assert!(
            cache
                .get("chat-1", "some content", "keywords", "{\"topn\":2}", later)
                .is_none()
        );
        assert_eq!(cache.sweep_expired(later), 1);
        assert!(cache.is_empty());
    }
}
