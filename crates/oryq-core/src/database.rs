//! Shareable [`Database`] handle coordinating `oryq-engine` and immutable `CoreRegistries`.

use std::any::Any;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_engine::{
    clone_error_envelope, BatchCommitReport, BatchItem, BatchTx, CheckpointOutcome, CommitReceipt,
    GroupCommitQueue, LockInspectionReport, MemoryVfs, OsVfs, PageInspectionReport, PageSize,
    StorageEngine, StorageIo, WalInspectionReport,
};
use oryq_types::{ErrorCategory, ErrorEnvelope, Result, TxId};

use crate::admin::{
    inspect_locks, inspect_page, inspect_wal, run_integrity_check, run_online_backup, run_vacuum,
    BackupReport, IntegrityReport, VacuumReport,
};
use crate::api::{ReadTransaction, WriteTransaction};
use crate::composition::CoreRegistries;
use crate::config::DatabaseConfig;

const MODULE_ID: &str = "core";

#[derive(Debug, Default, Clone, Copy)]
struct CoreWriteBatchTx;

impl BatchTx for CoreWriteBatchTx {
    type Tx<'a> = WriteTransaction<'a>;
}

#[derive(Debug)]
struct DatabaseInner {
    path: PathBuf,
    in_memory: bool,
    page_size: PageSize,
    engine: StorageEngine,
    registries: CoreRegistries,
    max_document_bytes: usize,
    batch_queue: GroupCommitQueue<CoreWriteBatchTx>,
}

/// Shareable (`Clone + Send + Sync`) embedded OryqDB database handle.
#[derive(Debug, Clone)]
pub struct Database {
    inner: Arc<DatabaseInner>,
}

impl Database {
    /// Special path sentinel that opens an isolated, zero-disk in-memory database.
    pub const IN_MEMORY_PATH: &'static str = ":memory:";

    /// Opens or creates an OryqDB database at `path` using the default OS VFS.
    ///
    /// Passing `":memory:"` ([`Database::IN_MEMORY_PATH`]) opens a fresh, isolated,
    /// zero-disk in-memory database backed by [`oryq_engine::MemoryVfs`].
    pub fn open(path: impl AsRef<Path>, config: DatabaseConfig) -> Result<Self> {
        let path_ref = path.as_ref();
        if path_ref == Path::new(Self::IN_MEMORY_PATH) {
            return Self::open_in_memory_with_config(config);
        }
        let vfs = Arc::new(OsVfs::new());
        Self::open_with_io_internal(vfs, path_ref, config, false)
    }

    /// Opens a fresh, isolated, zero-disk in-memory database with default configuration.
    pub fn open_in_memory() -> Result<Self> {
        Self::open_in_memory_with_config(DatabaseConfig::default())
    }

    /// Opens a fresh, isolated, zero-disk in-memory database with custom configuration.
    pub fn open_in_memory_with_config(config: DatabaseConfig) -> Result<Self> {
        let vfs = Arc::new(MemoryVfs::new());
        Self::open_with_io_internal(vfs, Path::new(Self::IN_MEMORY_PATH), config, true)
    }

    /// Opens or creates an OryqDB database using a caller-supplied [`StorageIo`] VFS.
    pub fn open_with_io(
        io: Arc<dyn StorageIo>,
        path: impl AsRef<Path>,
        config: DatabaseConfig,
    ) -> Result<Self> {
        let path_ref = path.as_ref();
        let in_memory = io.is_in_memory() || path_ref == Path::new(Self::IN_MEMORY_PATH);
        Self::open_with_io_internal(io, path_ref, config, in_memory)
    }

    fn open_with_io_internal(
        io: Arc<dyn StorageIo>,
        path: &Path,
        config: DatabaseConfig,
        in_memory: bool,
    ) -> Result<Self> {
        let uuid = generate_database_uuid();
        let engine = StorageEngine::open(io, path, config.to_engine_config(), uuid)?;
        let registries = CoreRegistries::default_v1();
        let batch_queue = GroupCommitQueue::new(config.group_commit);
        Ok(Self {
            inner: Arc::new(DatabaseInner {
                path: path.to_path_buf(),
                in_memory,
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

        self.inner
            .batch_queue
            .submit(op, |drained| self.execute_drained_batch(drained))
    }

    fn execute_drained_batch(&self, drained: Vec<BatchItem<CoreWriteBatchTx>>) {
        if drained.is_empty() {
            return;
        }
        let mut wtx = match self.begin_write() {
            Ok(tx) => tx,
            Err(err) => {
                for item in drained {
                    item.deliver_err(clone_error_envelope(&err));
                }
                return;
            }
        };

        let mut succeeded: Vec<(BatchItem<CoreWriteBatchTx>, Box<dyn Any + Send>)> =
            Vec::with_capacity(drained.len());
        for mut item in drained {
            let sp = wtx.savepoint();
            match item.execute(&mut wtx) {
                Ok(val) => match wtx.release_savepoint(sp) {
                    Ok(()) => succeeded.push((item, val)),
                    Err(err) => {
                        let _ = wtx.rollback_to(sp);
                        item.deliver_err(err);
                    }
                },
                Err(err) => {
                    let _ = wtx.rollback_to(sp);
                    item.deliver_err(err);
                }
            }
        }

        if succeeded.is_empty() {
            wtx.rollback();
            return;
        }

        match wtx.commit() {
            Ok(receipt) => {
                for (item, val) in succeeded {
                    item.deliver_ok(val, receipt);
                }
            }
            Err(commit_err) => {
                for (item, _) in succeeded {
                    item.deliver_err(clone_error_envelope(&commit_err));
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

    /// Returns `true` if this database is backed by an ephemeral in-memory VFS (`:memory:`).
    pub fn is_in_memory(&self) -> bool {
        self.inner.in_memory
    }

    /// Verifies snapshot-consistent collection & index integrity across the database.
    pub fn integrity_check(&self) -> Result<IntegrityReport> {
        run_integrity_check(self)
    }

    /// Creates a standalone, sealed online backup of this database at `dest`.
    pub fn backup_to(&self, dest: impl AsRef<Path>) -> Result<BackupReport> {
        run_online_backup(
            self,
            &self.inner.path,
            self.inner.in_memory,
            self.inner.page_size,
            dest.as_ref(),
        )
    }

    /// Compacts this database in place (`VACUUM`), reclaiming free space, shrinking the `.oryq`
    /// file on disk, and leaving a single sealed `.oryq` file without a `.oryq-wal` sidecar.
    pub fn vacuum(&self) -> Result<VacuumReport> {
        run_vacuum(
            self,
            &self.inner.engine,
            &self.inner.path,
            self.inner.in_memory,
            self.inner.page_size,
        )
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
