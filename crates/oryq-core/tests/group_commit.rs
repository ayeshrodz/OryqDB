//! End-to-end tests for `Database::commit_batch` and concurrent leader-follower
//! `Database::submit_batched` group commit (`X03` / Issue #18).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use oryq_core::{
    Database, DatabaseConfig, ErrorCategory, GroupCommitConfig, PageSize, SortOrder, SyncMode,
    Value,
};

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
