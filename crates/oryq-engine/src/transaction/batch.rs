//! Queued write batching & single-fsync group commit (`X03` / `v0.10.3`).
//!
//! Provides both:
//! - Explicit ordered batch execution ([`commit_batch`], [`StorageEngine::commit_batch`]) where
//!   each item executes inside its own statement savepoint so validation failures roll back only
//!   the failing item while all valid items commit in a single WAL write + fsync.
//! - Lock-free-of-daemons leader-follower [`GroupCommitQueue`] coalescing concurrent writer
//!   submissions into bounded single-fsync group commits with full unwind safety and
//!   `CommitOutcomeUnknown` propagation.

use std::any::Any;
use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use oryq_types::{ErrorCategory, ErrorEnvelope, Result};

use crate::ports::CommitReceipt;
use crate::transaction::{StorageEngine, WriteTxn};

const MODULE_ID: &str = "engine";

/// Outcome of an ordered [`commit_batch`] execution.
#[derive(Debug)]
pub struct BatchCommitReport<T> {
    /// Durable WAL commit receipt shared by all succeeded items (`None` if 0 items succeeded).
    pub receipt: Option<CommitReceipt>,
    /// Per-item outcomes in exact submission order.
    pub results: Vec<Result<T>>,
    /// Number of items that succeeded and were durably committed.
    pub committed_count: usize,
    /// Number of items that failed validation/execution and were rolled back via savepoint.
    pub failed_count: usize,
}

/// Configuration for leader-follower queued group commit ([`GroupCommitQueue`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupCommitConfig {
    /// Maximum number of queued write operations coalesced into a single WAL fsync (`>= 1`).
    pub max_batch_size: usize,
    /// Optional linger window for the leader to wait for additional concurrent writers before
    /// starting a batch commit (`Duration::ZERO` drains immediately with zero added latency).
    pub linger: Duration,
    /// Maximum total queued operations waiting for commit before rejecting new callers with `Busy`.
    pub max_queue_depth: usize,
}

impl Default for GroupCommitConfig {
    fn default() -> Self {
        Self {
            max_batch_size: 64,
            linger: Duration::ZERO,
            max_queue_depth: 1024,
        }
    }
}

/// Executes an ordered batch of write closures against `engine` inside a single [`WriteTxn`]
/// and a single durable WAL fsync.
///
/// - Each item runs inside its own statement savepoint: if an item returns `Err(e)`, only that
///   item's page mutations are rolled back and subsequent items continue in order.
/// - If zero items succeed, the transaction is rolled back with zero WAL writes and zero fsyncs.
/// - If the single WAL write/sync fails (including [`ErrorCategory::CommitOutcomeUnknown`]),
///   the engine is poisoned (for uncertain outcomes) and `Err(commit_err)` is returned so no
///   caller observes a false durable commit.
pub fn commit_batch<T, F>(engine: &StorageEngine, items: Vec<F>) -> Result<BatchCommitReport<T>>
where
    F: FnOnce(&mut WriteTxn<'_>) -> Result<T>,
{
    if engine.is_read_only() {
        return Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "batch::commit_batch",
            "cannot execute commit_batch on a read-only StorageEngine",
        ));
    }
    if engine.is_poisoned() {
        return Err(ErrorEnvelope::new(
            ErrorCategory::CommitOutcomeUnknown,
            MODULE_ID,
            "batch::commit_batch",
            "engine is poisoned after an uncertain commit write/sync failure; close and reopen to recover",
        ));
    }
    if items.is_empty() {
        return Ok(BatchCommitReport {
            receipt: None,
            results: Vec::new(),
            committed_count: 0,
            failed_count: 0,
        });
    }

    let mut wtx = engine.begin_write()?;
    let mut results = Vec::with_capacity(items.len());
    let mut committed_count = 0usize;
    let mut failed_count = 0usize;

    for item in items {
        let sp = wtx.savepoint();
        match item(&mut wtx) {
            Ok(val) => {
                wtx.release_savepoint(sp)?;
                results.push(Ok(val));
                committed_count += 1;
            }
            Err(err) => {
                wtx.rollback_to(sp)?;
                results.push(Err(err));
                failed_count += 1;
            }
        }
    }

    if committed_count == 0 {
        wtx.rollback();
        return Ok(BatchCommitReport {
            receipt: None,
            results,
            committed_count: 0,
            failed_count,
        });
    }

    let receipt = wtx.commit()?;
    Ok(BatchCommitReport {
        receipt: Some(receipt),
        results,
        committed_count,
        failed_count,
    })
}

