mod support;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use oryq_engine::{
    BTree, EngineConfig, FreelistManager, PageCache, PageId, PageSize, PageVersionEntry,
    PageVersionLocation, Pager, PagerPageStore, TreeKind, TxId, VersionIndex,
};
use oryq_types::ErrorCategory;
use support::fault_io::{FaultVfs, FaultVfsIo};

const TEST_UUID: [u8; 16] = [0xAB; 16];

#[test]
fn page_cache_pin_aware_eviction_and_quota_enforcement() {
    let page_bytes = PageSize::Size4096.bytes();
    let cache = PageCache::with_limit(page_bytes * 2);

    let p1 = cache
        .insert(
            PageId::new(1),
            TxId::new(1),
            Arc::from(vec![1u8; page_bytes]),
        )
        .unwrap();
    let p2 = cache
        .insert(
            PageId::new(2),
            TxId::new(1),
            Arc::from(vec![2u8; page_bytes]),
        )
        .unwrap();

    // Both pages are pinned: inserting a 3rd page must fail with QuotaExceeded.
    let err = cache
        .insert(
            PageId::new(3),
            TxId::new(1),
            Arc::from(vec![3u8; page_bytes]),
        )
        .unwrap_err();
    assert_eq!(err.category(), ErrorCategory::QuotaExceeded);
    assert_eq!(cache.stats().entries, 2);
    assert_eq!(cache.stats().used_bytes, page_bytes * 2);

    // Touch p1 so p2 is LRU, then unpin p2.
    let p1_hit = cache.get(PageId::new(1), TxId::new(1)).unwrap();
    assert_eq!(p1_hit.as_bytes()[0], 1);
    drop(p1_hit);
    drop(p2);

    // Now inserting p3 evicts unpinned p2 while preserving pinned p1.
    let p3 = cache
        .insert(
            PageId::new(3),
            TxId::new(1),
            Arc::from(vec![3u8; page_bytes]),
        )
        .unwrap();
    assert_eq!(p3.as_bytes()[0], 3);
    assert!(cache.get(PageId::new(2), TxId::new(1)).is_none());
    assert!(cache.get(PageId::new(1), TxId::new(1)).is_some());

    let stats = cache.stats();
    assert_eq!(stats.entries, 2);
    assert_eq!(stats.used_bytes, page_bytes * 2);
    assert_eq!(stats.evictions, 1);
    assert!(stats.hits >= 2);
    assert!(stats.misses >= 1);

    drop(p1);
    drop(p3);
    assert_eq!(cache.evict_all_unpinned(), 2);
    assert_eq!(cache.stats().used_bytes, 0);
}

#[test]
fn version_index_snapshot_resolution_pruning_and_quota() {
    let mut index = VersionIndex::new(4);
    let pid = PageId::new(5);

    for (tx, frame) in [(1, 10), (3, 20), (7, 30), (10, 40)] {
        index
            .record_version(
                pid,
                PageVersionEntry {
                    tx_id: TxId::new(tx),
                    location: PageVersionLocation::WalFrame {
                        frame_seq: frame,
                        file_offset: frame * 100,
                    },
                },
            )
            .unwrap();
    }

    // Quota cap reached (4/4)
    let err = index
        .record_version(
            PageId::new(6),
            PageVersionEntry {
                tx_id: TxId::new(1),
                location: PageVersionLocation::MainFile,
            },
        )
        .unwrap_err();
    assert_eq!(err.category(), ErrorCategory::QuotaExceeded);

    // Snapshot visibility checks
    assert_eq!(index.resolve_for_snapshot(pid, TxId::new(0)), None);
    assert_eq!(
        index.resolve_for_snapshot(pid, TxId::new(1)).unwrap().tx_id,
        TxId::new(1)
    );
    assert_eq!(
        index.resolve_for_snapshot(pid, TxId::new(2)).unwrap().tx_id,
        TxId::new(1)
    );
    assert_eq!(
        index.resolve_for_snapshot(pid, TxId::new(6)).unwrap().tx_id,
        TxId::new(3)
    );
    assert_eq!(
        index.resolve_for_snapshot(pid, TxId::new(7)).unwrap().tx_id,
        TxId::new(7)
    );
    assert_eq!(
        index
            .resolve_for_snapshot(pid, TxId::new(99))
            .unwrap()
            .tx_id,
        TxId::new(10)
    );

    // Prune below min_active_snapshot = 7 -> drops tx 1 and tx 3, keeps tx 7 and tx 10
    let pruned = index.prune_below(TxId::new(7));
    assert_eq!(pruned, 2);
    assert_eq!(index.total_entries(), 2);
    assert_eq!(
        index.resolve_for_snapshot(pid, TxId::new(7)).unwrap().tx_id,
        TxId::new(7)
    );

    // Non-monotonic tx_id rejected
    let err = index
        .record_version(
            pid,
            PageVersionEntry {
                tx_id: TxId::new(10),
                location: PageVersionLocation::MainFile,
            },
        )
        .unwrap_err();
    assert_eq!(err.category(), ErrorCategory::InvalidArgument);
}

