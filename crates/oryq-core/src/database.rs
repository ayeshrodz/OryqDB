//! Shareable [`Database`] handle coordinating `oryq-engine` and immutable `CoreRegistries`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_engine::{
    CheckpointOutcome, LockInspectionReport, OsVfs, PageInspectionReport, PageSize, StorageEngine,
    StorageIo, WalInspectionReport,
};
use oryq_types::{Result, TxId};

use crate::admin::{
    inspect_locks, inspect_page, inspect_wal, run_integrity_check, run_online_backup, BackupReport,
    IntegrityReport,
};
use crate::api::{ReadTransaction, WriteTransaction};
use crate::composition::CoreRegistries;
use crate::config::DatabaseConfig;

#[derive(Debug)]
struct DatabaseInner {
    path: PathBuf,
    page_size: PageSize,
    engine: StorageEngine,
    registries: CoreRegistries,
    max_document_bytes: usize,
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
        Ok(Self {
            inner: Arc::new(DatabaseInner {
                path: path.as_ref().to_path_buf(),
                page_size: config.page_size,
                engine,
                registries,
                max_document_bytes: config.max_document_bytes,
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
