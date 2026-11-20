//! Reproducible local benchmark suite measuring OryqDB insert, point lookup,
//! partial delta update + compaction, secondary range scan, and WAL checkpoint throughput.

use std::time::{Instant, SystemTime, UNIX_EPOCH};

use oryq_core::{Database, DatabaseConfig, PageSize, SortOrder, SyncMode, Value};

fn main() {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("oryqdb_bench_{}_{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("bench.oryq");

    let cfg = DatabaseConfig {
        page_size: PageSize::Size8192,
        sync_mode: SyncMode::Normal,
        ..DatabaseConfig::default()
    };

    let db = Database::open(&db_path, cfg).unwrap();
    const N: i64 = 2_000;

    // 1. Batched insert with secondary index
    let t0 = Instant::now();
    {
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("records").unwrap();
        let mut col = wtx.collection("records").unwrap();
        col.create_index("score", SortOrder::Asc, false, false, Some("score_1"))
            .unwrap();
        for i in 1..=N {
            let json = format!(
                r#"{{"_id": {i}, "name": "user_{i}", "score": {}, "region": "nz"}}"#,
                i % 500
            );
            col.insert_json(&json).unwrap();
        }
        wtx.commit().unwrap();
    }
    let insert_ms = t0.elapsed().as_secs_f64() * 1000.0;

    // 2. Primary _id point lookups
    let t1 = Instant::now();
    {
        let rtx = db.begin_read().unwrap();
        let col = rtx.collection("records").unwrap();
        for i in 1..=N {
            let doc = col.find_by_id(&Value::Int64(i)).unwrap();
            assert!(doc.is_some());
        }
    }
    let lookup_ms = t1.elapsed().as_secs_f64() * 1000.0;

    // 3. Partial delta updates (6 updates per doc -> exercises 4-delta chain + compaction)
    let t2 = Instant::now();
    {
        let mut wtx = db.begin_write().unwrap();
        let mut col = wtx.collection("records").unwrap();
        for round in 0..6 {
            for i in 1..=200 {
                let upd = format!(r#"{{"$inc": {{"score": 1}}, "$set": {{"round": {round}}}}}"#);
                assert!(col.update_by_id(&Value::Int64(i), &upd).unwrap());
            }
        }
        wtx.commit().unwrap();
    }
    let update_ms = t2.elapsed().as_secs_f64() * 1000.0;

    // 4. Secondary range scan (100 iterations)
    let t3 = Instant::now();
    {
        let rtx = db.begin_read().unwrap();
        let col = rtx.collection("records").unwrap();
        for _ in 0..100 {
            let rows = col
                .find_json(
                    Some(r#"{"score": {"$gte": 100, "$lt": 150}}"#),
                    None,
                    None,
                    0,
                    None,
                )
                .unwrap()
                .collect_all();
            assert!(!rows.is_empty());
        }
    }
    let range_ms = t3.elapsed().as_secs_f64() * 1000.0;

    // 5. WAL checkpoint + reset
    let t4 = Instant::now();
    let outcome = db.checkpoint(true).unwrap();
    let ckpt_ms = t4.elapsed().as_secs_f64() * 1000.0;

    println!("=== OryqDB Benchmark Suite (N={N}, 8KiB pages, SyncMode::Normal) ===");
    println!(
        "Batched Insert (2,000 docs + 1 secondary index): {:.2} ms ({:.0} docs/sec)",
        insert_ms,
        (N as f64) / (insert_ms / 1000.0)
    );
    println!(
        "Primary _id Point Lookup (2,000 lookups):       {:.2} ms ({:.0} ops/sec)",
        lookup_ms,
        (N as f64) / (lookup_ms / 1000.0)
    );
    println!(
        "Partial Delta Updates (1,200 ops, 6 rounds):    {:.2} ms ({:.0} ops/sec)",
        update_ms,
        1200.0 / (update_ms / 1000.0)
    );
    println!(
        "Secondary Range Scan (100 scans x ~200 rows):   {:.2} ms ({:.0} scans/sec)",
        range_ms,
        100.0 / (range_ms / 1000.0)
    );
    println!(
        "WAL Checkpoint + Reset ({} pages copied):       {:.2} ms",
        outcome.pages_copied, ckpt_ms
    );

    drop(db);
    let _ = std::fs::remove_dir_all(&dir);
}
