#![cfg(feature = "vec")]

//! End-to-end integration tests for `oryq-core --features vec` (and `--features fts,vec`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "fts")]
use oryq_core::VectorQuantization;
use oryq_core::{
    Database, DatabaseConfig, DistanceMetric, ErrorCategory, PageSize, Value, VectorIndexOptions,
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
            "oryqdb_vec_int_{label}_{}_{seq}_{nanos}",
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
fn vector_end_to_end_backfill_crud_reopen_planner_safety_and_backup() {
    let dir = TempDir::new("e2e");
    let db_path = dir.path().join("vectors.oryq");
    let backup_path = dir.path().join("vectors_backup.oryq");
    let cfg = DatabaseConfig {
        page_size: PageSize::Size4096,
        ..DatabaseConfig::default()
    };

    // 1. Insert existing docs, then create vector index (backfill)
    {
        let db = Database::open(&db_path, cfg.clone()).unwrap();
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("items").unwrap();
        {
            let mut col = wtx.collection("items").unwrap();
            col.insert_json(r#"{"_id": 1, "label": "cat", "embedding": [1.0, 0.0, 0.0]}"#)
                .unwrap();
            col.insert_json(r#"{"_id": 2, "label": "dog", "embedding": [0.8, 0.6, 0.0]}"#)
                .unwrap();
            col.insert_json(r#"{"_id": 3, "label": "car", "embedding": [0.0, 0.0, 1.0]}"#)
                .unwrap();
            col.insert_json(r#"{"_id": 4, "label": "no_vec"}"#).unwrap();

            let opts = VectorIndexOptions::new(3, DistanceMetric::Cosine).unwrap();
            let desc = col
                .create_vector_index("embedding", opts, Some("emb_cosine"))
                .unwrap();
            assert_eq!(desc.name, "emb_cosine");
            assert_eq!(desc.required_feature_bit, Some(1));
        }
        wtx.commit().unwrap();

        // Search after backfill
        {
            let rtx = db.begin_read().unwrap();
            let col = rtx.collection("items").unwrap();
            let hits = col
                .search_vector("emb_cosine", &[1.0, 0.0, 0.0], 2)
                .unwrap();
            assert_eq!(hits.len(), 2);
            assert_eq!(
                hits[0].0.view().get_field("_id").unwrap().to_owned_value(),
                Value::Int64(1)
            );
            assert_eq!(
                hits[1].0.view().get_field("_id").unwrap().to_owned_value(),
                Value::Int64(2)
            );

            // Planner safety regression: normal find_json on vector field must NOT use pluggable index
            let found = col
                .find_json(Some(r#"{"label": "car"}"#), None, None, 0, None)
                .unwrap()
                .collect_all();
            assert_eq!(found.len(), 1);
            assert_eq!(
                found[0].view().get_field("_id").unwrap().to_owned_value(),
                Value::Int64(3)
            );
        }

        // Insert invalid dimension fails atomically via savepoint rollback
        {
            let mut wtx = db.begin_write().unwrap();
            let mut col = wtx.collection("items").unwrap();
            let err = col
                .insert_json(r#"{"_id": 99, "label": "bad", "embedding": [1.0, 2.0]}"#)
                .unwrap_err();
            assert_eq!(err.category(), ErrorCategory::InvalidArgument);
            assert!(col.find_by_id(&Value::Int64(99)).unwrap().is_none());

            // Update & delete
            col.update_by_id(
                &Value::Int64(3),
                r#"{"$set": {"embedding": [0.95, 0.05, 0.0]}}"#,
            )
            .unwrap();
            col.delete_by_id(&Value::Int64(2)).unwrap();
            drop(col);
            wtx.commit().unwrap();
        }

        // Online backup preserves vector index
        db.backup_to(&backup_path).unwrap();
        db.checkpoint(true).unwrap();
    }

    // 2. Verify persistence across reopen AND in backup
    for path in [&db_path, &backup_path] {
        let db = Database::open(path, cfg.clone()).unwrap();
        let rtx = db.begin_read().unwrap();
        let col = rtx.collection("items").unwrap();
        let hits = col
            .search_vector("emb_cosine", &[1.0, 0.0, 0.0], 5)
            .unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0].0.view().get_field("_id").unwrap().to_owned_value(),
            Value::Int64(1)
        );
        assert_eq!(
            hits[1].0.view().get_field("_id").unwrap().to_owned_value(),
            Value::Int64(3)
        );
    }

    // 3. Drop vector index and verify search returns NotFound
    {
        let db = Database::open(&db_path, cfg).unwrap();
        let mut wtx = db.begin_write().unwrap();
        {
            let mut col = wtx.collection("items").unwrap();
            assert!(col.drop_index("emb_cosine").unwrap());
        }
        wtx.commit().unwrap();

        let rtx = db.begin_read().unwrap();
        let col = rtx.collection("items").unwrap();
        assert_eq!(
            col.search_vector("emb_cosine", &[1.0, 0.0, 0.0], 5)
                .unwrap_err()
                .category(),
            ErrorCategory::NotFound
        );
    }
}

#[cfg(all(feature = "fts", feature = "vec"))]
#[test]
fn combined_fts_and_vector_indexes_coexist_in_same_collection() {
    let dir = TempDir::new("fts_and_vec");
    let db_path = dir.path().join("hybrid.oryq");
    let backup_path = dir.path().join("hybrid_backup.oryq");
    let cfg = DatabaseConfig {
        page_size: PageSize::Size4096,
        ..DatabaseConfig::default()
    };

    let db = Database::open(&db_path, cfg.clone()).unwrap();
    let mut wtx = db.begin_write().unwrap();
    wtx.create_collection("docs").unwrap();
    {
        let mut col = wtx.collection("docs").unwrap();
        col.create_fts_index(&["title"], Some("title_fts")).unwrap();
        let opts =
            VectorIndexOptions::with_quantization(4, DistanceMetric::L2, VectorQuantization::Int8)
                .unwrap();
        col.create_vector_index("vec", opts, Some("vec_l2_int8"))
            .unwrap();

        col.insert_json(
            r#"{"_id": 1, "title": "Rust embedded database", "vec": [1.0, 0.0, 0.0, 0.0]}"#,
        )
        .unwrap();
        col.insert_json(
            r#"{"_id": 2, "title": "Vector search engine", "vec": [0.0, 1.0, 0.0, 0.0]}"#,
        )
        .unwrap();
    }
    wtx.commit().unwrap();

    // Both FTS and Vector searches work on the same collection
    {
        let rtx = db.begin_read().unwrap();
        let col = rtx.collection("docs").unwrap();
        let fts_hits = col.search_fts("title_fts", "embedded", 5).unwrap();
        assert_eq!(fts_hits.len(), 1);
        assert_eq!(
            fts_hits[0]
                .0
                .view()
                .get_field("_id")
                .unwrap()
                .to_owned_value(),
            Value::Int64(1)
        );
        let vec_hits = col
            .search_vector("vec_l2_int8", &[0.0, 1.0, 0.0, 0.0], 5)
            .unwrap();
        assert_eq!(vec_hits.len(), 2);
        assert_eq!(
            vec_hits[0]
                .0
                .view()
                .get_field("_id")
                .unwrap()
                .to_owned_value(),
            Value::Int64(2)
        );
    }

    // Online backup preserves BOTH FTS and Vector indexes
    db.backup_to(&backup_path).unwrap();
    let bdb = Database::open(&backup_path, cfg).unwrap();
    let rtx = bdb.begin_read().unwrap();
    let col = rtx.collection("docs").unwrap();
    assert_eq!(col.search_fts("title_fts", "vector", 5).unwrap().len(), 1);
    assert_eq!(
        col.search_vector("vec_l2_int8", &[1.0, 0.0, 0.0, 0.0], 1)
            .unwrap()[0]
            .0
            .view()
            .get_field("_id")
            .unwrap()
            .to_owned_value(),
        Value::Int64(1)
    );
}
