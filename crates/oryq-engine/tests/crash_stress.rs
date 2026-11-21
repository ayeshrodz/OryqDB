//! Crash-recovery, Page 0 shadow-header failover, WAL/page corruption, and multi-threaded
//! snapshot/writer stress tests for `oryq-engine`.

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_engine::{wal_path_for, BTree, EngineConfig, OsVfs, PageSize, StorageEngine, TreeKind};
use oryq_types::ErrorCategory;

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
            "oryqdb_engine_stress_{label}_{}_{seq}_{nanos}",
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

fn flip_byte_at(path: &Path, offset: u64) {
    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    f.seek(SeekFrom::Start(offset)).unwrap();
    let mut byte = [0u8; 1];
    f.read_exact(&mut byte).unwrap();
    byte[0] ^= 0xFF;
    f.seek(SeekFrom::Start(offset)).unwrap();
    f.write_all(&byte).unwrap();
    f.sync_all().unwrap();
}

#[test]
fn repeated_crash_truncated_wal_tail_and_midframe_truncation_preserve_committed_txns() {
    let dir = TempDir::new("wal_crash_cycles");
    let db_path = dir.path().join("crash.oryq");
    let wal_path = wal_path_for(&db_path);
    let cfg = EngineConfig {
        page_size: PageSize::Size4096,
        ..EngineConfig::default()
    };

    let mut root_page_id = None;
    for cycle in 1..=15u64 {
        let did_checkpoint = cycle % 5 == 0;
        {
            let vfs = Arc::new(OsVfs::new());
            let engine = StorageEngine::open(vfs, &db_path, cfg.clone(), [0x42; 16]).unwrap();

            // Verify all previously committed cycles survived!
            if let Some(root) = root_page_id {
                let rtx = engine.begin_read().unwrap();
                let tree = BTree::open(root, TreeKind::Collection);
                for prev in 1..cycle {
                    let k = prev.to_be_bytes();
                    let val = tree.get(&rtx, &k).unwrap();
                    assert_eq!(
                        val,
                        Some(format!("payload_{prev}").into_bytes()),
                        "missing committed key {prev} on cycle {cycle}"
                    );
                }
            }

            // Commit new cycle
            let mut wtx = engine.begin_write().unwrap();
            let mut tree = match root_page_id {
                None => BTree::create_empty(&mut wtx, TreeKind::Collection).unwrap(),
                Some(r) => BTree::open(r, TreeKind::Collection),
            };
            tree.insert(
                &mut wtx,
                &cycle.to_be_bytes(),
                format!("payload_{cycle}").as_bytes(),
            )
            .unwrap();
            root_page_id = Some(tree.root());
            wtx.set_catalog_root_page_id(tree.root());
            wtx.commit().unwrap();

            if did_checkpoint {
                engine.checkpoint(true).unwrap();
            }
        }

        if !did_checkpoint {
            assert!(
                wal_path.exists(),
                "WAL must exist on non-checkpoint cycle {cycle}"
            );
            // Alternate between garbage tail append and simulated partial/mid-frame tail truncation
            let mut f = OpenOptions::new()
                .read(true)
                .append(true)
                .open(&wal_path)
                .unwrap();
            if cycle % 2 == 0 {
                // Simulate a torn mid-header write (< WAL_FRAME_HEADER_SIZE = 40 bytes)
                let before_len = f.metadata().unwrap().len();
                f.write_all(&[0x5A; 39]).unwrap();
                f.sync_all().unwrap();
                f.set_len(before_len + 23).unwrap();
            } else {
                f.write_all(&[0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03])
                    .unwrap();
            }
            f.sync_all().unwrap();
        }
    }

    // Final reopen verifies all 15 cycles intact
    {
        let vfs = Arc::new(OsVfs::new());
        let engine = StorageEngine::open(vfs, &db_path, cfg.clone(), [0x42; 16]).unwrap();
        let rtx = engine.begin_read().unwrap();
        let tree = BTree::open(root_page_id.unwrap(), TreeKind::Collection);
        assert_eq!(tree.scan_all(&rtx).unwrap().len(), 15);

        // Write one more uncheckpointed tx so we can test complete WAL frame bitflip -> Corruption
        let mut wtx = engine.begin_write().unwrap();
        let mut wtree = BTree::open(root_page_id.unwrap(), TreeKind::Collection);
        wtree
            .insert(&mut wtx, &99u64.to_be_bytes(), b"wal_bitflip_test")
            .unwrap();
        wtx.set_catalog_root_page_id(wtree.root());
        wtx.commit().unwrap();
    }

    // Flip a byte inside a complete committed WAL frame -> MUST fail closed with Corruption!
    assert!(wal_path.exists());
    flip_byte_at(&wal_path, 64);
    {
        let vfs = Arc::new(OsVfs::new());
        let err = StorageEngine::open(vfs, &db_path, cfg, [0x42; 16]).unwrap_err();
        assert_eq!(err.category(), ErrorCategory::Corruption);
    }
}

