//! Integration tests for zero-disk in-memory ephemeral databases (`:memory:` / `MemoryVfs`,
//! `v0.12.1` / Issue #29).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_core::{Database, DatabaseConfig, ErrorCategory, PageSize, SortOrder, Value};
use oryq_engine::{MemoryVfs, OpenMode, StorageIo};

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
            "oryqdb_in_memory_{label}_{}_{seq}_{nanos}",
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
fn memory_vfs_positioned_io_locking_rename_and_remove() {
    let vfs = MemoryVfs::new();
    let p1 = Path::new("virtual_a.oryq");
    let p2 = Path::new("virtual_b.oryq");

    assert!(!vfs.exists(p1));
    assert_eq!(
        vfs.open(p1, OpenMode::READ_ONLY).unwrap_err().category(),
        ErrorCategory::NotFound
    );

    let fh = vfs.open(p1, OpenMode::CREATE_NEW).unwrap();
    assert!(vfs.exists(p1));
    assert_eq!(vfs.file_count(), 1);
    assert_eq!(
        vfs.open(p1, OpenMode::CREATE_NEW).unwrap_err().category(),
        ErrorCategory::Io
    );

    fh.write_all_at(b"hello", 4).unwrap();
    assert_eq!(fh.len().unwrap(), 9);
    let mut buf = [0u8; 9];
    fh.read_exact_at(&mut buf, 0).unwrap();
    assert_eq!(&buf, b"\0\0\0\0hello");

    // Read past EOF fails with Io
    let mut eof_buf = [0u8; 4];
    assert_eq!(
        fh.read_exact_at(&mut eof_buf, 7).unwrap_err().category(),
        ErrorCategory::Io
    );

    // Overflow checks on u64::MAX offsets
    assert_eq!(
        fh.read_exact_at(&mut eof_buf, u64::MAX)
            .unwrap_err()
            .category(),
        ErrorCategory::Overflow
    );
    assert_eq!(
        fh.write_all_at(b"x", u64::MAX).unwrap_err().category(),
        ErrorCategory::Overflow
    );

    // Empty write past EOF is a POSIX-compatible no-op (does not zero-extend)
    fh.write_all_at(&[], 100).unwrap();
    assert_eq!(fh.len().unwrap(), 9);

    // set_len grow & shrink
    fh.set_len(16).unwrap();
    assert_eq!(fh.len().unwrap(), 16);
    fh.set_len(5).unwrap();
    assert_eq!(fh.len().unwrap(), 5);

    // Read-only handle rejects write_all_at and set_len
    let ro = vfs.open(p1, OpenMode::READ_ONLY).unwrap();
    assert_eq!(
        ro.write_all_at(b"a", 0).unwrap_err().category(),
        ErrorCategory::Io
    );
    assert_eq!(ro.set_len(0).unwrap_err().category(), ErrorCategory::Io);
    drop(ro);

    // Truncate open mode clears contents
    let trunc_mode = OpenMode {
        truncate: true,
        ..OpenMode::READ_WRITE
    };
    let tfh = vfs.open(p1, trunc_mode).unwrap();
    assert_eq!(tfh.len().unwrap(), 0);
    drop(tfh);

    // Exclusive lock blocks second handle until first handle unlocks or drops
    fh.try_lock_exclusive().unwrap();
    // Re-acquiring lock on the same handle is idempotent
    fh.try_lock_exclusive().unwrap();
    let fh2 = vfs.open(p1, OpenMode::READ_WRITE).unwrap();
    assert_eq!(
        fh2.try_lock_exclusive().unwrap_err().category(),
        ErrorCategory::LockedByProcess
    );
    drop(fh);
    fh2.try_lock_exclusive().unwrap();
    fh2.unlock().unwrap();
    drop(fh2);

    // Rename onto same path is a no-op; rename onto existing path overwrites cleanly
    vfs.rename(p1, p1).unwrap();
    let fh_other = vfs.open(p2, OpenMode::CREATE_NEW).unwrap();
    fh_other.write_all_at(b"old", 0).unwrap();
    drop(fh_other);
    vfs.rename(p1, p2).unwrap();
    assert!(!vfs.exists(p1));
    assert!(vfs.exists(p2));
    assert_eq!(vfs.open(p2, OpenMode::READ_ONLY).unwrap().len().unwrap(), 0);
    vfs.remove_file(p2).unwrap();
    assert!(!vfs.exists(p2));
    assert_eq!(vfs.file_count(), 0);
}

