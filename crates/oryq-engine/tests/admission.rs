mod support;

use std::path::Path;
use std::sync::Arc;

use oryq_engine::{BTree, EngineConfig, PageSize, StorageEngine, TreeKind, TxId};
use oryq_types::ErrorCategory;
use support::fault_io::{FaultVfs, FaultVfsIo};

const TEST_UUID: [u8; 16] = [0x11; 16];

#[test]
fn snapshot_isolation_nested_readers_and_writer_exclusion() {
    let vfs = FaultVfs::new();
    let io = Arc::new(FaultVfsIo(Arc::clone(&vfs)));
    let path = Path::new("/mem/mvcc_iso.oryq");
    let cfg = EngineConfig {
        page_size: PageSize::Size4096,
        max_readers: 2,
        ..EngineConfig::default()
    };

    let engine = StorageEngine::open(io, path, cfg, TEST_UUID).unwrap();

    // Tx 1: insert k1 -> v1
    {
        let mut w = engine.begin_write().unwrap();
        let mut tree = BTree::open(w.catalog_root_page_id(), TreeKind::Collection);
        tree.insert(&mut w, b"k1", b"v1").unwrap();
        w.set_catalog_root_page_id(tree.root());
        let receipt = w.commit().unwrap();
        assert_eq!(receipt.tx_id, TxId::new(1));
    }

    // Two nested readers on the same thread pinned at snapshot 1
    let r1 = engine.begin_read().unwrap();
    let r2 = engine.begin_read().unwrap();
    assert_eq!(r1.snapshot_id(), TxId::new(1));
    assert_eq!(r2.snapshot_id(), TxId::new(1));

    // 3rd reader exceeds max_readers=2 -> Busy
    assert_eq!(
        engine.begin_read().unwrap_err().category(),
        ErrorCategory::Busy
    );

    // Concurrent writer commits Tx 2: update k1 -> v2, insert k2 -> v2
    {
        let mut w = engine.begin_write().unwrap();
        // Second concurrent try_begin_write must fail with Busy
        assert_eq!(
            engine.try_begin_write().unwrap_err().category(),
            ErrorCategory::Busy
        );
        let mut tree = BTree::open(w.catalog_root_page_id(), TreeKind::Collection);
        tree.insert(&mut w, b"k1", b"v2").unwrap();
        tree.insert(&mut w, b"k2", b"v2").unwrap();
        w.set_catalog_root_page_id(tree.root());
        let receipt = w.commit().unwrap();
        assert_eq!(receipt.tx_id, TxId::new(2));
    }

    // Both old snapshots still see Tx 1 state
    let t1 = BTree::open(r1.catalog_root_page_id(), TreeKind::Collection);
    assert_eq!(t1.get(&r1, b"k1").unwrap().as_deref(), Some(&b"v1"[..]));
    assert_eq!(t1.get(&r1, b"k2").unwrap(), None);

    let t2 = BTree::open(r2.catalog_root_page_id(), TreeKind::Collection);
    assert_eq!(t2.get(&r2, b"k1").unwrap().as_deref(), Some(&b"v1"[..]));

    // Drop one reader and open a new reader at snapshot 2
    drop(r2);
    let r3 = engine.begin_read().unwrap();
    assert_eq!(r3.snapshot_id(), TxId::new(2));
    let t3 = BTree::open(r3.catalog_root_page_id(), TreeKind::Collection);
    assert_eq!(t3.get(&r3, b"k1").unwrap().as_deref(), Some(&b"v2"[..]));
    assert_eq!(t3.get(&r3, b"k2").unwrap().as_deref(), Some(&b"v2"[..]));
}

#[test]
fn nested_savepoints_allocation_rollback_and_drop_rollback() {
    let vfs = FaultVfs::new();
    let io = Arc::new(FaultVfsIo(Arc::clone(&vfs)));
    let path = Path::new("/mem/savepoints.oryq");
    let cfg = EngineConfig {
        page_size: PageSize::Size4096,
        ..EngineConfig::default()
    };

    let engine = StorageEngine::open(io, path, cfg, TEST_UUID).unwrap();

    {
        let mut w = engine.begin_write().unwrap();
        let mut tree = BTree::open(w.catalog_root_page_id(), TreeKind::Collection);
        tree.insert(&mut w, b"a", b"keep-a").unwrap();
        w.set_catalog_root_page_id(tree.root());

        let sp1 = w.savepoint();
        tree.insert(&mut w, b"b", b"keep-b").unwrap();
        w.set_catalog_root_page_id(tree.root());

        let pages_before_sp2 = w.total_page_count();
        let sp2 = w.savepoint();
        // Insert enough large entries to force page allocation + split inside sp2
        for i in 0..40u32 {
            let k = format!("rollback-{i:03}").into_bytes();
            let v = vec![0xCCu8; 200];
            tree.insert(&mut w, &k, &v).unwrap();
        }
        w.set_catalog_root_page_id(tree.root());
        assert!(w.total_page_count() > pages_before_sp2);

        // Roll back sp2 -> allocations and keys vanish cleanly
        w.rollback_to(sp2).unwrap();
        assert_eq!(w.total_page_count(), pages_before_sp2);
        w.release_savepoint(sp1).unwrap();
        w.commit().unwrap();
    }

    // Verify only "a" and "b" committed
    {
        let r = engine.begin_read().unwrap();
        let tree = BTree::open(r.catalog_root_page_id(), TreeKind::Collection);
        assert_eq!(tree.get(&r, b"a").unwrap().as_deref(), Some(&b"keep-a"[..]));
        assert_eq!(tree.get(&r, b"b").unwrap().as_deref(), Some(&b"keep-b"[..]));
        assert_eq!(tree.get(&r, b"rollback-000").unwrap(), None);
    }

    // Drop without commit automatically rolls back
    {
        let mut w = engine.begin_write().unwrap();
        let mut tree = BTree::open(w.catalog_root_page_id(), TreeKind::Collection);
        tree.insert(&mut w, b"uncommitted", b"nope").unwrap();
        w.set_catalog_root_page_id(tree.root());
        // drop(w) without commit
    }

    {
        let r = engine.begin_read().unwrap();
        let tree = BTree::open(r.catalog_root_page_id(), TreeKind::Collection);
        assert_eq!(tree.get(&r, b"uncommitted").unwrap(), None);
        assert_eq!(r.snapshot_id(), TxId::new(1));
    }
}

#[test]
fn hard_wal_cap_rejects_commit_without_poisoning() {
    let vfs = FaultVfs::new();
    let io = Arc::new(FaultVfsIo(Arc::clone(&vfs)));
    let path = Path::new("/mem/wal_cap.oryq");
    let cfg = EngineConfig {
        page_size: PageSize::Size4096,
        max_wal_hard_bytes: 2048, // smaller than a single 4096B FullPageFrame
        ..EngineConfig::default()
    };

    let engine = StorageEngine::open(io, path, cfg, TEST_UUID).unwrap();
    let mut w = engine.begin_write().unwrap();
    let mut tree = BTree::open(w.catalog_root_page_id(), TreeKind::Collection);
    tree.insert(&mut w, b"k", b"v").unwrap();
    w.set_catalog_root_page_id(tree.root());

    let err = w.commit().unwrap_err();
    assert_eq!(err.category(), ErrorCategory::QuotaExceeded);
    assert!(!engine.is_poisoned());
    assert_eq!(engine.latest_committed_tx_id().unwrap(), TxId::INITIAL);
}
