//! End-to-end tests for `Database::commit_batch` and concurrent leader-follower
//! `Database::submit_batched` group commit (`X03` / Issue #18).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oryq_core::{
    Database, DatabaseConfig, ErrorCategory, ErrorEnvelope, GroupCommitConfig, PageSize, SortOrder,
    SyncMode, Value,
};
use oryq_engine::{FileHandle, FileIdentity, FileRegistry, OpenMode, OsVfs, StorageIo};

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
            "oryqdb_core_batch_{label}_{}_{seq}_{nanos}",
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

#[test]
fn database_commit_batch_per_item_unique_rollback_and_persistence() {
    let dir = TempDir::new("explicit_batch");
    let db_path = dir.path().join("batch.oryq");
    let cfg = DatabaseConfig {
        page_size: PageSize::Size4096,
        sync_mode: SyncMode::Normal,
        ..DatabaseConfig::default()
    };

    {
        let db = Database::open(&db_path, cfg.clone()).unwrap();
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("users").unwrap();
        {
            let mut col = wtx.collection("users").unwrap();
            col.create_index("email", SortOrder::Asc, true, false, Some("email_uniq"))
                .unwrap();
        }
        wtx.commit().unwrap();

        type Op = Box<dyn FnOnce(&mut oryq_core::WriteTransaction<'_>) -> oryq_core::Result<i64>>;
        let ops: Vec<Op> = vec![
            Box::new(|wtx| {
                let mut col = wtx.collection("users")?;
                col.insert_json(r#"{"_id": 1, "email": "a@example.com"}"#)?;
                Ok(1)
            }),
            Box::new(|wtx| {
                // Duplicate unique email -> must fail and roll back only this item!
                let mut col = wtx.collection("users")?;
                col.insert_json(r#"{"_id": 2, "email": "a@example.com"}"#)?;
                Ok(2)
            }),
            Box::new(|wtx| {
                let mut col = wtx.collection("users")?;
                col.insert_json(r#"{"_id": 3, "email": "b@example.com"}"#)?;
                Ok(3)
            }),
        ];

        let report = db.commit_batch(ops).unwrap();
        assert_eq!(report.committed_count, 2);
        assert_eq!(report.failed_count, 1);
        assert!(report.receipt.is_some());
        assert_eq!(report.results[0].as_ref().copied().unwrap(), 1);
        assert_eq!(
            report.results[1].as_ref().unwrap_err().category(),
            ErrorCategory::DuplicateKey
        );
        assert_eq!(report.results[2].as_ref().copied().unwrap(), 3);
    }

    // Reopen and verify exact durable state
    let db = Database::open(&db_path, cfg).unwrap();
    let rtx = db.begin_read().unwrap();
    let col = rtx.collection("users").unwrap();
    assert_eq!(col.count().unwrap(), 2);
    assert!(col.find_by_id(&Value::Int64(1)).unwrap().is_some());
    assert!(col.find_by_id(&Value::Int64(2)).unwrap().is_none());
    assert!(col.find_by_id(&Value::Int64(3)).unwrap().is_some());
}

#[test]
fn concurrent_submit_batched_coalesces_writers_and_isolates_failures() {
    let dir = TempDir::new("concurrent_group_commit");
    let db_path = dir.path().join("group.oryq");
    let cfg = DatabaseConfig {
        page_size: PageSize::Size8192,
        sync_mode: SyncMode::Normal,
        group_commit: GroupCommitConfig {
            max_batch_size: 32,
            linger: Duration::from_micros(500),
            max_queue_depth: 256,
        },
        ..DatabaseConfig::default()
    };

    let db = Database::open(&db_path, cfg.clone()).unwrap();
    {
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("events").unwrap();
        wtx.commit().unwrap();
    }

    // 8 concurrent threads each submitting 15 writes = 120 writes (+ 1 intentional duplicate per thread)
    const THREADS: usize = 8;
    const OPS_PER_THREAD: usize = 15;
    let mut handles = Vec::with_capacity(THREADS);
    for t in 0..THREADS {
        let db_clone = db.clone();
        handles.push(thread::spawn(move || {
            let mut tx_ids = Vec::with_capacity(OPS_PER_THREAD);
            for i in 0..OPS_PER_THREAD {
                let id = (t * 100 + i + 1) as i64;
                let (returned_id, receipt) = db_clone
                    .submit_batched(move |wtx| {
                        let mut col = wtx.collection("events")?;
                        let json = format!(r#"{{"_id": {id}, "worker": {t}, "seq": {i}}}"#);
                        col.insert_json(&json)?;
                        Ok(id)
                    })
                    .unwrap();
                assert_eq!(returned_id, id);
                tx_ids.push(receipt.tx_id.raw());
            }
            // Submit one duplicate _id that must fail cleanly without affecting other threads
            let dup_id = (t * 100 + 1) as i64;
            let err = db_clone
                .submit_batched(move |wtx| {
                    let mut col = wtx.collection("events")?;
                    let json = format!(r#"{{"_id": {dup_id}, "dup": true}}"#);
                    col.insert_json(&json)?;
                    Ok(())
                })
                .unwrap_err();
            assert_eq!(err.category(), ErrorCategory::DuplicateKey);
            tx_ids
        }));
    }

    let mut all_tx_ids = HashSet::new();
    for h in handles {
        for tx_id in h.join().unwrap() {
            all_tx_ids.insert(tx_id);
        }
    }

    // Verify all 120 valid inserts committed and were coalesced into fewer WAL transactions than 120
    let rtx = db.begin_read().unwrap();
    let col = rtx.collection("events").unwrap();
    assert_eq!(col.count().unwrap(), THREADS * OPS_PER_THREAD);
    drop(rtx);
    assert!(
        all_tx_ids.len() < THREADS * OPS_PER_THREAD,
        "expected group commit coalescing, got {} distinct tx_ids for {} ops",
        all_tx_ids.len(),
        THREADS * OPS_PER_THREAD
    );

    let report = db.integrity_check().unwrap();
    assert!(report.ok, "integrity issues: {:?}", report.issues);
    assert_eq!(report.documents_checked, THREADS * OPS_PER_THREAD);
}

#[derive(Debug)]
struct FaultSyncVfs {
    inner: OsVfs,
    fail_wal_sync: Arc<AtomicBool>,
}

#[derive(Debug)]
struct FaultFileHandle {
    inner: Arc<dyn FileHandle>,
    is_wal: bool,
    fail_wal_sync: Arc<AtomicBool>,
}

impl FileHandle for FaultFileHandle {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> oryq_core::Result<()> {
        self.inner.read_exact_at(buf, offset)
    }
    fn write_all_at(&self, buf: &[u8], offset: u64) -> oryq_core::Result<()> {
        self.inner.write_all_at(buf, offset)
    }
    fn sync_data(&self) -> oryq_core::Result<()> {
        if self.is_wal && self.fail_wal_sync.load(Ordering::SeqCst) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Io,
                "test",
                "FaultFileHandle::sync_data",
                "injected WAL sync failure",
            ));
        }
        self.inner.sync_data()
    }
    fn sync_all(&self) -> oryq_core::Result<()> {
        if self.is_wal && self.fail_wal_sync.load(Ordering::SeqCst) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Io,
                "test",
                "FaultFileHandle::sync_all",
                "injected WAL sync failure",
            ));
        }
        self.inner.sync_all()
    }
    fn len(&self) -> oryq_core::Result<u64> {
        self.inner.len()
    }
    fn set_len(&self, size: u64) -> oryq_core::Result<()> {
        self.inner.set_len(size)
    }
    fn try_lock_exclusive(&self) -> oryq_core::Result<()> {
        self.inner.try_lock_exclusive()
    }
    fn unlock(&self) -> oryq_core::Result<()> {
        self.inner.unlock()
    }
    fn identity(&self) -> &FileIdentity {
        self.inner.identity()
    }
}

