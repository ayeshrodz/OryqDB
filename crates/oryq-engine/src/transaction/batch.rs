//! Queued write batching & single-fsync group commit (`X03` / `v0.10.3`).
//!
//! Provides both:
//! - Explicit ordered batch execution ([`commit_batch`], [`StorageEngine::commit_batch`]) where
//!   each item executes inside its own statement savepoint so validation failures roll back only
//!   the failing item while all valid items commit in a single WAL write + fsync.
//! - Zero-daemon leader-follower [`GroupCommitQueue`] parameterized by a GAT [`BatchTx`] marker
//!   so both `oryq-engine::WriteTxn<'_>` and `oryq-core::WriteTransaction<'_>` share the exact
//!   same unwind-safe group-commit coordinator with early linger wakeup, bounded queue depth,
//!   and `CommitOutcomeUnknown` propagation.

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
                if let Err(err) = wtx.release_savepoint(sp) {
                    let _ = wtx.rollback_to(sp);
                    return Err(err);
                }
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

/// GAT marker trait associating a lifetime-parameterized transaction handle (such as
/// `oryq_engine::WriteTxn<'a>` or `oryq_core::WriteTransaction<'a>`) with a [`GroupCommitQueue`].
pub trait BatchTx {
    type Tx<'a>;
}

/// Built-in [`BatchTx`] marker for [`WriteTxn`].
#[derive(Debug, Default, Clone, Copy)]
pub struct EngineWriteBatchTx;

impl BatchTx for EngineWriteBatchTx {
    type Tx<'a> = WriteTxn<'a>;
}

type BoxedAny = Box<dyn Any + Send>;
type ResultSlot<T> = Arc<Mutex<Option<Result<(T, CommitReceipt)>>>>;
type TaskRunFn<B> = Box<dyn for<'a> FnOnce(&mut <B as BatchTx>::Tx<'a>) -> Result<BoxedAny> + Send>;
type TaskDeliverFn = Box<dyn FnOnce(Result<(BoxedAny, CommitReceipt)>) + Send>;

struct QueuedTask<B: BatchTx> {
    run: TaskRunFn<B>,
    deliver: TaskDeliverFn,
}

struct QueueState<B: BatchTx> {
    pending: VecDeque<QueuedTask<B>>,
    leader_active: bool,
    leader_lingering: bool,
}

/// Unwind-safe leader-follower group-commit queue that coalesces concurrent write submissions
/// into ordered single-fsync batches without spawning background daemon threads.
pub struct GroupCommitQueue<B: BatchTx> {
    config: GroupCommitConfig,
    state: Mutex<QueueState<B>>,
    cv: Condvar,
    leader_cv: Condvar,
}

impl<B: BatchTx> fmt::Debug for GroupCommitQueue<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GroupCommitQueue")
            .field("config", &self.config)
            .finish()
    }
}

impl<B: BatchTx> GroupCommitQueue<B> {
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
                leader_lingering: false,
            }),
            cv: Condvar::new(),
            leader_cv: Condvar::new(),
        }
    }

    pub fn config(&self) -> GroupCommitConfig {
        self.config
    }

    /// Submits a single write operation to the group-commit queue and waits for its durable
    /// `(result, CommitReceipt)` outcome.
    ///
    /// `execute_batch` is invoked ONLY by the elected leader with a drained FIFO batch of items
    /// and must execute each item inside its own savepoint and deliver every item's outcome via
    /// [`BatchItem::deliver_ok`] or [`BatchItem::deliver_err`].
    pub fn submit<T, F, E>(&self, op: F, mut execute_batch: E) -> Result<(T, CommitReceipt)>
    where
        T: Send + 'static,
        F: for<'a> FnOnce(&mut B::Tx<'a>) -> Result<T> + Send + 'static,
        E: FnMut(Vec<BatchItem<B>>),
    {
        let slot: ResultSlot<T> = Arc::new(Mutex::new(None));
        let slot_for_deliver = Arc::clone(&slot);

        let task = QueuedTask {
            run: Box::new(move |tx: &mut B::Tx<'_>| -> Result<BoxedAny> {
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

        // Wake a lingering leader immediately once the batch reaches `max_batch_size`.
        if st.leader_lingering && st.pending.len() >= self.config.max_batch_size {
            self.leader_cv.notify_one();
        }

        loop {
            if let Some(res) = slot.lock().map_err(|_| queue_poisoned())?.take() {
                return res;
            }

            if !st.leader_active {
                st.leader_active = true;

                // Optional linger to coalesce arriving burst before draining; wakes early on full batch.
                if !self.config.linger.is_zero() && st.pending.len() < self.config.max_batch_size {
                    if let Some(deadline) = Instant::now().checked_add(self.config.linger) {
                        st.leader_lingering = true;
                        while st.pending.len() < self.config.max_batch_size {
                            let now = Instant::now();
                            if now >= deadline {
                                break;
                            }
                            let (guard, timeout) = self
                                .leader_cv
                                .wait_timeout(st, deadline - now)
                                .map_err(|_| queue_poisoned())?;
                            st = guard;
                            if timeout.timed_out() {
                                break;
                            }
                        }
                        st.leader_lingering = false;
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
                struct LeaderGuard<'q, B: BatchTx> {
                    queue: &'q GroupCommitQueue<B>,
                }
                impl<B: BatchTx> Drop for LeaderGuard<'_, B> {
                    fn drop(&mut self) {
                        if let Ok(mut st) = self.queue.state.lock() {
                            st.leader_active = false;
                            st.leader_lingering = false;
                            self.queue.cv.notify_all();
                        }
                    }
                }
                let _leader_guard = LeaderGuard { queue: self };

                execute_batch(drained);

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
pub struct BatchItem<B: BatchTx> {
    run: Option<TaskRunFn<B>>,
    deliver: Option<TaskDeliverFn>,
}

impl<B: BatchTx> fmt::Debug for BatchItem<B> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BatchItem")
            .field("pending_run", &self.run.is_some())
            .field("pending_deliver", &self.deliver.is_some())
            .finish()
    }
}

impl<B: BatchTx> BatchItem<B> {
    /// Executes the queued closure against `tx`.
    pub fn execute(&mut self, tx: &mut B::Tx<'_>) -> Result<BoxedAny> {
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

impl<B: BatchTx> Drop for BatchItem<B> {
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
