//! Tests for `oryq_engine::commit_batch`: ordered batch admission, single-fsync WAL commit,
//! per-item savepoint rollback, all-failed no-op rollback, and `CommitOutcomeUnknown` sync fault
//! poisoning (`X03` / Issue #18).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_engine::{
    commit_batch, BTree, EngineConfig, FileHandle, FileIdentity, FileRegistry, OpenMode, OsVfs,
    PageSize, StorageEngine, StorageIo, SyncMode, TreeKind,
};
use oryq_types::{ErrorCategory, ErrorEnvelope, Result, TxId};

static DIR_SEQ: AtomicU64 = AtomicU64::new(1);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let seq = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "oryqdb_batch_{label}_{}_{seq}_{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Counting & fault-injecting VFS wrapper verifying exact fsync counts and `CommitOutcomeUnknown`.
#[derive(Debug)]
struct CountingSyncVfs {
    inner: OsVfs,
    wal_syncs: Arc<AtomicU64>,
    fail_wal_sync: Arc<AtomicBool>,
}

#[derive(Debug)]
struct CountingFileHandle {
    inner: Arc<dyn FileHandle>,
    is_wal: bool,
    wal_syncs: Arc<AtomicU64>,
    fail_wal_sync: Arc<AtomicBool>,
}

impl FileHandle for CountingFileHandle {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> Result<()> {
        self.inner.read_exact_at(buf, offset)
    }
    fn write_all_at(&self, buf: &[u8], offset: u64) -> Result<()> {
        self.inner.write_all_at(buf, offset)
    }
    fn sync_data(&self) -> Result<()> {
        if self.is_wal {
            self.wal_syncs.fetch_add(1, Ordering::SeqCst);
            if self.fail_wal_sync.load(Ordering::SeqCst) {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Io,
                    "test",
                    "CountingFileHandle::sync_data",
                    "injected WAL sync fault",
                ));
            }
        }
        self.inner.sync_data()
    }
    fn sync_all(&self) -> Result<()> {
        if self.is_wal {
            self.wal_syncs.fetch_add(1, Ordering::SeqCst);
            if self.fail_wal_sync.load(Ordering::SeqCst) {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Io,
                    "test",
                    "CountingFileHandle::sync_all",
                    "injected WAL sync fault",
                ));
            }
        }
        self.inner.sync_all()
    }
    fn len(&self) -> Result<u64> {
        self.inner.len()
    }
    fn set_len(&self, size: u64) -> Result<()> {
        self.inner.set_len(size)
    }
    fn try_lock_exclusive(&self) -> Result<()> {
        self.inner.try_lock_exclusive()
    }
    fn unlock(&self) -> Result<()> {
        self.inner.unlock()
    }
    fn identity(&self) -> &FileIdentity {
        self.inner.identity()
    }
}

impl StorageIo for CountingSyncVfs {
    fn open(&self, path: &Path, mode: OpenMode) -> Result<Arc<dyn FileHandle>> {
        let fh = self.inner.open(path, mode)?;
        let is_wal = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with("-wal"))
            .unwrap_or(false);
        Ok(Arc::new(CountingFileHandle {
            inner: fh,
            is_wal,
            wal_syncs: Arc::clone(&self.wal_syncs),
            fail_wal_sync: Arc::clone(&self.fail_wal_sync),
        }))
    }
    fn sync_parent_dir(&self, path: &Path) -> Result<()> {
        self.inner.sync_parent_dir(path)
    }
    fn remove_file(&self, path: &Path) -> Result<()> {
        self.inner.remove_file(path)
    }
    fn exists(&self, path: &Path) -> bool {
        self.inner.exists(path)
    }
    fn file_registry(&self) -> Arc<FileRegistry> {
        self.inner.file_registry()
    }
}