#[test]
fn page0_shadow_slot_failover_and_dual_slot_corruption_fail_closed() {
    let dir = TempDir::new("page0_failover");
    let db_path = dir.path().join("page0.oryq");
    let cfg = EngineConfig {
        page_size: PageSize::Size4096,
        ..EngineConfig::default()
    };

    let root;
    {
        let vfs = Arc::new(OsVfs::new());
        let engine = StorageEngine::open(vfs, &db_path, cfg.clone(), [0x33; 16]).unwrap();
        let mut wtx = engine.begin_write().unwrap();
        let mut tree = BTree::create_empty(&mut wtx, TreeKind::Collection).unwrap();
        tree.insert(&mut wtx, &1u64.to_be_bytes(), b"survives_slot_corruption")
            .unwrap();
        root = tree.root();
        wtx.set_catalog_root_page_id(root);
        wtx.commit().unwrap();
        // Checkpoint twice so BOTH Slot A and Slot B hold valid committed headers!
        engine.checkpoint(true).unwrap();

        let mut wtx2 = engine.begin_write().unwrap();
        let mut tree2 = BTree::open(root, TreeKind::Collection);
        tree2
            .insert(&mut wtx2, &2u64.to_be_bytes(), b"second_gen")
            .unwrap();
        wtx2.set_catalog_root_page_id(tree2.root());
        wtx2.commit().unwrap();
        engine.checkpoint(true).unwrap();
    }

    // 1. Corrupt Page 0 Slot A (offset 32) -> StorageEngine must fail over cleanly to Slot B!
    flip_byte_at(&db_path, 32);
    {
        let vfs = Arc::new(OsVfs::new());
        let engine = StorageEngine::open(vfs, &db_path, cfg.clone(), [0x33; 16]).unwrap();
        let rtx = engine.begin_read().unwrap();
        let tree = BTree::open(rtx.catalog_root_page_id(), TreeKind::Collection);
        assert_eq!(
            tree.get(&rtx, &1u64.to_be_bytes()).unwrap(),
            Some(b"survives_slot_corruption".to_vec())
        );
    }

    // 2. Now corrupt Page 0 Slot B as well (offset 256 + 32) -> MUST fail closed with Corruption!
    flip_byte_at(&db_path, 256 + 32);
    {
        let vfs = Arc::new(OsVfs::new());
        let err = StorageEngine::open(vfs, &db_path, cfg, [0x33; 16]).unwrap_err();
        assert_eq!(err.category(), ErrorCategory::Corruption);
    }
}

