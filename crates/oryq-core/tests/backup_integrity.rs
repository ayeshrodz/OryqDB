use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_core::{Database, DatabaseConfig, ErrorCategory, PageSize, SortOrder, Value};

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "oryqdb_admin_{label}_{}_{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn integrity_check_sealed_online_backup_and_readonly_inspection() {
    let dir = unique_temp_dir("backup");
    let src_path = dir.join("live.oryq");
    let dst_path = dir.join("backup_sealed.oryq");

    let cfg = DatabaseConfig {
        page_size: PageSize::Size4096,
        ..DatabaseConfig::default()
    };

    {
        let db = Database::open(&src_path, cfg.clone()).unwrap();
        {
            let mut wtx = db.begin_write().unwrap();
            wtx.create_collection("users").unwrap();
            let mut col = wtx.collection("users").unwrap();
            col.create_index("age", SortOrder::Asc, false, false, Some("age_1"))
                .unwrap();
            col.create_compound_index(
                &[("tags", SortOrder::Asc), ("age", SortOrder::Desc)],
                false,
                false,
                Some("tags_age"),
            )
            .unwrap();
            col.insert_json(r#"{"_id": 1, "name": "Alice", "age": 30, "tags": ["rust", "db"]}"#)
                .unwrap();
            col.insert_json(r#"{"_id": 2, "name": "Bob", "age": 25, "tags": ["rust"]}"#)
                .unwrap();
            wtx.commit().unwrap();
        }

        // 1. Integrity check passes
        let ic = db.integrity_check().unwrap();
        assert!(ic.ok, "unexpected integrity issues: {:?}", ic.issues);
        assert_eq!(ic.collections_checked, 1);
        assert_eq!(ic.documents_checked, 2);
        assert_eq!(ic.indexes_checked, 3); // primary + secondary + compound

        // 2. Rejects backing up onto live source file
        let err = db.backup_to(&src_path).unwrap_err();
        assert_eq!(err.category(), ErrorCategory::InvalidArgument);

        // 3. Online backup produces a standalone sealed copy
        let report = db.backup_to(&dst_path).unwrap();
        assert_eq!(report.collections_copied, 1);
        assert_eq!(report.documents_copied, 2);
        assert!(report.sealed);

        // 4. Non-mutating lock probe while live DB is open
        let locks_while_open = Database::inspect_locks(&src_path).unwrap();
        assert!(locks_while_open.file_exists);
        assert!(!locks_while_open.exclusive_lock_available);

        // On Windows, mandatory locks reject reads from separate handles with LockedByProcess.
        #[cfg(windows)]
        {
            let err = Database::inspect_page(&src_path, 0).unwrap_err();
            assert_eq!(err.category(), ErrorCategory::LockedByProcess);
        }
    }

    // After closing live DB, exclusive lock is available and read-only page/WAL inspection succeeds on all OSes
    let locks_after_close = Database::inspect_locks(&src_path).unwrap();
    assert!(locks_after_close.exclusive_lock_available);

    let p0 = Database::inspect_page(&src_path, 0).unwrap();
    assert_eq!(p0.page_type, "StorageHeader");
    assert!(p0.checksum_valid);
    assert_eq!(p0.page_size, 4096);

    let wal = Database::inspect_wal(&src_path).unwrap();
    assert!(wal.wal_exists);
    assert!(wal.committed_txns >= 1);

    // Backup opens cleanly standalone (without WAL!) and contains exact data + indexes
    {
        let backup_db = Database::open(&dst_path, cfg).unwrap();
        assert!(backup_db.is_sealed().unwrap());
        let ic = backup_db.integrity_check().unwrap();
        assert!(ic.ok);
        assert_eq!(ic.documents_checked, 2);
        assert_eq!(ic.indexes_checked, 3);

        let rtx = backup_db.begin_read().unwrap();
        let col = rtx.collection("users").unwrap();
        let alice = col.find_by_id(&Value::Int64(1)).unwrap().unwrap();
        assert_eq!(
            alice.view().get_field("name").unwrap().to_owned_value(),
            Value::String("Alice".into())
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}
