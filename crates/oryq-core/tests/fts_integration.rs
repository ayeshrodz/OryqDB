#![cfg(feature = "fts")]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_core::{Database, DatabaseConfig, PageSize, Value};

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
            "oryqdb_core_fts_{label}_{}_{seq}_{nanos}",
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
fn core_fts_create_backfill_search_update_delete_reopen_and_drop() {
    let dir = TempDir::new("e2e");
    let db_path = dir.path().join("fts_e2e.oryq");
    let cfg = DatabaseConfig {
        page_size: PageSize::Size4096,
        ..DatabaseConfig::default()
    };

    {
        let db = Database::open(&db_path, cfg.clone()).unwrap();
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("articles").unwrap();
        {
            let mut col = wtx.collection("articles").unwrap();
            // Insert doc BEFORE creating FTS index to verify backfill!
            col.insert_json(
                r#"{"_id": 1, "title": "Embedded Rust DB", "body": "OryqDB is a single-file Rust document database"}"#,
            )
            .unwrap();
            col.create_fts_index(&["title", "body"], Some("text_idx"))
                .unwrap();
            // Insert docs AFTER creating FTS index
            col.insert_json(
                r#"{"_id": 2, "title": "Garden Notes", "body": "Planting tomatoes in spring"}"#,
            )
            .unwrap();
            col.insert_json(
                r#"{"_id": 3, "title": "Rust Performance", "body": "Fast zero-copy Rust parsers"}"#,
            )
            .unwrap();
        }
        wtx.commit().unwrap();
        db.checkpoint(true).unwrap();
    }

    // Reopen from disk and verify BM25 search, update, delete, and drop_index
    {
        let db = Database::open(&db_path, cfg.clone()).unwrap();
        let ic = db.integrity_check().unwrap();
        assert!(ic.ok, "integrity issues: {:?}", ic.issues);

        {
            let rtx = db.begin_read().unwrap();
            let col = rtx.collection("articles").unwrap();
            // Regression check: normal `find()` equality on FTS-indexed leading field `title`
            // must NOT select the FTS index as a secondary index; it must return the exact doc!
            let eq_docs = col
                .find_json(
                    Some(r#"{"title": "Embedded Rust DB"}"#),
                    None,
                    None,
                    0,
                    None,
                )
                .unwrap()
                .collect_all();
            assert_eq!(eq_docs.len(), 1);
            assert_eq!(
                eq_docs[0].view().get_field("_id").unwrap().to_owned_value(),
                Value::Int64(1)
            );

            let hits = col.search_fts("text_idx", "rust database", 10).unwrap();
            assert_eq!(hits.len(), 2);
            assert_eq!(
                hits[0].0.view().get_field("_id").unwrap().to_owned_value(),
                Value::Int64(1)
            );
            assert_eq!(
                hits[1].0.view().get_field("_id").unwrap().to_owned_value(),
                Value::Int64(3)
            );
            assert!(hits[0].1 > hits[1].1);
        }

        // Update doc 2 to include "rust" and delete doc 3
        {
            let mut wtx = db.begin_write().unwrap();
            {
                let mut col = wtx.collection("articles").unwrap();
                assert!(col
                    .update_by_id(
                        &Value::Int64(2),
                        r#"{"$set": {"body": "Rust tools for garden sensors"}}"#
                    )
                    .unwrap());
                assert!(col.delete_by_id(&Value::Int64(3)).unwrap());
            }
            wtx.commit().unwrap();
        }

        {
            let rtx = db.begin_read().unwrap();
            let col = rtx.collection("articles").unwrap();
            let hits = col.search_fts("text_idx", "rust", 10).unwrap();
            assert_eq!(hits.len(), 2);
            let ids: Vec<Value> = hits
                .iter()
                .map(|(d, _)| d.view().get_field("_id").unwrap().to_owned_value())
                .collect();
            assert!(ids.contains(&Value::Int64(1)));
            assert!(ids.contains(&Value::Int64(2)));
        }

        // Online backup preserves FTS indexes so `search_fts` works on the restored backup!
        let backup_path = dir.path().join("fts_backup.oryq");
        db.backup_to(&backup_path).unwrap();
        {
            let bdb = Database::open(&backup_path, cfg.clone()).unwrap();
            let brtx = bdb.begin_read().unwrap();
            let bcol = brtx.collection("articles").unwrap();
            let bhits = bcol.search_fts("text_idx", "rust", 10).unwrap();
            assert_eq!(bhits.len(), 2);
        }

        // In-place VACUUM preserves FTS indexes so `search_fts` continues working!
        let vac_rep = db.vacuum().unwrap();
        assert!(vac_rep.sealed);
        {
            let rtx = db.begin_read().unwrap();
            let col = rtx.collection("articles").unwrap();
            let vhits = col.search_fts("text_idx", "rust", 10).unwrap();
            assert_eq!(vhits.len(), 2);
        }

        // Drop FTS index cleanly
        {
            let mut wtx = db.begin_write().unwrap();
            {
                let mut col = wtx.collection("articles").unwrap();
                assert!(col.drop_index("text_idx").unwrap());
            }
            wtx.commit().unwrap();
        }
    }
}