/// Type-erased queued write task for leader-follower [`GroupCommitQueue`].
type BoxedAny = Box<dyn Any + Send>;
type ResultSlot<T> = Arc<Mutex<Option<Result<(T, CommitReceipt)>>>>;
type TaskRunFn<Tx> = Box<dyn FnOnce(&mut Tx) -> Result<BoxedAny> + Send>;
type TaskDeliverFn = Box<dyn FnOnce(Result<(BoxedAny, CommitReceipt)>) + Send>;

struct QueuedTask<Tx> {
    run: TaskRunFn<Tx>,
    deliver: TaskDeliverFn,
}

struct QueueState<Tx> {
    pending: VecDeque<QueuedTask<Tx>>,
    leader_active: bool,
}

/// Unwind-safe leader-follower group-commit queue that coalesces concurrent write submissions
/// into ordered single-fsync batches without spawning background daemon threads.
pub struct GroupCommitQueue<Tx> {
    config: GroupCommitConfig,
    state: Mutex<QueueState<Tx>>,
    cv: Condvar,
}

impl<Tx> fmt::Debug for GroupCommitQueue<Tx> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupCommitQueue")
            .field("config", &self.config)
            .finish()
    }
}

impl<Tx> GroupCommitQueue<Tx> {
    pub fn new(config: GroupCommitConfig) -> Self {
        let sanitized = GroupCommitConfig {
            max_batch_size: config.max_batch_size.max(1),
            linger: config.linger,
            max_queue_depth: config.max_queue_depth.max(1),
        };
        Self {
            config: sanitized,
            state: Mutex::new(QueueState {
                pending: VecDeque::new(),
                leader_active: false,
            }),
            cv: Condvar::new(),
        }
    }

    pub fn config(&self) -> GroupCommitConfig {
        self.config
    }

    /// Submits a single write operation to the group-commit queue and waits for its durable
    /// `(result, CommitReceipt)` outcome.
    ///
    /// `execute_batch` is invoked ONLY by the elected leader with a drained FIFO slice of tasks
    /// and must execute each task inside its own savepoint and commit all succeeded tasks in a
    /// single WAL fsync.
    pub fn submit<T, F, E>(&self, op: F, mut execute_batch: E) -> Result<(T, CommitReceipt)>
    where
        T: Send + 'static,
        F: FnOnce(&mut Tx) -> Result<T> + Send + 'static,
        E: FnMut(Vec<BatchItem<Tx>>) -> Result<()>,
    {
        let slot: ResultSlot<T> = Arc::new(Mutex::new(None));
        let slot_for_deliver = Arc::clone(&slot);

        let task = QueuedTask {
            run: Box::new(move |tx: &mut Tx| -> Result<BoxedAny> {
                let val = op(tx)?;
                Ok(Box::new(val))
            }),
            deliver: Box::new(move |res: Result<(BoxedAny, CommitReceipt)>| {
                let typed = res.and_then(|(boxed, receipt)| {
                    boxed.downcast::<T>().map(|v| (*v, receipt)).map_err(|_| {
                        ErrorEnvelope::new(
                            ErrorCategory::Internal,
                            MODULE_ID,
                            "GroupCommitQueue::submit",
                            "group commit result type downcast mismatch",
                        )
                    })
                });
                if let Ok(mut guard) = slot_for_deliver.lock() {
                    *guard = Some(typed);
                }
            }),
        };

        let mut st = self.state.lock().map_err(|_| queue_poisoned())?;
        if st.pending.len() >= self.config.max_queue_depth {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Busy,
                MODULE_ID,
                "GroupCommitQueue::submit",
                format!(
                    "group commit queue full (max_queue_depth={})",
                    self.config.max_queue_depth
                ),
            ));
        }
        st.pending.push_back(task);

        loop {
            if let Some(res) = slot.lock().map_err(|_| queue_poisoned())?.take() {
                return res;
            }

            if !st.leader_active {
                st.leader_active = true;

                // Optional linger to coalesce arriving burst before draining.
                if !self.config.linger.is_zero() && st.pending.len() < self.config.max_batch_size {
                    let deadline = Instant::now() + self.config.linger;
                    while st.pending.len() < self.config.max_batch_size {
                        let now = Instant::now();
                        if now >= deadline {
                            break;
                        }
                        let (guard, timeout) = self
                            .cv
                            .wait_timeout(st, deadline - now)
                            .map_err(|_| queue_poisoned())?;
                        st = guard;
                        if timeout.timed_out() {
                            break;
                        }
                    }
                }

                let drain_count = st.pending.len().min(self.config.max_batch_size);
                let mut drained = Vec::with_capacity(drain_count);
                for _ in 0..drain_count {
                    if let Some(t) = st.pending.pop_front() {
                        drained.push(BatchItem {
                            run: Some(t.run),
                            deliver: Some(t.deliver),
                        });
                    }
                }
                drop(st);

                // RAII guard ensures `leader_active` is ALWAYS cleared and waiters woken even on panic.
                struct LeaderGuard<'q, Tx> {
                    queue: &'q GroupCommitQueue<Tx>,
                }
                impl<Tx> Drop for LeaderGuard<'_, Tx> {
                    fn drop(&mut self) {
                        if let Ok(mut st) = self.queue.state.lock() {
                            st.leader_active = false;
                            self.queue.cv.notify_all();
                        }
                    }
                }
                let _leader_guard = LeaderGuard { queue: self };

                let _ = execute_batch(drained);

                drop(_leader_guard);
                st = self.state.lock().map_err(|_| queue_poisoned())?;
                continue;
            }

            st = self.cv.wait(st).map_err(|_| queue_poisoned())?;
        }
    }
}

