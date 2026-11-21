//! Crash-recovery, page-corruption, and multi-threaded snapshot/writer stress tests for `oryq-engine`.

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_engine::{wal_path_for, BTree, EngineConfig, OsVfs, PageSize, StorageEngine, TreeKind};
use oryq_types::ErrorCategory;

fn unique_temp_dir(label: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "oryqdb_engine_stress_{label}_{}_{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn repeated_crash_truncated_wal_tail_preserves_all_committed_txns() {
    let dir = unique_temp_dir("wal_crash_cycles");
    let db_path = dir.join("crash.oryq");
    let wal_path = wal_path_for(&db_path);
    let cfg = EngineConfig {
        page_size: PageSize::Size4096,
        ..EngineConfig::default()
    };

    let mut root_page_id = None;
    for cycle in 1..=15u64 {
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

            if cycle % 5 == 0 {
                engine.checkpoint(true).unwrap();
            }
        }

        // Simulate a torn / garbage uncommitted tail appended to WAL right before crash
        if wal_path.exists() {
            let mut f = OpenOptions::new().append(true).open(&wal_path).unwrap();
            f.write_all(&[0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03])
                .unwrap();
            f.sync_all().unwrap();
        }
    }

    // Final reopen verifies all 15 cycles intact
    {
        let vfs = Arc::new(OsVfs::new());
        let engine = StorageEngine::open(vfs, &db_path, cfg, [0x42; 16]).unwrap();
        let rtx = engine.begin_read().unwrap();
        let tree = BTree::open(root_page_id.unwrap(), TreeKind::Collection);
        assert_eq!(tree.scan_all(&rtx).unwrap().len(), 15);
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn page_bitflip_detected_as_corruption_without_panic() {
    let dir = unique_temp_dir("bitflip");
    let db_path = dir.join("flip.oryq");
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
    {
        let offset = root.raw() * 4096 + 128;
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&db_path)
            .unwrap();
        f.seek(SeekFrom::Start(offset)).unwrap();
        let mut byte = [0u8; 1];
        f.read_exact(&mut byte).unwrap();
        byte[0] ^= 0xFF;
        f.seek(SeekFrom::Start(offset)).unwrap();
        f.write_all(&byte).unwrap();
        f.sync_all().unwrap();
    }

    // Reading the corrupted page must return Corruption, never panic
    {
        let vfs = Arc::new(OsVfs::new());
        let engine = StorageEngine::open(vfs, &db_path, cfg, [0x55; 16]).unwrap();
        let rtx = engine.begin_read().unwrap();
        let tree = BTree::open(root, TreeKind::Collection);
        let err = tree.get(&rtx, &1u64.to_be_bytes()).unwrap_err();
        assert_eq!(err.category(), ErrorCategory::Corruption);
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn concurrent_readers_with_pinned_snapshot_and_writer_checkpoint_stress() {
    let dir = unique_temp_dir("mvcc_stress");
    let db_path = dir.join("mvcc.oryq");
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

    drop(engine);
    let _ = std::fs::remove_dir_all(&dir);
}
