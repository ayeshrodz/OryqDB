//! Shareable [`Database`] handle coordinating `oryq-engine` and immutable `CoreRegistries`.

use std::any::Any;
use std::collections::VecDeque;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use oryq_engine::{
    clone_error_envelope, BatchCommitReport, CheckpointOutcome, CommitReceipt, GroupCommitConfig,
    LockInspectionReport, OsVfs, PageInspectionReport, PageSize, StorageEngine, StorageIo,
    WalInspectionReport,
};
use oryq_types::{ErrorCategory, ErrorEnvelope, Result, TxId};

use crate::admin::{
    inspect_locks, inspect_page, inspect_wal, run_integrity_check, run_online_backup, BackupReport,
    IntegrityReport,
};
use crate::api::{ReadTransaction, WriteTransaction};
use crate::composition::CoreRegistries;
use crate::config::DatabaseConfig;

const MODULE_ID: &str = "core";

type BoxedAny = Box<dyn Any + Send>;
type ResultSlot<T> = Arc<Mutex<Option<Result<(T, CommitReceipt)>>>>;
type CoreBatchRunFn =
    Box<dyn for<'db> FnOnce(&mut WriteTransaction<'db>) -> Result<BoxedAny> + Send>;
type CoreBatchDeliverFn = Box<dyn FnOnce(Result<(BoxedAny, CommitReceipt)>) + Send>;

struct CoreQueuedTask {
    run: Option<CoreBatchRunFn>,
    deliver: Option<CoreBatchDeliverFn>,
}

impl CoreQueuedTask {
    fn execute(&mut self, wtx: &mut WriteTransaction<'_>) -> Result<BoxedAny> {
        let f = self.run.take().ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Internal,
                MODULE_ID,
                "Database::submit_batched",
                "queued batch task already executed",
            )
        })?;
        f(wtx)
    }

    fn deliver_ok(mut self, val: BoxedAny, receipt: CommitReceipt) {
        if let Some(d) = self.deliver.take() {
            d(Ok((val, receipt)));
        }
    }

    fn deliver_err(mut self, err: ErrorEnvelope) {
        if let Some(d) = self.deliver.take() {
            d(Err(err));
        }
    }
}

impl Drop for CoreQueuedTask {
    fn drop(&mut self) {
        if let Some(d) = self.deliver.take() {
            d(Err(ErrorEnvelope::new(
                ErrorCategory::Internal,
                MODULE_ID,
                "Database::submit_batched",
                "group commit leader aborted before delivering batch item outcome",
            )));
        }
    }
}

struct CoreBatchQueueState {
    pending: VecDeque<CoreQueuedTask>,
    leader_active: bool,
}

struct CoreBatchQueue {
    config: GroupCommitConfig,
    state: Mutex<CoreBatchQueueState>,
    cv: Condvar,
}

impl fmt::Debug for CoreBatchQueue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CoreBatchQueue")
            .field("config", &self.config)
            .finish()
    }
}

impl CoreBatchQueue {
    fn new(config: GroupCommitConfig) -> Self {
        Self {
            config: GroupCommitConfig {
                max_batch_size: config.max_batch_size.max(1),
                linger: config.linger,
                max_queue_depth: config.max_queue_depth.max(1),
            },
            state: Mutex::new(CoreBatchQueueState {
                pending: VecDeque::new(),
                leader_active: false,
            }),
            cv: Condvar::new(),
        }
    }
}

#[derive(Debug)]
struct DatabaseInner {
    path: PathBuf,
    page_size: PageSize,
    engine: StorageEngine,
    registries: CoreRegistries,
    max_document_bytes: usize,
    batch_queue: CoreBatchQueue,
}

/// Shareable (`Clone + Send + Sync`) embedded OryqDB database handle.
#[derive(Debug, Clone)]
pub struct Database {
    inner: Arc<DatabaseInner>,
}

impl Database {
    /// Opens or creates an OryqDB database at `path` using the default OS VFS.
    pub fn open(path: impl AsRef<Path>, config: DatabaseConfig) -> Result<Self> {
        let vfs = Arc::new(OsVfs::new());
        Self::open_with_io(vfs, path, config)
    }

    /// Opens or creates an OryqDB database using a caller-supplied [`StorageIo`] VFS.
    pub fn open_with_io(
        io: Arc<dyn StorageIo>,
        path: impl AsRef<Path>,
        config: DatabaseConfig,
    ) -> Result<Self> {
        let uuid = generate_database_uuid();
        let engine = StorageEngine::open(io, path.as_ref(), config.to_engine_config(), uuid)?;
        let registries = CoreRegistries::default_v1();
        let batch_queue = CoreBatchQueue::new(config.group_commit);
        Ok(Self {
            inner: Arc::new(DatabaseInner {
                path: path.as_ref().to_path_buf(),
                page_size: config.page_size,
                engine,
                registries,
                max_document_bytes: config.max_document_bytes,
                batch_queue,
            }),
        })
    }

