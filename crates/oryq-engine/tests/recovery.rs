mod support;

use std::path::Path;
use std::sync::Arc;

use oryq_engine::{
    wal_path_for, BTree, EngineConfig, OpenMode, PageSize, StorageEngine, StorageIo, TreeKind, TxId,
};
use oryq_types::ErrorCategory;
use support::fault_io::{FaultVfs, FaultVfsIo};

const TEST_UUID: [u8; 16] = [0x33; 16];

#[test]
fn wal_recovery_truncates_incomplete_tail_and_is_restartable_twice() {
    let vfs = FaultVfs::new();
    let io = Arc::new(FaultVfsIo(Arc::clone(&vfs)));
    let path = Path::new("/mem/recover_tail.oryq");
    let wal_path = wal_path_for(path);
    let cfg = EngineConfig {
        page_size: PageSize::Size4096,
        ..EngineConfig::default()
    };

    // Commit 3 transactions without checkpointing
    {
        let engine = StorageEngine::open(io.clone(), path, cfg.clone(), TEST_UUID).unwrap();
        for i in 1..=3u32 {
            let mut w = engine.begin_write().unwrap();
            let mut tree = BTree::open(w.catalog_root_page_id(), TreeKind::Collection);
            let k = format!("tx-{i}").into_bytes();
            let v = format!("val-{i}").into_bytes();
            tree.insert(&mut w, &k, &v).unwrap();
            w.set_catalog_root_page_id(tree.root());
            w.commit().unwrap();
        }
    }

    // Append a torn trailing partial frame header (20 bytes) simulating mid-write crash
    {
        let wf = io.open(&wal_path, OpenMode::READ_WRITE).unwrap();
        let end = wf.len().unwrap();
        wf.write_all_at(&[0xAAu8; 20], end).unwrap();
    }

    // Recover twice in a row (restartability invariant)
    for _ in 0..2 {
        let engine = StorageEngine::open(io.clone(), path, cfg.clone(), TEST_UUID).unwrap();
        assert_eq!(engine.latest_committed_tx_id().unwrap(), TxId::new(3));
        let r = engine.begin_read().unwrap();
        let tree = BTree::open(r.catalog_root_page_id(), TreeKind::Collection);
        for i in 1..=3u32 {
            let k = format!("tx-{i}").into_bytes();
            let v = format!("val-{i}").into_bytes();
            assert_eq!(tree.get(&r, &k).unwrap().as_deref(), Some(v.as_slice()));
        }
    }
}

#[test]
fn complete_corrupt_wal_frame_and_missing_unsealed_wal_fail_closed() {
    let vfs = FaultVfs::new();
    let io = Arc::new(FaultVfsIo(Arc::clone(&vfs)));
    let path = Path::new("/mem/corrupt_wal.oryq");
    let wal_path = wal_path_for(path);
    let cfg = EngineConfig {
        page_size: PageSize::Size4096,
        ..EngineConfig::default()
    };

    {
        let engine = StorageEngine::open(io.clone(), path, cfg.clone(), TEST_UUID).unwrap();
        let mut w = engine.begin_write().unwrap();
        let mut tree = BTree::open(w.catalog_root_page_id(), TreeKind::Collection);
        tree.insert(&mut w, b"k", b"v").unwrap();
        w.set_catalog_root_page_id(tree.root());
        w.commit().unwrap();
    }

    // Flip 1 bit inside the committed WAL frame payload -> fails closed with Corruption
    let orig = vfs.snapshot_bytes(&wal_path)[64 + 48 + 16];
    vfs.corrupt_bytes(&wal_path, 64 + 48 + 16, &[orig ^ 0x01]);
    let err = StorageEngine::open(io.clone(), path, cfg.clone(), TEST_UUID).unwrap_err();
    assert_eq!(err.category(), ErrorCategory::Corruption);

    // Deleting WAL when wal_required=1 -> fails closed with Corruption
    io.remove_file(&wal_path).unwrap();
    let err2 = StorageEngine::open(io, path, cfg, TEST_UUID).unwrap_err();
    assert_eq!(err2.category(), ErrorCategory::Corruption);
}

#[test]
fn uncertain_sync_failure_poisons_engine_until_reopen() {
    let vfs = FaultVfs::new();
    let io = Arc::new(FaultVfsIo(Arc::clone(&vfs)));
    let path = Path::new("/mem/uncertain_sync.oryq");
    let cfg = EngineConfig {
        page_size: PageSize::Size4096,
        ..EngineConfig::default()
    };

    {
        let engine = StorageEngine::open(io.clone(), path, cfg.clone(), TEST_UUID).unwrap();
        // Commit Tx 1 cleanly so database is unsealed
        {
            let mut w = engine.begin_write().unwrap();
            let mut tree = BTree::open(w.catalog_root_page_id(), TreeKind::Collection);
            tree.insert(&mut w, b"tx1", b"ok").unwrap();
            w.set_catalog_root_page_id(tree.root());
            w.commit().unwrap();
        }

        // Inject sync failure on Tx 2 commit -> CommitOutcomeUnknown + poisoned engine
        {
            let mut w = engine.begin_write().unwrap();
            let mut tree = BTree::open(w.catalog_root_page_id(), TreeKind::Collection);
            tree.insert(&mut w, b"tx2", b"uncertain").unwrap();
            w.set_catalog_root_page_id(tree.root());
            vfs.fail_next_sync();
            let err = w.commit().unwrap_err();
            assert_eq!(err.category(), ErrorCategory::CommitOutcomeUnknown);
        }

        assert!(engine.is_poisoned());
        assert_eq!(
            engine.begin_read().unwrap_err().category(),
            ErrorCategory::CommitOutcomeUnknown
        );
        assert_eq!(
            engine.begin_write().unwrap_err().category(),
            ErrorCategory::CommitOutcomeUnknown
        );
    }

    // Reopening recovers cleanly (and because the WAL bytes were written before sync failed,
    // recovery retains valid committed state without corruption!)
    {
        let engine = StorageEngine::open(io, path, cfg, TEST_UUID).unwrap();
        assert!(!engine.is_poisoned());
        let r = engine.begin_read().unwrap();
        let tree = BTree::open(r.catalog_root_page_id(), TreeKind::Collection);
        assert_eq!(tree.get(&r, b"tx1").unwrap().as_deref(), Some(&b"ok"[..]));
    }
}