/// One drained task handed to a leader batch executor.
///
/// If dropped without calling [`Self::deliver_ok`] or [`Self::deliver_err`] (for example on panic),
/// automatically delivers an `Internal` error so waiting followers never hang.
pub struct BatchItem<Tx> {
    run: Option<TaskRunFn<Tx>>,
    deliver: Option<TaskDeliverFn>,
}

impl<Tx> fmt::Debug for BatchItem<Tx> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BatchItem")
            .field("pending_run", &self.run.is_some())
            .field("pending_deliver", &self.deliver.is_some())
            .finish()
    }
}

impl<Tx> BatchItem<Tx> {
    /// Executes the queued closure against `tx`.
    pub fn execute(&mut self, tx: &mut Tx) -> Result<BoxedAny> {
        let f = self.run.take().ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Internal,
                MODULE_ID,
                "BatchItem::execute",
                "batch item already executed",
            )
        })?;
        f(tx)
    }

    /// Delivers a durable `(value, CommitReceipt)` result to the waiting caller.
    pub fn deliver_ok(mut self, val: BoxedAny, receipt: CommitReceipt) {
        if let Some(d) = self.deliver.take() {
            d(Ok((val, receipt)));
        }
    }

    /// Delivers an error to the waiting caller.
    pub fn deliver_err(mut self, err: ErrorEnvelope) {
        if let Some(d) = self.deliver.take() {
            d(Err(err));
        }
    }
}

impl<Tx> Drop for BatchItem<Tx> {
    fn drop(&mut self) {
        if let Some(d) = self.deliver.take() {
            d(Err(ErrorEnvelope::new(
                ErrorCategory::Internal,
                MODULE_ID,
                "BatchItem::drop",
                "group commit leader aborted before delivering batch item outcome",
            )));
        }
    }
}

/// Helper to clone an [`ErrorEnvelope`] (preserving category, module, operation, and message)
/// when broadcasting a shared group-commit failure (such as `CommitOutcomeUnknown`) to all
/// followers in a batch.
pub fn clone_error_envelope(err: &ErrorEnvelope) -> ErrorEnvelope {
    ErrorEnvelope::new(
        err.category(),
        err.module_id(),
        err.operation(),
        err.message().to_owned(),
    )
}

fn queue_poisoned() -> ErrorEnvelope {
    ErrorEnvelope::new(
        ErrorCategory::Internal,
        MODULE_ID,
        "GroupCommitQueue",
        "group commit queue mutex poisoned",
    )
}