#[test]
fn engine_commit_batch_single_fsync_per_item_savepoint_and_sync_fault_poisoning() {
    let dir = TempDir::new("engine_batch");
    let db_path = dir.path().join("batch.oryq");
    let wal_syncs = Arc::new(AtomicU64::new(0));
    let fail_wal_sync = Arc::new(AtomicBool::new(false));
    let vfs = Arc::new(CountingSyncVfs {
        inner: OsVfs::new(),
        wal_syncs: Arc::clone(&wal_syncs),
        fail_wal_sync: Arc::clone(&fail_wal_sync),
    });

    let engine = StorageEngine::open(
        vfs,
        &db_path,
        EngineConfig {
            page_size: PageSize::Size4096,
            sync_mode: SyncMode::Full,
            ..EngineConfig::default()
        },
        [0x77; 16],
    )
    .unwrap();

    // Unseal with 1 initial commit so subsequent batch measures pure commit fsync count
    {
        let mut wtx = engine.begin_write().unwrap();
        let mut tree = BTree::open(wtx.catalog_root_page_id(), TreeKind::Collection);
        tree.insert(&mut wtx, b"k0", b"v0").unwrap();
        wtx.set_catalog_root_page_id(tree.root());
        wtx.commit().unwrap();
    }

    // 1. Ordered batch of 5 items where item #2 fails validation after writing a key:
    //    only item #2 rolls back; items 0,1,3,4 commit in ONE single WAL fsync!
    let syncs_before = wal_syncs.load(Ordering::SeqCst);
    type BatchOp = Box<dyn FnOnce(&mut oryq_engine::WriteTxn<'_>) -> Result<u32>>;
    let ops: Vec<BatchOp> = vec![
        Box::new(|wtx| {
            let mut tree = BTree::open(wtx.catalog_root_page_id(), TreeKind::Collection);
            tree.insert(wtx, b"k1", b"v1")?;
            wtx.set_catalog_root_page_id(tree.root());
            Ok(1)
        }),
        Box::new(|wtx| {
            let mut tree = BTree::open(wtx.catalog_root_page_id(), TreeKind::Collection);
            tree.insert(wtx, b"k2", b"v2")?;
            wtx.set_catalog_root_page_id(tree.root());
            Ok(2)
        }),
        Box::new(|wtx| {
            // Mutate then fail -> savepoint must roll back `k_bad`!
            let mut tree = BTree::open(wtx.catalog_root_page_id(), TreeKind::Collection);
            tree.insert(wtx, b"k_bad", b"must_rollback")?;
            wtx.set_catalog_root_page_id(tree.root());
            Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                "test",
                "bad_item",
                "validation failure",
            ))
        }),
        Box::new(|wtx| {
            let mut tree = BTree::open(wtx.catalog_root_page_id(), TreeKind::Collection);
            tree.insert(wtx, b"k3", b"v3")?;
            wtx.set_catalog_root_page_id(tree.root());
            Ok(3)
        }),
    ];

    let report = commit_batch(&engine, ops).unwrap();
    let syncs_after = wal_syncs.load(Ordering::SeqCst);
    assert_eq!(
        syncs_after - syncs_before,
        1,
        "entire batch must use 1 fsync"
    );
    assert_eq!(report.committed_count, 3);
    assert_eq!(report.failed_count, 1);
    assert_eq!(report.receipt.unwrap().tx_id, TxId::new(2));
    assert_eq!(report.results[0].as_ref().copied().unwrap(), 1);
    assert_eq!(report.results[1].as_ref().copied().unwrap(), 2);
    assert_eq!(
        report.results[2].as_ref().unwrap_err().category(),
        ErrorCategory::InvalidArgument
    );
    assert_eq!(report.results[3].as_ref().copied().unwrap(), 3);

    // Verify committed keys exist and rolled-back key does NOT exist
    {
        let rtx = engine.begin_read().unwrap();
        let tree = BTree::open(rtx.catalog_root_page_id(), TreeKind::Collection);
        assert_eq!(
            tree.get(&rtx, b"k1").unwrap().as_deref(),
            Some(b"v1".as_slice())
        );
        assert_eq!(
            tree.get(&rtx, b"k2").unwrap().as_deref(),
            Some(b"v2".as_slice())
        );
        assert_eq!(tree.get(&rtx, b"k_bad").unwrap(), None);
        assert_eq!(
            tree.get(&rtx, b"k3").unwrap().as_deref(),
            Some(b"v3".as_slice())
        );
    }

    // 2. All-failed batch performs ZERO WAL fsyncs and returns receipt: None
    let syncs_before_fail = wal_syncs.load(Ordering::SeqCst);
    let all_fail: Vec<BatchOp> = vec![Box::new(|_| {
        Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            "test",
            "fail_only",
            "nope",
        ))
    })];
    let fail_report = engine.commit_batch(all_fail).unwrap();
    assert_eq!(wal_syncs.load(Ordering::SeqCst), syncs_before_fail);
    assert!(fail_report.receipt.is_none());
    assert_eq!(fail_report.committed_count, 0);
    assert_eq!(fail_report.failed_count, 1);

    // 3. Injected WAL sync fault -> returns CommitOutcomeUnknown and poisons engine
    fail_wal_sync.store(true, Ordering::SeqCst);
    let fault_ops: Vec<BatchOp> = vec![Box::new(|wtx| {
        let mut tree = BTree::open(wtx.catalog_root_page_id(), TreeKind::Collection);
        tree.insert(wtx, b"k_uncertain", b"v")?;
        wtx.set_catalog_root_page_id(tree.root());
        Ok(99)
    })];
    let err = engine.commit_batch(fault_ops).unwrap_err();
    assert_eq!(err.category(), ErrorCategory::CommitOutcomeUnknown);
    assert!(engine.is_poisoned());
    // Subsequent batch on poisoned engine fails closed immediately
    let empty: Vec<BatchOp> = Vec::new();
    assert_eq!(
        engine.commit_batch(empty).unwrap_err().category(),
        ErrorCategory::CommitOutcomeUnknown
    );
}