#[test]
fn page_bitflip_detected_as_corruption_without_panic() {
    let dir = TempDir::new("bitflip");
    let db_path = dir.path().join("flip.oryq");
    let cfg = EngineConfig {
        page_size: PageSize::Size4096,
        ..EngineConfig::default()
    };

    let root;
    {
        let vfs = Arc::new(OsVfs::new());
        let engine = StorageEngine::open(vfs, &db_path, cfg.clone(), [0x55; 16]).unwrap();
        let mut wtx = engine.begin_write().unwrap();
        let mut tree = BTree::create_empty(&mut wtx, TreeKind::Collection).unwrap();
        for i in 1..=20u64 {
            tree.insert(&mut wtx, &i.to_be_bytes(), b"important_record_data")
                .unwrap();
        }
        root = tree.root();
        wtx.set_catalog_root_page_id(root);
        wtx.commit().unwrap();
        engine.checkpoint(true).unwrap();
    }

    // Flip a byte inside the data root page on disk
    flip_byte_at(&db_path, root.raw() * 4096 + 128);

    // Reading the corrupted page must return Corruption, never panic
    {
        let vfs = Arc::new(OsVfs::new());
        let engine = StorageEngine::open(vfs, &db_path, cfg, [0x55; 16]).unwrap();
        let rtx = engine.begin_read().unwrap();
        let tree = BTree::open(root, TreeKind::Collection);
        let err = tree.get(&rtx, &1u64.to_be_bytes()).unwrap_err();
        assert_eq!(err.category(), ErrorCategory::Corruption);
    }
}

#[test]
fn concurrent_readers_with_pinned_snapshot_and_writer_checkpoint_stress() {
    let dir = TempDir::new("mvcc_stress");
    let db_path = dir.path().join("mvcc.oryq");
    let cfg = EngineConfig {
        page_size: PageSize::Size4096,
        ..EngineConfig::default()
    };
    let vfs = Arc::new(OsVfs::new());
    let engine = Arc::new(StorageEngine::open(vfs, &db_path, cfg, [0x77; 16]).unwrap());

    // Seed initial root
    {
        let mut wtx = engine.begin_write().unwrap();
        let mut tree = BTree::create_empty(&mut wtx, TreeKind::Collection).unwrap();
        tree.insert(&mut wtx, &0u64.to_be_bytes(), b"seed").unwrap();
        wtx.set_catalog_root_page_id(tree.root());
        wtx.commit().unwrap();
    }

    // Pin a long-lived reader at snapshot 0
    let pinned_reader = engine.begin_read().unwrap();
    let pinned_root = pinned_reader.catalog_root_page_id();

    let barrier = Arc::new(Barrier::new(5));
    let mut handles = Vec::new();

    // 4 concurrent reader threads continuously scanning latest snapshot
    for _ in 0..4 {
        let eng = Arc::clone(&engine);
        let bar = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            bar.wait();
            for _ in 0..40 {
                let rtx = eng.begin_read().unwrap();
                let tree = BTree::open(rtx.catalog_root_page_id(), TreeKind::Collection);
                let rows = tree.scan_all(&rtx).unwrap();
                assert!(!rows.is_empty());
            }
        }));
    }

    // Writer thread commits 30 sequential transactions + passive checkpoints
    barrier.wait();
    for i in 1..=30u64 {
        let mut wtx = engine.begin_write().unwrap();
        let mut tree = BTree::open(wtx.catalog_root_page_id(), TreeKind::Collection);
        tree.insert(&mut wtx, &i.to_be_bytes(), format!("v{i}").as_bytes())
            .unwrap();
        wtx.set_catalog_root_page_id(tree.root());
        wtx.commit().unwrap();
        if i % 10 == 0 {
            let ckpt = engine.checkpoint(true).unwrap();
            // Pinned reader holds WAL reset back while open!
            assert!(!ckpt.wal_reset);
        }
    }

    for h in handles {
        h.join().unwrap();
    }

    // Pinned reader must STILL see only the original 1 seed row!
    let pinned_tree = BTree::open(pinned_root, TreeKind::Collection);
    assert_eq!(pinned_tree.scan_all(&pinned_reader).unwrap().len(), 1);
    drop(pinned_reader);

    // Once pinned reader drops, checkpoint(true) resets the WAL cleanly!
    let ckpt = engine.checkpoint(true).unwrap();
    assert!(ckpt.wal_reset);
}