impl StorageIo for FaultSyncVfs {
    fn open(&self, path: &Path, mode: OpenMode) -> oryq_core::Result<Arc<dyn FileHandle>> {
        let fh = self.inner.open(path, mode)?;
        let is_wal = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.ends_with("-wal"))
            .unwrap_or(false);
        Ok(Arc::new(FaultFileHandle {
            inner: fh,
            is_wal,
            fail_wal_sync: Arc::clone(&self.fail_wal_sync),
        }))
    }
    fn sync_parent_dir(&self, path: &Path) -> oryq_core::Result<()> {
        self.inner.sync_parent_dir(path)
    }
    fn remove_file(&self, path: &Path) -> oryq_core::Result<()> {
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
fn submit_batched_propagates_commit_outcome_unknown_to_all_followers_and_rejects_readonly() {
    let dir = TempDir::new("fault_and_ro");
    let db_path = dir.path().join("fault.oryq");
    let fail_wal_sync = Arc::new(AtomicBool::new(false));
    let vfs = Arc::new(FaultSyncVfs {
        inner: OsVfs::new(),
        fail_wal_sync: Arc::clone(&fail_wal_sync),
    });

    let cfg = DatabaseConfig {
        page_size: PageSize::Size4096,
        sync_mode: SyncMode::Full,
        group_commit: GroupCommitConfig {
            max_batch_size: 4,
            linger: Duration::from_millis(20),
            max_queue_depth: 64,
        },
        ..DatabaseConfig::default()
    };
    let db = Database::open_with_io(vfs, &db_path, cfg).unwrap();
    {
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("logs").unwrap();
        wtx.commit().unwrap();
    }

    // Inject WAL sync fault and launch 4 concurrent followers into the same group commit
    fail_wal_sync.store(true, Ordering::SeqCst);
    let barrier = Arc::new(Barrier::new(4));
    let mut handles = Vec::new();
    for i in 1..=4i64 {
        let dbc = db.clone();
        let b = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            b.wait();
            dbc.submit_batched(move |wtx| {
                let mut col = wtx.collection("logs")?;
                col.insert_json(&format!(r#"{{"_id": {i}}}"#))?;
                Ok(i)
            })
        }));
    }
    for h in handles {
        let err = h.join().unwrap().unwrap_err();
        assert_eq!(err.category(), ErrorCategory::CommitOutcomeUnknown);
    }

    // Subsequent submit_batched and commit_batch fail closed with CommitOutcomeUnknown
    assert_eq!(
        db.submit_batched(|_| Ok(())).unwrap_err().category(),
        ErrorCategory::CommitOutcomeUnknown
    );
    let empty: Vec<fn(&mut oryq_core::WriteTransaction<'_>) -> oryq_core::Result<()>> = Vec::new();
    assert_eq!(
        db.commit_batch(empty).unwrap_err().category(),
        ErrorCategory::CommitOutcomeUnknown
    );
    drop(db);

    // Read-only Database rejects both commit_batch and submit_batched with InvalidArgument
    let ro_path = dir.path().join("ro.oryq");
    {
        let _w = Database::open(
            &ro_path,
            DatabaseConfig {
                page_size: PageSize::Size4096,
                ..DatabaseConfig::default()
            },
        )
        .unwrap();
    }
    let ro_db = Database::open(
        &ro_path,
        DatabaseConfig {
            page_size: PageSize::Size4096,
            read_only: true,
            ..DatabaseConfig::default()
        },
    )
    .unwrap();
    let empty_ro: Vec<fn(&mut oryq_core::WriteTransaction<'_>) -> oryq_core::Result<()>> =
        Vec::new();
    assert_eq!(
        ro_db.commit_batch(empty_ro).unwrap_err().category(),
        ErrorCategory::InvalidArgument
    );
    assert_eq!(
        ro_db.submit_batched(|_| Ok(())).unwrap_err().category(),
        ErrorCategory::InvalidArgument
    );
}
