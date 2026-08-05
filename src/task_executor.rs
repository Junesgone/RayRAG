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
use tokio::sync::{mpsc, Semaphore};

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
    pub fn new(id: impl Into<String>, doc_id: impl Into<String>, kb_id: impl Into<String>, name: impl Into<String>) -> Self {
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
        if task.to_page > 0 && task.from_page < task.to_page as usize {
            if !message.is_empty() {
                message = format!("Page({}~{}): {message}", task.from_page + 1, task.to_page + 1);
            }
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
    pub fn spawn(self: &Arc<Self>, handler: TaskHandler, retry_backoff: Duration) -> tokio::task::JoinHandle<()> {
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
        self.current
            .lock()
            .unwrap()
            .insert(task.id.clone(), task.clone_with_state(TaskState::Processing));

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
                self.current.lock().unwrap().insert(retry.id.clone(), retry.clone());
                Some(retry)
            }
            Err(error) => {
                let mut failed = task.clone();
                failed.state = TaskState::Failed;
                failed.progress = 1.0;
                failed.message = error.to_string();
                self.current.lock().unwrap().insert(failed.id.clone(), failed);
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
                events
                    .lock()
                    .unwrap()
                    .extend_from_slice(&[(0.05, String::new()), (0.7, String::new()), (0.9, String::new()), (1.0, String::new())]);
                Ok(chunk_count)
            })
        })
    }

    #[tokio::test]
    async fn queue_drains_through_state_machine_with_progress() {
        let executor = Arc::new(TaskExecutor::new("test-executor", 2));
        let events: Arc<Mutex<Vec<(f32, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let recorded = events.clone();
        let handle = executor.spawn(mock_chunk_embedding_handler(events, 3), Duration::from_millis(5));

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
                .submit(ExecTask::new(format!("t-{i}"), format!("doc-{i}"), "kb-4", "d.txt"))
                .unwrap();
        }
        wait_until(|| executor.stats().done == 6, Duration::from_secs(10)).await;
        assert_eq!(executor.stats().failed, 0);
        assert!(peak.load(Ordering::SeqCst) <= 2, "peak concurrency {peak:?} exceeded limit");
        assert_eq!(peak.load(Ordering::SeqCst), 2, "concurrency limit was never reached");

        // Shutdown closes the queue; further submissions are rejected.
        executor.shutdown(handle).await;
        assert!(executor
            .submit(ExecTask::new("t-late", "doc-late", "kb-4", "e.txt"))
            .is_err());
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
        assert!(message.contains("[ERROR]chunking exploded"), "unexpected: {message}");

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
}
