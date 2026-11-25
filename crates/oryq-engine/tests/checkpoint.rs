mod support;

use std::path::Path;
use std::sync::Arc;

use oryq_engine::{
    wal_path_for, BTree, EngineConfig, PageSize, Pager, StorageEngine, StorageIo, TreeKind, TxId,
};
use support::fault_io::{FaultVfs, FaultVfsIo};

const TEST_UUID: [u8; 16] = [0x22; 16];

#[test]
fn sealed_unseal_passive_checkpoint_and_full_wal_reset() {
    let vfs = FaultVfs::new();
    let io = Arc::new(FaultVfsIo(Arc::clone(&vfs)));
    let path = Path::new("/mem/checkpoint.oryq");
    let wal_path = wal_path_for(path);
    let cfg = EngineConfig {
        page_size: PageSize::Size4096,
        ..EngineConfig::default()
    };

    {
        let engine = StorageEngine::open(io.clone(), path, cfg.clone(), TEST_UUID).unwrap();
        assert!(engine.is_sealed().unwrap());
        assert!(!io.exists(&wal_path));

        // Tx 1 unseals database and creates WAL
        {
            let mut w = engine.begin_write().unwrap();
            let mut tree = BTree::open(w.catalog_root_page_id(), TreeKind::Collection);
            tree.insert(&mut w, b"k1", b"v1").unwrap();
            w.set_catalog_root_page_id(tree.root());
            w.commit().unwrap();
        }
        assert!(!engine.is_sealed().unwrap());
        assert!(io.exists(&wal_path));

        // Hold reader at Tx 1 while Tx 2 commits
        let r1 = engine.begin_read().unwrap();
        {
            let mut w = engine.begin_write().unwrap();
            let mut tree = BTree::open(w.catalog_root_page_id(), TreeKind::Collection);
            tree.insert(&mut w, b"k2", b"v2").unwrap();
            w.set_catalog_root_page_id(tree.root());
            w.commit().unwrap();
        }

        // Checkpoint with active reader -> passive only (copies through Tx 1, no WAL reset)
        let passive = engine.checkpoint(true).unwrap();
        assert_eq!(passive.checkpointed_tx_id, TxId::new(1));
        assert!(!passive.wal_reset);
        assert!(!engine.is_sealed().unwrap());

        // Reader still sees Tx 1 cleanly
        let t1 = BTree::open(r1.catalog_root_page_id(), TreeKind::Collection);
        assert_eq!(t1.get(&r1, b"k1").unwrap().as_deref(), Some(&b"v1"[..]));
        assert_eq!(t1.get(&r1, b"k2").unwrap(), None);
        drop(r1);

        // Now zero active readers -> checkpoint(true) copies Tx 2, resets WAL to 64B, and seals!
        let full = engine.checkpoint(true).unwrap();
        assert_eq!(full.checkpointed_tx_id, TxId::new(2));
        assert!(full.wal_reset);
        assert!(engine.is_sealed().unwrap());
    }

    // Remove .oryq-wal while sealed -> standalone .oryq opens cleanly with all data!
    io.remove_file(&wal_path).unwrap();
    {
        let engine = StorageEngine::open(io, path, cfg, TEST_UUID).unwrap();
        assert!(engine.is_sealed().unwrap());
        let r = engine.begin_read().unwrap();
        let tree = BTree::open(r.catalog_root_page_id(), TreeKind::Collection);
        assert_eq!(tree.get(&r, b"k1").unwrap().as_deref(), Some(&b"v1"[..]));
        assert_eq!(tree.get(&r, b"k2").unwrap().as_deref(), Some(&b"v2"[..]));
    }
}

