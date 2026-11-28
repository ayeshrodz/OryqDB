//! Integration tests for Issue #47 (v0.15.0):
//! lazy streaming query/count, k-way spillable_sort, chunked backup/vacuum, and batch CRUD.

use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_core::{Database, DatabaseConfig, SortOrder};
use oryq_document::{from_json_str, FieldPath};
use oryq_query::{spillable_sort, QueryContext, SortSpec};
use oryq_types::ErrorCategory;

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!(
            "oryq_issue47_{label}_{}_{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&p).unwrap();
        Self(p)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn streaming_kway_spillable_sort_orders_many_runs_exactly() {
    let mut docs = Vec::with_capacity(400);
    for i in 0..400i64 {
        // Deterministic pseudo-shuffled keys so every spill run has overlapping ranges.
        let key = (i * 197 + 31) % 400;
        let json =
            format!(r#"{{"_id": {i}, "rank": {key}, "pad": "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx"}}"#);
        let (doc, _) = from_json_str(&json, None).unwrap();
        docs.push(doc);
    }

    // Tiny 2 KiB memory budget forces dozens of spill runs and k-way BinaryHeap merge.
    let ctx = QueryContext::default_with_limit(2 * 1024);
    let sort = SortSpec {
        keys: vec![(FieldPath::parse("rank").unwrap(), SortOrder::Asc)],
    };
    let sorted = spillable_sort(docs, &sort, &ctx).unwrap();
    assert_eq!(sorted.len(), 400);
    for (idx, doc) in sorted.iter().enumerate() {
        assert_eq!(
            doc.view().get_field("rank").unwrap().to_owned_value(),
            oryq_document::Value::Int64(idx as i64)
        );
    }
}

#[test]
fn chunked_backup_and_vacuum_preserve_large_collection_and_indexes() {
    let dir = TempDir::new("chunked_backup");
    let src_path = dir.0.join("src.oryq");
    let bak_path = dir.0.join("bak.oryq");

    let cfg = DatabaseConfig {
        sync_mode: oryq_core::SyncMode::Off,
        ..DatabaseConfig::default()
    };
    let db = Database::open(&src_path, cfg).unwrap();
    {
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("events").unwrap();
        let mut col = wtx.collection("events").unwrap();
        col.create_index("bucket", SortOrder::Asc, false, false, Some("idx_bucket"))
            .unwrap();
        for i in 0..2_500i64 {
            col.insert_json(&format!(
                r#"{{"_id": {i}, "bucket": {}, "msg": "payload-{i}"}}"#,
                i % 25
            ))
            .unwrap();
        }
        wtx.commit().unwrap();
    }

    // Backup crosses BACKUP_CHUNK_DOCS (1,000) multiple times.
    let bak_report = db.backup_to(&bak_path).unwrap();
    assert_eq!(bak_report.collections_copied, 1);
    assert_eq!(bak_report.documents_copied, 2_500);
    assert!(bak_report.sealed);

    let bak_db = Database::open(&bak_path, DatabaseConfig::default()).unwrap();
    assert!(bak_db.integrity_check().unwrap().ok);
    {
        let rtx = bak_db.begin_read().unwrap();
        let col = rtx.collection("events").unwrap();
        assert_eq!(col.count().unwrap(), 2_500);
        let filter = oryq_core::parse_filter_json(r#"{"bucket": 7}"#).unwrap();
        assert_eq!(col.count_matching(Some(&filter)).unwrap(), 100);
        let top5 = col
            .find_json(Some(r#"{"bucket": 7}"#), None, None, 10, Some(5))
            .unwrap()
            .collect_all();
        assert_eq!(top5.len(), 5);
    }
    drop(bak_db);

    // Vacuum also runs chunked copy + integrity check.
    let vac_report = db.vacuum().unwrap();
    assert_eq!(vac_report.documents_compacted, 2_500);
    assert!(db.integrity_check().unwrap().ok);
    {
        let rtx = db.begin_read().unwrap();
        assert_eq!(rtx.collection("events").unwrap().count().unwrap(), 2_500);
    }
}

#[test]
fn batch_crud_insert_update_delete_and_upsert_work_end_to_end() {
    let db = Database::open_in_memory().unwrap();
    {
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("users").unwrap();
        let mut col = wtx.collection("users").unwrap();
        col.create_index("team", SortOrder::Asc, false, false, Some("idx_team"))
            .unwrap();

        // insert_many_json
        let inserted = col
            .insert_many_json(
                r#"[
                    {"_id": 1, "name": "Ada", "team": "core", "score": 10},
                    {"_id": 2, "name": "Grace", "team": "core", "score": 20},
                    {"_id": 3, "name": "Linus", "team": "kernel", "score": 30}
                ]"#,
            )
            .unwrap();
        assert_eq!(inserted.len(), 3);

        // Non-array input rejected
        let err = col.insert_many_json(r#"{"_id": 99}"#).unwrap_err();
        assert_eq!(err.category(), ErrorCategory::InvalidArgument);

        // update_one_json updates exactly 1 matching doc
        assert!(col
            .update_one_json(r#"{"team": "core"}"#, r#"{"$inc": {"score": 5}}"#)
            .unwrap());

        // update_many_json updates all matching docs
        let upd_count = col
            .update_many_json(r#"{"team": "core"}"#, r#"{"$set": {"active": true}}"#)
            .unwrap();
        assert_eq!(upd_count, 2);

        // upsert_json: replace existing (_id=3) -> false; insert new (_id=4) -> true
        assert!(!col
            .upsert_json(r#"{"_id": 3, "name": "Linus T.", "team": "kernel", "score": 35}"#)
            .unwrap());
        assert!(col
            .upsert_json(r#"{"_id": 4, "name": "Ken", "team": "unix", "score": 40}"#)
            .unwrap());

        // delete_one_json deletes 1 doc
        assert!(col.delete_one_json(r#"{"team": "core"}"#).unwrap());
        assert!(!col.delete_one_json(r#"{"team": "nonexistent"}"#).unwrap());

        // delete_many_json deletes remaining matching docs
        let del_count = col.delete_many_json(r#"{"score": {"$gte": 35}}"#).unwrap();
        assert_eq!(del_count, 2);

        wtx.commit().unwrap();
    }

    let rtx = db.begin_read().unwrap();
    let col = rtx.collection("users").unwrap();
    assert_eq!(col.count().unwrap(), 1);
    let remaining = col.find_all().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(
        remaining[0]
            .view()
            .get_field("active")
            .unwrap()
            .to_owned_value(),
        oryq_document::Value::Bool(true)
    );
    assert!(db.integrity_check().unwrap().ok);
}