#[test]
fn in_memory_databases_are_isolated_support_full_sql_parity_and_backup_to_disk() {
    let dir = TempDir::new("e2e");
    let backup_path = dir.path().join("exported.oryq");

    // Two concurrent `:memory:` databases in the same process are completely isolated
    let db1 = Database::open_in_memory().unwrap();
    let db2 = Database::open(":memory:", DatabaseConfig::default()).unwrap();
    assert!(db1.is_in_memory());
    assert!(db2.is_in_memory());

    // Populate db1 with collection, secondary index, and 80 docs
    {
        let mut wtx = db1.begin_write().unwrap();
        wtx.create_collection("orders").unwrap();
        {
            let mut col = wtx.collection("orders").unwrap();
            col.create_index("region", SortOrder::Asc, false, false, Some("region_idx"))
                .unwrap();
            let pad = "p".repeat(256);
            for i in 1..=80i64 {
                let region = if i % 2 == 0 { "apac" } else { "emea" };
                col.insert_json(&format!(
                    r#"{{"_id": {i}, "region": "{region}", "amount": {}, "pad": "{pad}"}}"#,
                    i * 10
                ))
                .unwrap();
            }
        }
        wtx.commit().unwrap();
    }

    // db2 must NOT see db1's collection
    {
        let rtx2 = db2.begin_read().unwrap();
        assert!(rtx2.list_collections().unwrap().is_empty());
    }

    // Delete 70 docs, checkpoint, and run in-memory VACUUM (zero disk I/O!)
    {
        let mut wtx = db1.begin_write().unwrap();
        {
            let mut col = wtx.collection("orders").unwrap();
            for i in 11..=80i64 {
                assert!(col.delete_by_id(&Value::Int64(i)).unwrap());
            }
        }
        wtx.commit().unwrap();
    }
    db1.checkpoint(true).unwrap();
    let vac = db1.vacuum().unwrap();
    assert!(vac.sealed);
    assert!(
        vac.bytes_after < vac.bytes_before,
        "expected in-memory VACUUM to reclaim pages: before={} after={}",
        vac.bytes_before,
        vac.bytes_after
    );
    assert_eq!(vac.documents_compacted, 10);
    assert!(db1.integrity_check().unwrap().ok);

    // Query & aggregation on db1
    {
        let rtx = db1.begin_read().unwrap();
        let col = rtx.collection("orders").unwrap();
        let apac = col
            .find_json(Some(r#"{"region": "apac"}"#), None, None, 0, None)
            .unwrap()
            .collect_all();
        assert_eq!(apac.len(), 5);

        let agg = col
            .aggregate_json(r#"[{"$group": {"_id": "$region", "total": {"$sum": "$amount"}}}]"#)
            .unwrap();
        assert_eq!(agg.len(), 2);
    }

    // Backing up to `:memory:` is rejected
    assert_eq!(
        db1.backup_to(":memory:").unwrap_err().category(),
        ErrorCategory::InvalidArgument
    );

    // Online backup from `:memory:` to a real single-file `.oryq` on disk
    let rep = db1.backup_to(&backup_path).unwrap();
    assert!(rep.sealed);
    assert_eq!(rep.collections_copied, 1);
    assert_eq!(rep.documents_copied, 10);
    assert!(backup_path.exists());
    drop(db1);
    drop(db2);

    // Reopen exported `.oryq` from disk and verify data + index survived
    let disk_db = Database::open(
        &backup_path,
        DatabaseConfig {
            page_size: PageSize::Size4096,
            ..DatabaseConfig::default()
        },
    )
    .unwrap();
    assert!(!disk_db.is_in_memory());
    assert!(disk_db.is_sealed().unwrap());
    assert!(disk_db.integrity_check().unwrap().ok);
    {
        let rtx = disk_db.begin_read().unwrap();
        let col = rtx.collection("orders").unwrap();
        assert_eq!(col.count().unwrap(), 10);
        let found = col
            .find_json(Some(r#"{"region": "emea"}"#), None, None, 0, None)
            .unwrap()
            .collect_all();
        assert_eq!(found.len(), 5);
    }

    // Zero-disk check: no `:memory:` or sidecar files were created in the working directory
    assert!(!Path::new(":memory:").exists());
    assert!(!Path::new(":memory:-wal").exists());
    assert!(!Path::new(":memory:-vacuum").exists());
}

#[test]
fn in_memory_edge_cases_readonly_create_flag_and_custom_memory_vfs_vacuum() {
    use std::sync::Arc;

    // 1. Opening a fresh `:memory:` DB with `read_only: true` (invalid combo or missing) or
    //    `create_if_missing: false` fails cleanly.
    assert_eq!(
        Database::open_in_memory_with_config(DatabaseConfig {
            read_only: true,
            ..DatabaseConfig::default()
        })
        .unwrap_err()
        .category(),
        ErrorCategory::InvalidArgument
    );
    assert_eq!(
        Database::open_in_memory_with_config(DatabaseConfig {
            read_only: true,
            create_if_missing: false,
            ..DatabaseConfig::default()
        })
        .unwrap_err()
        .category(),
        ErrorCategory::NotFound
    );
    assert_eq!(
        Database::open_in_memory_with_config(DatabaseConfig {
            create_if_missing: false,
            ..DatabaseConfig::default()
        })
        .unwrap_err()
        .category(),
        ErrorCategory::NotFound
    );

    // 2. `Database::open_with_io` with `MemoryVfs` and a custom virtual path (not `:memory:`)
    //    must still be detected as in-memory and perform zero-disk VACUUM cleanly.
    let vfs = Arc::new(MemoryVfs::new());
    let virtual_path = Path::new("custom_virtual_db.oryq");
    let cfg = DatabaseConfig {
        compression_threshold_bytes: None,
        ..DatabaseConfig::default()
    };
    let db = Database::open_with_io(vfs.clone(), virtual_path, cfg).unwrap();
    assert!(db.is_in_memory());
    {
        let mut wtx = db.begin_write().unwrap();
        wtx.create_collection("notes").unwrap();
        {
            let mut col = wtx.collection("notes").unwrap();
            let pad = "z".repeat(256);
            for i in 1..=40i64 {
                col.insert_json(&format!(r#"{{"_id": {i}, "pad": "{pad}"}}"#))
                    .unwrap();
            }
        }
        wtx.commit().unwrap();
    }
    {
        let mut wtx = db.begin_write().unwrap();
        {
            let mut col = wtx.collection("notes").unwrap();
            for i in 6..=40i64 {
                assert!(col.delete_by_id(&Value::Int64(i)).unwrap());
            }
        }
        wtx.commit().unwrap();
    }
    db.checkpoint(true).unwrap();
    let rep = db.vacuum().unwrap();
    assert!(rep.sealed);
    assert!(rep.bytes_after < rep.bytes_before);
    assert_eq!(rep.documents_compacted, 5);
    assert!(db.integrity_check().unwrap().ok);
    // Confirm nothing leaked onto real disk
    assert!(!virtual_path.exists());
}