    pub fn begin_read(&self) -> Result<ReadTransaction<'_>> {
        let tx = self.inner.engine.begin_read()?;
        Ok(ReadTransaction::new(tx))
    }

    pub fn begin_write(&self) -> Result<WriteTransaction<'_>> {
        let tx = self.inner.engine.begin_write()?;
        Ok(WriteTransaction::new(
            tx,
            &self.inner.registries,
            self.inner.max_document_bytes,
        ))
    }

    pub fn try_begin_write(&self) -> Result<WriteTransaction<'_>> {
        let tx = self.inner.engine.try_begin_write()?;
        Ok(WriteTransaction::new(
            tx,
            &self.inner.registries,
            self.inner.max_document_bytes,
        ))
    }

    /// Executes an ordered batch of write closures inside a single [`WriteTransaction`] and a
    /// single durable WAL fsync (`X03`).
    ///
    /// - Each item executes inside its own statement savepoint so validation failures roll back
    ///   only that item while all valid items commit together in one fsync.
    /// - If zero items succeed, the transaction is rolled back with zero WAL writes/fsyncs.
    /// - If the single WAL write/sync fails (including [`ErrorCategory::CommitOutcomeUnknown`]),
    ///   `Err(commit_err)` is returned and no item is reported as durably committed.
    pub fn commit_batch<T, F>(&self, items: Vec<F>) -> Result<BatchCommitReport<T>>
    where
        F: FnOnce(&mut WriteTransaction<'_>) -> Result<T>,
    {
        if self.inner.engine.is_read_only() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "Database::commit_batch",
                "cannot execute commit_batch on a read-only Database",
            ));
        }
        if self.inner.engine.is_poisoned() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::CommitOutcomeUnknown,
                MODULE_ID,
                "Database::commit_batch",
                "database engine is poisoned after an uncertain commit write/sync failure; close and reopen to recover",
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

        let mut wtx = self.begin_write()?;
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

    /// Submits a write operation to the database's leader-follower group-commit queue (`X03`).
    ///
    /// Concurrent callers are coalesced into ordered single-fsync batches (bounded by
    /// `DatabaseConfig::group_commit`) while preserving per-operation savepoint rollback and
    /// `CommitOutcomeUnknown` propagation on sync failure.
    pub fn submit_batched<T, F>(&self, op: F) -> Result<(T, CommitReceipt)>
    where
        T: Send + 'static,
        F: for<'db> FnOnce(&mut WriteTransaction<'db>) -> Result<T> + Send + 'static,
    {
        if self.inner.engine.is_read_only() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "Database::submit_batched",
                "cannot submit batched write on a read-only Database",
            ));
        }
        if self.inner.engine.is_poisoned() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::CommitOutcomeUnknown,
                MODULE_ID,
                "Database::submit_batched",
                "database engine is poisoned after an uncertain commit write/sync failure; close and reopen to recover",
            ));
        }

        let slot: ResultSlot<T> = Arc::new(Mutex::new(None));
        let slot_for_deliver = Arc::clone(&slot);

        let task = CoreQueuedTask {
            run: Some(Box::new(
                move |wtx: &mut WriteTransaction<'_>| -> Result<BoxedAny> {
                    let val = op(wtx)?;
                    Ok(Box::new(val))
                },
            )),
            deliver: Some(Box::new(move |res: Result<(BoxedAny, CommitReceipt)>| {
                let typed = res.and_then(|(boxed, receipt)| {
                    boxed.downcast::<T>().map(|v| (*v, receipt)).map_err(|_| {
                        ErrorEnvelope::new(
                            ErrorCategory::Internal,
                            MODULE_ID,
                            "Database::submit_batched",
                            "group commit result type downcast mismatch",
                        )
                    })
                });
                if let Ok(mut guard) = slot_for_deliver.lock() {
                    *guard = Some(typed);
                }
            })),
        };

        let q = &self.inner.batch_queue;
        let mut st = q.state.lock().map_err(|_| batch_queue_poisoned())?;
        if st.pending.len() >= q.config.max_queue_depth {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Busy,
                MODULE_ID,
                "Database::submit_batched",
                format!(
                    "group commit queue full (max_queue_depth={})",
                    q.config.max_queue_depth
                ),
            ));
        }
        st.pending.push_back(task);

        loop {
            if let Some(res) = slot.lock().map_err(|_| batch_queue_poisoned())?.take() {
                return res;
            }

            if !st.leader_active {
                st.leader_active = true;

                if !q.config.linger.is_zero() && st.pending.len() < q.config.max_batch_size {
                    let deadline = Instant::now() + q.config.linger;
                    while st.pending.len() < q.config.max_batch_size {
                        let now = Instant::now();
                        if now >= deadline {
                            break;
                        }
                        let (guard, timeout) =
                            q.cv.wait_timeout(st, deadline - now)
                                .map_err(|_| batch_queue_poisoned())?;
                        st = guard;
                        if timeout.timed_out() {
                            break;
                        }
                    }
                }

                let drain_count = st.pending.len().min(q.config.max_batch_size);
                let mut drained = Vec::with_capacity(drain_count);
                for _ in 0..drain_count {
                    if let Some(t) = st.pending.pop_front() {
                        drained.push(t);
                    }
                }
                drop(st);

                struct LeaderGuard<'q>(&'q CoreBatchQueue);
                impl Drop for LeaderGuard<'_> {
                    fn drop(&mut self) {
                        if let Ok(mut st) = self.0.state.lock() {
                            st.leader_active = false;
                            self.0.cv.notify_all();
                        }
                    }
                }
                let _leader_guard = LeaderGuard(q);

                self.execute_drained_batch(drained);

                drop(_leader_guard);
                st = q.state.lock().map_err(|_| batch_queue_poisoned())?;
                continue;
            }

            st = q.cv.wait(st).map_err(|_| batch_queue_poisoned())?;
        }
    }

    fn execute_drained_batch(&self, drained: Vec<CoreQueuedTask>) {
        if drained.is_empty() {
            return;
        }
        let mut wtx = match self.begin_write() {
            Ok(tx) => tx,
            Err(err) => {
                for task in drained {
                    task.deliver_err(clone_error_envelope(&err));
                }
                return;
            }
        };

        let mut succeeded: Vec<(CoreQueuedTask, BoxedAny)> = Vec::with_capacity(drained.len());
        for mut task in drained {
            let sp = wtx.savepoint();
            match task.execute(&mut wtx) {
                Ok(val) => match wtx.release_savepoint(sp) {
                    Ok(()) => succeeded.push((task, val)),
                    Err(err) => task.deliver_err(err),
                },
                Err(err) => {
                    let _ = wtx.rollback_to(sp);
                    task.deliver_err(err);
                }
            }
        }

        if succeeded.is_empty() {
            wtx.rollback();
            return;
        }

        match wtx.commit() {
            Ok(receipt) => {
                for (task, val) in succeeded {
                    task.deliver_ok(val, receipt);
                }
            }
            Err(commit_err) => {
                for (task, _) in succeeded {
                    task.deliver_err(clone_error_envelope(&commit_err));
                }
            }
        }
    }

    pub fn checkpoint(&self, allow_reset: bool) -> Result<CheckpointOutcome> {
        self.inner.engine.checkpoint(allow_reset)
    }

    pub fn is_sealed(&self) -> Result<bool> {
        self.inner.engine.is_sealed()
    }

    pub fn latest_committed_tx_id(&self) -> Result<TxId> {
        self.inner.engine.latest_committed_tx_id()
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// Verifies snapshot-consistent collection & index integrity across the database.
    pub fn integrity_check(&self) -> Result<IntegrityReport> {
        run_integrity_check(self)
    }

    /// Creates a standalone, sealed online backup of this database at `dest`.
    pub fn backup_to(&self, dest: impl AsRef<Path>) -> Result<BackupReport> {
        run_online_backup(self, &self.inner.path, self.inner.page_size, dest.as_ref())
    }

    /// Read-only forensic inspection of a single physical page without mutating or recovering.
    pub fn inspect_page(path: impl AsRef<Path>, page_id: u64) -> Result<PageInspectionReport> {
        inspect_page(path.as_ref(), page_id)
    }

    /// Read-only forensic inspection of the companion `.wal` file without mutating or replaying.
    pub fn inspect_wal(path: impl AsRef<Path>) -> Result<WalInspectionReport> {
        inspect_wal(path.as_ref())
    }

    /// Non-mutating OS exclusive-lock probe for `path`.
    pub fn inspect_locks(path: impl AsRef<Path>) -> Result<LockInspectionReport> {
        inspect_locks(path.as_ref())
    }
}

fn batch_queue_poisoned() -> ErrorEnvelope {
    ErrorEnvelope::new(
        ErrorCategory::Internal,
        MODULE_ID,
        "Database::submit_batched",
        "group commit queue mutex poisoned",
    )
}

fn generate_database_uuid() -> [u8; 16] {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut uuid = nanos.to_be_bytes();
    let pid = std::process::id().to_be_bytes();
    uuid[0] ^= pid[0];
    uuid[1] ^= pid[1];
    uuid[2] ^= pid[2];
    uuid[3] ^= pid[3];
    // Mark RFC 4122 version 4 / variant bits.
    uuid[6] = (uuid[6] & 0x0F) | 0x40;
    uuid[8] = (uuid[8] & 0x3F) | 0x80;
    uuid
}