#[test]
fn interrupted_reset_intent_completes_on_reopen() {
    let vfs = FaultVfs::new();
    let io = Arc::new(FaultVfsIo(Arc::clone(&vfs)));
    let path = Path::new("/mem/reset_intent.oryq");
    let wal_path = wal_path_for(path);
    let cfg = EngineConfig {
        page_size: PageSize::Size4096,
        ..EngineConfig::default()
    };

    // Commit Tx 1 and checkpoint it into `.oryq`
    {
        let engine = StorageEngine::open(io.clone(), path, cfg.clone(), TEST_UUID).unwrap();
        let mut w = engine.begin_write().unwrap();
        let mut tree = BTree::open(w.catalog_root_page_id(), TreeKind::Collection);
        tree.insert(&mut w, b"persist", b"yes").unwrap();
        w.set_catalog_root_page_id(tree.root());
        w.commit().unwrap();
        let out = engine.checkpoint(false).unwrap();
        assert_eq!(out.checkpointed_tx_id, TxId::new(1));
    }

    // Simulate a crash after publishing reset_intent_seq = 1 in Page 0, with a garbage/corrupt WAL
    {
        let mut pager = Pager::open_existing(io.clone(), path, &cfg).unwrap();
        pager.header_mut().reset_intent_seq = 1;
        pager.commit_header().unwrap();
        pager.commit_header().unwrap();
    }
    vfs.corrupt_bytes(&wal_path, 0, &[0xDE, 0xAD, 0xBE, 0xEF]);

    // Reopen detects reset_intent_seq = 1, completes reset cleanly, and seals the database!
    {
        let engine = StorageEngine::open(io.clone(), path, cfg.clone(), TEST_UUID).unwrap();
        assert!(engine.is_sealed().unwrap());
        let r = engine.begin_read().unwrap();
        let tree = BTree::open(r.catalog_root_page_id(), TreeKind::Collection);
        assert_eq!(
            tree.get(&r, b"persist").unwrap().as_deref(),
            Some(&b"yes"[..])
        );
    }

    // Second reopen confirms reset_intent_seq was durably cleared
    {
        let pager = Pager::open_existing(io, path, &cfg).unwrap();
        assert_eq!(pager.header().reset_intent_seq, 0);
        assert_eq!(pager.header().checkpointed_wal_seq, 1);
        assert!(!pager.header().wal_required);
    }
}

#[test]
fn checkpointed_main_file_reads_hit_page_cache_and_refresh_on_update() {
    let vfs = FaultVfs::new();
    let io = Arc::new(FaultVfsIo(Arc::clone(&vfs)));
    let path = Path::new("/mem/cache_main_hits.oryq");
    let cfg = EngineConfig {
        page_size: PageSize::Size4096,
        ..EngineConfig::default()
    };

    let engine = StorageEngine::open(io, path, cfg, TEST_UUID).unwrap();

    // Commit Tx 1 and checkpoint + reset WAL so pages live solely in the main `.oryq` file.
    {
        let mut w = engine.begin_write().unwrap();
        let mut tree = BTree::open(w.catalog_root_page_id(), TreeKind::Collection);
        tree.insert(&mut w, b"alpha", b"v1").unwrap();
        w.set_catalog_root_page_id(tree.root());
        w.commit().unwrap();
    }
    let ckpt = engine.checkpoint(true).unwrap();
    assert!(ckpt.wal_reset);

    // Warm main_page_lsns / PageCache on first read, then verify subsequent reads hit PageCache
    // with zero additional misses.
    {
        let r = engine.begin_read().unwrap();
        let tree = BTree::open(r.catalog_root_page_id(), TreeKind::Collection);
        assert_eq!(tree.get(&r, b"alpha").unwrap().as_deref(), Some(&b"v1"[..]));
    }
    let before = engine.cache_stats();
    for _ in 0..10 {
        let r = engine.begin_read().unwrap();
        let tree = BTree::open(r.catalog_root_page_id(), TreeKind::Collection);
        assert_eq!(tree.get(&r, b"alpha").unwrap().as_deref(), Some(&b"v1"[..]));
    }
    let after = engine.cache_stats();
    assert!(
        after.hits >= before.hits + 10,
        "expected >= 10 PageCache hits on checkpointed reads, before={} after={}",
        before.hits,
        after.hits
    );
    assert_eq!(
        after.misses, before.misses,
        "expected zero PageCache misses on repeated checkpointed reads"
    );

    // Update the same key in Tx 2, checkpoint + reset WAL again, and verify reads see the new value
    // (proving main_page_lsns refreshes cleanly and never serves stale cached pages).
    {
        let mut w = engine.begin_write().unwrap();
        let mut tree = BTree::open(w.catalog_root_page_id(), TreeKind::Collection);
        tree.insert(&mut w, b"alpha", b"v2").unwrap();
        w.set_catalog_root_page_id(tree.root());
        w.commit().unwrap();
    }
    assert!(engine.checkpoint(true).unwrap().wal_reset);
    {
        let r = engine.begin_read().unwrap();
        let tree = BTree::open(r.catalog_root_page_id(), TreeKind::Collection);
        assert_eq!(tree.get(&r, b"alpha").unwrap().as_deref(), Some(&b"v2"[..]));
    }
}