#[test]
fn btree_multi_level_splits_upserts_scans_deletes_and_root_collapse() {
    let vfs = FaultVfs::new();
    let io = Arc::new(FaultVfsIo(Arc::clone(&vfs)));
    let path = Path::new("/mem/btree_full.oryq");
    let cfg = EngineConfig {
        page_size: PageSize::Size4096,
        ..EngineConfig::default()
    };
    let cache = PageCache::with_limit(PageSize::Size4096.bytes() * 32);

    let mut oracle: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    let mut keys: Vec<u32> = (0..400u32).collect();
    // Deterministic Fisher-Yates shuffle (LCG) so inserts exercise non-sequential splits
    let mut seed: u64 = 0xDEAD_BEEF_1234_5678;
    for i in (1..keys.len()).rev() {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let j = (seed as usize) % (i + 1);
        keys.swap(i, j);
    }

    let root_after_inserts = {
        let mut pager = Pager::create_new(io.clone(), path, &cfg, TEST_UUID).unwrap();
        let mut store = PagerPageStore::new(&mut pager, Some(&cache), TxId::new(1), TxId::new(1));
        let mut tree = BTree::create_empty(&mut store, TreeKind::Collection).unwrap();
        let initial_root = tree.root();

        for &k in &keys {
            let kb = format!("key-{k:05}").into_bytes();
            let vb = vec![(k % 251) as u8; 128];
            assert!(tree.insert(&mut store, &kb, &vb).unwrap().is_none());
            oracle.insert(kb, vb);
        }

        // 400 * ~155B = ~62 KiB on 4 KiB pages -> guaranteed multi-level root split
        assert_ne!(tree.root(), initial_root);

        // Upsert 100 keys with new values
        for &k in &keys[..100] {
            let kb = format!("key-{k:05}").into_bytes();
            let new_vb = vec![((k + 77) % 251) as u8; 160];
            let prev = tree.insert(&mut store, &kb, &new_vb).unwrap();
            assert_eq!(prev, oracle.insert(kb, new_vb));
        }

        // Verify point lookups, forward scan, range scan, and reverse scan
        for (k, v) in &oracle {
            assert_eq!(tree.get(&store, k).unwrap().as_deref(), Some(v.as_slice()));
        }

        let scanned = tree.scan_all(&store).unwrap();
        let expected: Vec<(Vec<u8>, Vec<u8>)> =
            oracle.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        assert_eq!(scanned, expected);

        let range = tree.scan_range(&store, b"key-00100", b"key-00150").unwrap();
        assert_eq!(range.len(), 50);
        assert_eq!(range[0].0, b"key-00100");
        assert_eq!(range[49].0, b"key-00149");

        // Reverse scan via BTreeCursor::seek_last / step_prev
        let mut rev = Vec::new();
        let mut cur = tree.cursor(&store);
        assert!(cur.seek_last().unwrap());
        while let Some((k, v)) = cur.current() {
            rev.push((k.to_vec(), v.to_vec()));
            if !cur.step_prev().unwrap() {
                break;
            }
        }
        rev.reverse();
        assert_eq!(rev, expected);

        let root = tree.root();
        store.pager_mut().commit_header().unwrap();
        root
    };

    // Reopen from disk, verify persistence, then delete all 400 keys down to empty root
    let final_root = {
        let mut pager = Pager::open_existing(io.clone(), path, &cfg).unwrap();

        let mut store = PagerPageStore::new(&mut pager, Some(&cache), TxId::new(2), TxId::new(2));
        let mut tree = BTree::open(root_after_inserts, TreeKind::Collection);

        // Verify persisted tree matches oracle
        assert_eq!(tree.scan_all(&store).unwrap().len(), 400);

        // Delete 395 keys -> triggers sibling borrows, merges, freelist retirement & root collapse
        for &k in &keys[..395] {
            let kb = format!("key-{k:05}").into_bytes();
            let removed = tree.delete(&mut store, &kb).unwrap();
            assert_eq!(removed, oracle.remove(&kb));
        }

        // Non-existent delete returns None
        assert!(tree.delete(&mut store, b"key-99999").unwrap().is_none());

        let remaining = tree.scan_all(&store).unwrap();
        let expected_rem: Vec<(Vec<u8>, Vec<u8>)> =
            oracle.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        assert_eq!(remaining, expected_rem);
        assert_eq!(remaining.len(), 5);

        // Delete final 5 keys -> tree is completely empty
        for &k in &keys[395..] {
            let kb = format!("key-{k:05}").into_bytes();
            let removed = tree.delete(&mut store, &kb).unwrap();
            assert_eq!(removed, oracle.remove(&kb));
        }
        assert!(tree.scan_all(&store).unwrap().is_empty());
        let mut cur = tree.cursor(&store);
        assert!(!cur.seek_first().unwrap());
        assert!(!cur.seek_last().unwrap());

        let final_root = tree.root();
        assert!(FreelistManager::validate_chain(store.pager()).unwrap() > 0);
        store.pager_mut().commit_header().unwrap();
        final_root
    };

    // Reopen empty tree and verify freelist integrity
    {
        let mut pager = Pager::open_existing(io, path, &cfg).unwrap();
        assert!(FreelistManager::validate_chain(&pager).unwrap() > 0);
        let store = PagerPageStore::new(&mut pager, None, TxId::new(3), TxId::new(3));
        let tree = BTree::open(final_root, TreeKind::Collection);
        assert!(tree.scan_all(&store).unwrap().is_empty());
    }
}

#[test]
fn btree_rejects_oversized_single_entry() {
    let vfs = FaultVfs::new();
    let io = Arc::new(FaultVfsIo(Arc::clone(&vfs)));
    let path = Path::new("/mem/oversized.oryq");
    let cfg = EngineConfig {
        page_size: PageSize::Size4096,
        ..EngineConfig::default()
    };

    let mut pager = Pager::create_new(io, path, &cfg, TEST_UUID).unwrap();
    let mut store = PagerPageStore::new(&mut pager, None, TxId::new(1), TxId::new(1));
    let mut tree = BTree::create_empty(&mut store, TreeKind::Collection).unwrap();

    let huge_val = vec![0xAAu8; 5000];
    let err = tree.insert(&mut store, b"k", &huge_val).unwrap_err();
    assert_eq!(err.category(), ErrorCategory::InvalidArgument);
}
