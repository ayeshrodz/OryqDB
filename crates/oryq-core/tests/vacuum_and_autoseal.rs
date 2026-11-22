//! Tests for single-file auto-seal on close (`auto_checkpoint_on_close`) and in-place
//! `Database::vacuum()` file compaction (`v0.12.0` / Issue #28).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_core::{Database, DatabaseConfig, ErrorCategory, PageSize, SortOrder, SyncMode, Value};
use oryq_engine::wal_path_for;

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
            "oryqdb_vacuum_{label}_{}_{seq}_{nanos}",
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
fn auto_checkpoint_on_close_leaves_single_sealed_oryq_file() {
    let dir = TempDir::new("autoseal");
    let db_path = dir.path().join("single.oryq");
    let wal_path = wal_path_for(&db_path);

    // 1. Default config (`auto_checkpoint_on_close = true`) seals and deletes `.oryq-wal` on drop
    {
        let db = Database::open(&db_path, DatabaseConfig::default()).unwrap();
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("notes").unwrap();
        {
            let mut col = wtx.collection("notes").unwrap();
            col.insert_json(r#"{"_id": 1, "body": "single file persistence"}"#)
                .unwrap();
        }
        wtx.commit().unwrap();
        // While open & uncheckpointed, WAL exists
        assert!(wal_path.exists());
    }
    // After clean drop, ONLY the single `.oryq` file remains on disk!
    assert!(db_path.exists());
    assert!(
        !wal_path.exists(),
        "expected .oryq-wal to be removed on clean close"
    );

    // Reopening sees sealed DB with exact committed document
    {
        let db = Database::open(&db_path, DatabaseConfig::default()).unwrap();
        assert!(db.is_sealed().unwrap());
        let rtx = db.begin_read().unwrap();
        let col = rtx.collection("notes").unwrap();
        assert!(col.find_by_id(&Value::Int64(1)).unwrap().is_some());
    }

    // 2. Opt-out (`auto_checkpoint_on_close = false`) preserves unsealed `.oryq-wal` on drop
    let optout_path = dir.path().join("optout.oryq");
    let optout_wal = wal_path_for(&optout_path);
    {
        let db = Database::open(
            &optout_path,
            DatabaseConfig {
                auto_checkpoint_on_close: false,
                ..DatabaseConfig::default()
            },
        )
        .unwrap();
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("notes").unwrap();
        wtx.collection("notes")
            .unwrap()
            .insert_json(r#"{"_id": 1}"#)
            .unwrap();
        wtx.commit().unwrap();
    }
    assert!(optout_wal.exists());
}

#[test]
fn vacuum_shrinks_file_preserves_indexes_and_rejects_busy_or_readonly() {
    let dir = TempDir::new("vacuum");
    let db_path = dir.path().join("bloated.oryq");
    let wal_path = wal_path_for(&db_path);
    let cfg = DatabaseConfig {
        page_size: PageSize::Size4096,
        sync_mode: SyncMode::Normal,
        ..DatabaseConfig::default()
    };

    let db = Database::open(&db_path, cfg.clone()).unwrap();
    let payload = "x".repeat(512);

    // Insert 150 docs with a secondary index so `.oryq` grows across many pages
    {
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("items").unwrap();
        {
            let mut col = wtx.collection("items").unwrap();
            col.create_index("tag", SortOrder::Asc, false, false, Some("tag_idx"))
                .unwrap();
            for i in 1..=150i64 {
                let doc = format!(
                    r#"{{"_id": {i}, "tag": "t{}", "pad": "{payload}"}}"#,
                    i % 10
                );
                col.insert_json(&doc).unwrap();
            }
        }
        wtx.commit().unwrap();
    }
    db.checkpoint(true).unwrap();

    // Delete 140 of the 150 docs and checkpoint -> file still retains old allocated pages
    {
        let mut wtx = db.begin_write().unwrap();
        {
            let mut col = wtx.collection("items").unwrap();
            for i in 11..=150i64 {
                assert!(col.delete_by_id(&Value::Int64(i)).unwrap());
            }
        }
        wtx.commit().unwrap();
    }
    db.checkpoint(true).unwrap();
    let size_before = std::fs::metadata(&db_path).unwrap().len();

    // VACUUM while a ReadTransaction is open must fail with Busy
    {
        let _rtx = db.begin_read().unwrap();
        assert_eq!(db.vacuum().unwrap_err().category(), ErrorCategory::Busy);
    }

    // VACUUM when idle shrinks the `.oryq` file, seals it, and removes `.oryq-wal`
    let rep = db.vacuum().unwrap();
    assert!(rep.sealed);
    assert_eq!(rep.bytes_before, size_before);
    assert!(
        rep.bytes_after < rep.bytes_before,
        "expected VACUUM to shrink file: before={} after={}",
        rep.bytes_before,
        rep.bytes_after
    );
    assert_eq!(rep.bytes_reclaimed, rep.bytes_before - rep.bytes_after);
    assert_eq!(rep.collections_compacted, 1);
    assert_eq!(rep.documents_compacted, 10);
    assert!(!wal_path.exists());
    assert_eq!(std::fs::metadata(&db_path).unwrap().len(), rep.bytes_after);
    assert!(db.integrity_check().unwrap().ok);

    // Verify remaining 10 docs & secondary index still work both live and after reopen
    {
        let rtx = db.begin_read().unwrap();
        let col = rtx.collection("items").unwrap();
        assert_eq!(col.count().unwrap(), 10);
        let found = col
            .find_json(Some(r#"{"tag": "t1"}"#), None, None, 0, None)
            .unwrap()
            .collect_all();
        assert_eq!(found.len(), 1);
    }
    drop(db);

    // Reopen read-only -> verify data and confirm VACUUM rejects read-only DB
    let ro_db = Database::open(
        &db_path,
        DatabaseConfig {
            read_only: true,
            ..cfg
        },
    )
    .unwrap();
    assert!(ro_db.integrity_check().unwrap().ok);
    assert_eq!(
        ro_db.vacuum().unwrap_err().category(),
        ErrorCategory::InvalidArgument
    );
}
