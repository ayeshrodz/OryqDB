//! End-to-end multi-threaded concurrency, snapshot atomicity, delta-compaction,
//! uncheckpointed WAL replay, deletes, savepoint rollback storm, and integrity verification
//! stress test for `oryq-core`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_core::{Database, DatabaseConfig, ErrorCategory, PageSize, SortOrder, Value};

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
            "oryqdb_core_stress_{label}_{}_{seq}_{nanos}",
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
fn concurrent_readers_writers_delta_compaction_and_savepoint_rollback_storm() {
    let dir = TempDir::new("concurrency");
    let db_path = dir.path().join("stress.oryq");
    let cfg = DatabaseConfig {
        page_size: PageSize::Size4096,
        ..DatabaseConfig::default()
    };

    let db = Arc::new(Database::open(&db_path, cfg.clone()).unwrap());
    {
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("accounts").unwrap();
        let mut col = wtx.collection("accounts").unwrap();
        col.create_index("email", SortOrder::Asc, true, false, Some("email_uniq"))
            .unwrap();
        col.create_index("balance", SortOrder::Asc, false, false, Some("bal_1"))
            .unwrap();
        for i in 1..=20i64 {
            col.insert_json(&format!(
                r#"{{"_id": {i}, "email": "u{i}@oryq.dev", "balance": 100}}"#
            ))
            .unwrap();
        }
        wtx.commit().unwrap();
    }

    let barrier = Arc::new(Barrier::new(5));
    let mut readers = Vec::new();
    for _ in 0..4 {
        let db_r = Arc::clone(&db);
        let bar = Arc::clone(&barrier);
        readers.push(thread::spawn(move || {
            bar.wait();
            for _ in 0..25 {
                let rtx = db_r.begin_read().unwrap();
                let col = rtx.collection("accounts").unwrap();
                let docs = col
                    .find_json(Some(r#"{"balance": {"$gte": 100}}"#), None, None, 0, None)
                    .unwrap()
                    .collect_all();
                assert_eq!(docs.len(), 20);

                // Snapshot atomicity invariant: every account in one ReadTransaction must
                // observe the EXACT same balance (no torn cross-transaction visibility).
                let expected_bal = docs[0]
                    .view()
                    .get_field("balance")
                    .unwrap()
                    .to_owned_value();
                for d in &docs {
                    assert_eq!(
                        d.view().get_field("balance").unwrap().to_owned_value(),
                        expected_bal,
                        "torn snapshot read detected across accounts"
                    );
                }

                let agg = col
                    .aggregate_json(r#"[{"$group": {"_id": null, "cnt": {"$count": {}}}}]"#)
                    .unwrap();
                assert_eq!(
                    agg[0].view().get_field("cnt").unwrap().to_owned_value(),
                    Value::Int64(20)
                );
            }
        }));
    }

    // Writer executes 6 rounds of partial updates (triggering 4-delta chain + 5th-patch compaction)
    // interleaved with intentional unique-key violations that must roll back cleanly via savepoints.
    barrier.wait();
    for round in 1..=6i64 {
        let mut wtx = db.begin_write().unwrap();
        {
            let mut col = wtx.collection("accounts").unwrap();
            for i in 1..=20i64 {
                assert!(col
                    .update_by_id(&Value::Int64(i), r#"{"$inc": {"balance": 10}}"#)
                    .unwrap());
            }
            // Intentional duplicate unique email violation — statement savepoint must roll back!
            let dup_err = col
                .insert_json(&format!(
                    r#"{{"_id": {}, "email": "u1@oryq.dev", "balance": 999}}"#,
                    1000 + round
                ))
                .unwrap_err();
            assert_eq!(dup_err.category(), ErrorCategory::DuplicateKey);
        }
        wtx.commit().unwrap();
        if round == 2 || round == 4 {
            let _ = db.checkpoint(true).unwrap();
        }
    }

    for h in readers {
        h.join().unwrap();
    }

    // Round 7 (UNCHECKPOINTED before drop/reopen): partial delta updates + delete_by_id(20)
    // so reopening from disk exercises uncheckpointed WAL replay of delta chains and deletes!
    {
        let mut wtx = db.begin_write().unwrap();
        {
            let mut col = wtx.collection("accounts").unwrap();
            for i in 1..=19i64 {
                assert!(col
                    .update_by_id(&Value::Int64(i), r#"{"$inc": {"balance": 10}}"#)
                    .unwrap());
            }
            assert!(col.delete_by_id(&Value::Int64(20)).unwrap());
        }
        wtx.commit().unwrap();
    }

    // Drop WITHOUT checkpointing, reopen from disk, and verify full WAL recovery + integrity!
    drop(db);
    let reopened = Database::open(&db_path, cfg).unwrap();
    let ic = reopened.integrity_check().unwrap();
    assert!(
        ic.ok,
        "integrity issues after uncheckpointed WAL reopen: {:?}",
        ic.issues
    );
    assert_eq!(ic.documents_checked, 19);

    let rtx = reopened.begin_read().unwrap();
    let col = rtx.collection("accounts").unwrap();
    assert!(col.find_by_id(&Value::Int64(20)).unwrap().is_none());
    for i in 1..=19i64 {
        let doc = col.find_by_id(&Value::Int64(i)).unwrap().unwrap();
        assert_eq!(
            doc.view().get_field("balance").unwrap().to_owned_value(),
            Value::Int64(170)
        );
    }
}
