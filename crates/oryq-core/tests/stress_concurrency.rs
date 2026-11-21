//! End-to-end multi-threaded concurrency, delta-compaction, savepoint rollback storm,
//! and integrity verification stress test for `oryq-core`.

use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_core::{Database, DatabaseConfig, ErrorCategory, PageSize, SortOrder, Value};

fn unique_temp_dir(label: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "oryqdb_core_stress_{label}_{}_{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn concurrent_readers_writers_delta_compaction_and_savepoint_rollback_storm() {
    let dir = unique_temp_dir("concurrency");
    let db_path = dir.join("stress.oryq");
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

    // Writer executes 6 rounds of partial updates (triggering delta-chain + compaction)
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
        if round % 2 == 0 {
            let _ = db.checkpoint(true).unwrap();
        }
    }

    for h in readers {
        h.join().unwrap();
    }

    // Verify full integrity and exact balances (100 + 6 * 10 = 160)
    let ic = db.integrity_check().unwrap();
    assert!(ic.ok, "integrity issues: {:?}", ic.issues);
    assert_eq!(ic.documents_checked, 20);
    {
        let rtx = db.begin_read().unwrap();
        let col = rtx.collection("accounts").unwrap();
        for i in 1..=20i64 {
            let doc = col.find_by_id(&Value::Int64(i)).unwrap().unwrap();
            assert_eq!(
                doc.view().get_field("balance").unwrap().to_owned_value(),
                Value::Int64(160)
            );
        }
    }

    // Reopen from disk and re-verify
    drop(db);
    let reopened = Database::open(&db_path, cfg).unwrap();
    let ic2 = reopened.integrity_check().unwrap();
    assert!(ic2.ok);
    assert_eq!(ic2.documents_checked, 20);

    let _ = std::fs::remove_dir_all(&dir);
}
