//! Regression tests for Issue #46 (v0.14.2):
//! - `ReadTransaction` is `Send + Sync` (concurrent multi-threaded reads on one snapshot)
//! - Single-thread re-entrant `begin_write()` returns `ErrorCategory::Busy` instead of deadlocking
//! - Concurrent `read_only` opens share OS + process-local locks cleanly while blocking writers

use std::sync::Arc;
use std::thread;

use oryq_core::{Database, DatabaseConfig, ErrorCategory, ReadTransaction};

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn read_transaction_is_send_and_sync_across_threads() {
    assert_send_sync::<ReadTransaction<'_>>();

    let db = Database::open_in_memory().unwrap();
    {
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("items").unwrap();
        let mut col = wtx.collection("items").unwrap();
        for i in 0..20 {
            col.insert_json(&format!(r#"{{"_id": {i}, "v": {i}}}"#))
                .unwrap();
        }
        wtx.commit().unwrap();
    }

    let rtx = db.begin_read().unwrap();
    thread::scope(|s| {
        for _ in 0..4 {
            s.spawn(|| {
                let col = rtx.collection("items").unwrap();
                assert_eq!(col.count().unwrap(), 20);
            });
        }
    });
}

#[test]
fn single_thread_reentrant_begin_write_returns_busy_instead_of_deadlocking() {
    let db = Database::open_in_memory().unwrap();
    let _wtx1 = db.begin_write().unwrap();
    let err = db.begin_write().unwrap_err();
    assert_eq!(err.category(), ErrorCategory::Busy);
}

#[test]
fn concurrent_readonly_opens_share_locks_and_block_exclusive_writer() {
    let dir = std::env::temp_dir().join(format!(
        "oryq_shared_ro_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("shared.oryq");

    {
        let db = Database::open(&db_path, DatabaseConfig::default()).unwrap();
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("docs").unwrap();
        wtx.collection("docs")
            .unwrap()
            .insert_json(r#"{"_id": 1, "ok": true}"#)
            .unwrap();
        wtx.commit().unwrap();
    }

    let ro_cfg = DatabaseConfig {
        read_only: true,
        create_if_missing: false,
        auto_checkpoint_on_close: false,
        ..DatabaseConfig::default()
    };

    // Two concurrent read-only handles on the same physical file must succeed!
    let ro1 = Arc::new(Database::open(&db_path, ro_cfg.clone()).unwrap());
    let ro2 = Arc::new(Database::open(&db_path, ro_cfg).unwrap());
    assert_eq!(
        ro1.begin_read()
            .unwrap()
            .collection("docs")
            .unwrap()
            .count()
            .unwrap(),
        1
    );
    assert_eq!(
        ro2.begin_read()
            .unwrap()
            .collection("docs")
            .unwrap()
            .count()
            .unwrap(),
        1
    );

    // An exclusive read-write open while shared readers are active must fail with LockedByProcess.
    let rw_err = Database::open(&db_path, DatabaseConfig::default()).unwrap_err();
    assert_eq!(rw_err.category(), ErrorCategory::LockedByProcess);

    drop(ro1);
    drop(ro2);

    // Once all shared readers close, exclusive read-write open succeeds again.
    let rw = Database::open(&db_path, DatabaseConfig::default()).unwrap();
    assert_eq!(
        rw.begin_read()
            .unwrap()
            .collection("docs")
            .unwrap()
            .count()
            .unwrap(),
        1
    );
    drop(rw);
    let _ = std::fs::remove_dir_all(&dir);
}
