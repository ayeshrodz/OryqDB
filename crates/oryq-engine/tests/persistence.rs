mod support;

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use oryq_engine::{
    CommitPayload, EngineConfig, FreelistManager, HeaderSlot, OverflowHeader, OverflowRef,
    OverflowStore, PageAllocator, PageFlags, PageSize, PageType, Pager, SlotFlags, SlottedPageMut,
    SlottedPageView, StorageHeader, TxId, WalFrameHeader, WalFrameKind, WalHeader, ORYQ_MAGIC,
    STORAGE_HEADER_SIZE, WAL_COMMIT_PAYLOAD_SIZE, WAL_FRAME_HEADER_SIZE, WAL_HEADER_SIZE,
    WAL_MAGIC,
};
use oryq_types::{BudgetDomain, BudgetHandle, ErrorCategory};
use support::fault_io::{FaultVfs, FaultVfsIo};

const TEST_UUID: [u8; 16] = [
    0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xA0, 0xB0, 0xC0, 0xD0, 0xE0, 0xF0, 0x01,
];

#[test]
fn create_reopen_and_dual_shadow_header_failover() {
    for page_size in [PageSize::Size4096, PageSize::Size8192, PageSize::Size16384] {
        let vfs = FaultVfs::new();
        let io = Arc::new(FaultVfsIo(Arc::clone(&vfs)));
        let path = Path::new("/mem/test.oryq");
        let cfg = EngineConfig {
            page_size,
            ..EngineConfig::default()
        };

        {
            let mut pager = Pager::create_new(io.clone(), path, &cfg, TEST_UUID).unwrap();
            assert_eq!(pager.header().header_generation, 1);
            assert_eq!(pager.header().total_page_count, 2);
            assert_eq!(pager.page_size(), page_size);
            assert_eq!(vfs.sync_count.load(Ordering::Relaxed), 1);
            assert_eq!(vfs.dir_sync_count.load(Ordering::Relaxed), 1);

            // Commit gen 2 into Slot B
            pager.header_mut().checkpointed_tx_id = TxId::new(10);
            pager.commit_header().unwrap();
            assert_eq!(pager.active_slot(), HeaderSlot::SlotB);
            assert_eq!(pager.header().header_generation, 2);

            // Commit gen 3 into Slot A
            pager.header_mut().checkpointed_tx_id = TxId::new(20);
            pager.commit_header().unwrap();
            assert_eq!(pager.active_slot(), HeaderSlot::SlotA);
            assert_eq!(pager.header().header_generation, 3);
        }

        // Normal reopen selects Slot A (gen 3)
        {
            let pager = Pager::open_existing(io.clone(), path, &cfg).unwrap();
            assert_eq!(pager.active_slot(), HeaderSlot::SlotA);
            assert_eq!(pager.header().header_generation, 3);
            assert_eq!(pager.header().checkpointed_tx_id, TxId::new(20));
        }

        // Corrupt Slot A -> failover selects Slot B (gen 2)
        vfs.corrupt_bytes(path, 12, &[0xFF, 0xFF]);
        {
            let pager = Pager::open_existing(io.clone(), path, &cfg).unwrap();
            assert_eq!(pager.active_slot(), HeaderSlot::SlotB);
            assert_eq!(pager.header().header_generation, 2);
            assert_eq!(pager.header().checkpointed_tx_id, TxId::new(10));
        }

        // Corrupt Slot B as well -> returns Corruption
        vfs.corrupt_bytes(path, 256 + 12, &[0xFF, 0xFF]);
        let err = Pager::open_existing(io, path, &cfg).unwrap_err();
        assert_eq!(err.category(), ErrorCategory::Corruption);
    }
}

#[test]
fn slotted_page_persistence_compaction_and_bitflip_detection() {
    let vfs = FaultVfs::new();
    let io = Arc::new(FaultVfsIo(Arc::clone(&vfs)));
    let path = Path::new("/mem/slotted.oryq");
    let cfg = EngineConfig::default();

    let pid = {
        let mut pager = Pager::create_new(io.clone(), path, &cfg, TEST_UUID).unwrap();
        let pid = PageAllocator::allocate_page(&mut pager, TxId::new(1), TxId::new(1)).unwrap();
        let mut raw = vec![0u8; cfg.page_size.bytes()];
        let mut page = SlottedPageMut::init(
            &mut raw,
            pid,
            PageType::ColLeaf,
            PageFlags::EMPTY,
            TxId::new(1),
        )
        .unwrap();

        let s0 = page
            .insert_slot(b"hello-slot-0", SlotFlags::EMPTY.with(SlotFlags::BASE_DOC))
            .unwrap();
        let s1 = page
            .insert_slot(b"second-slot-1", SlotFlags::EMPTY.with(SlotFlags::BASE_DOC))
            .unwrap();
        let s2 = page
            .insert_slot(b"third-slot-2", SlotFlags::EMPTY.with(SlotFlags::BASE_DOC))
            .unwrap();
        assert_eq!((s0, s1, s2), (0, 1, 2));

        page.tombstone_slot(s1).unwrap();
        page.compact_payloads();
        assert_eq!(page.header().fragmented_bytes, 0);
        page.finalize();

        pager.write_page(pid, &raw).unwrap();
        pager.commit_header().unwrap();
        pid
    };

    // Reopen and verify persisted slotted page
    {
        let pager = Pager::open_existing(io.clone(), path, &cfg).unwrap();
        let guard = pager.read_page(pid, None).unwrap();
        let view = SlottedPageView::parse(guard.as_slice(), pid).unwrap();
        assert_eq!(view.slot_count(), 3);
        assert_eq!(view.payload(0), Some(&b"hello-slot-0"[..]));
        assert_eq!(view.payload(1), None);
        assert_eq!(view.payload(2), Some(&b"third-slot-2"[..]));
    }

    // Flip 1 bit in the persisted page payload -> read_page rejects with Corruption
    let page_offset = (pid.raw() as usize) * cfg.page_size.bytes();
    let orig = vfs.snapshot_bytes(path)[page_offset + 100];
    vfs.corrupt_bytes(path, page_offset + 100, &[orig ^ 0x01]);
    {
        let pager = Pager::open_existing(io, path, &cfg).unwrap();
        let err = pager.read_page(pid, None).unwrap_err();
        assert_eq!(err.category(), ErrorCategory::Corruption);
    }
}

#[test]
fn overflow_chains_and_horizon_gated_freelist_reuse() {
    let vfs = FaultVfs::new();
    let io = Arc::new(FaultVfsIo(Arc::clone(&vfs)));
    let path = Path::new("/mem/overflow_free.oryq");
    let cfg = EngineConfig::default();

    let big_blob: Vec<u8> = (0..25_000usize).map(|i| (i % 251) as u8).collect();
    let oref = {
        let mut pager = Pager::create_new(io.clone(), path, &cfg, TEST_UUID).unwrap();
        let oref =
            OverflowStore::write_blob(&mut pager, &big_blob, 0x06, TxId::new(1), TxId::new(1))
                .unwrap();
        assert_eq!(oref.total_len as usize, big_blob.len());

        // Round-trip OverflowRef 16-byte encoding
        let mut ref_bytes = [0u8; 16];
        oref.encode(&mut ref_bytes);
        assert_eq!(OverflowRef::decode(&ref_bytes).unwrap(), oref);

        pager.commit_header().unwrap();
        oref
    };

    // Reopen, read with memory budget, then free under TxId(5)
    {
        let mut pager = Pager::open_existing(io.clone(), path, &cfg).unwrap();

        let tiny_budget = BudgetHandle::new(BudgetDomain::Query, 1024);
        let err = OverflowStore::read_blob(&pager, &oref, Some(&tiny_budget)).unwrap_err();
        assert_eq!(err.category(), ErrorCategory::QuotaExceeded);
        assert_eq!(tiny_budget.used_bytes(), 0);

        let ok_budget = BudgetHandle::new(BudgetDomain::Query, 64 * 1024);
        let (read_back, res) = OverflowStore::read_blob(&pager, &oref, Some(&ok_budget)).unwrap();
        assert_eq!(read_back, big_blob);
        assert_eq!(ok_budget.used_bytes(), big_blob.len());
        drop(res);
        assert_eq!(ok_budget.used_bytes(), 0);

        // Free overflow chain under TxId(5) -> 4 pages retired (25000 / 8144 = 4)
        let freed = OverflowStore::free_blob(&mut pager, &oref, TxId::new(5)).unwrap();
        assert_eq!(freed, 4);
        assert_eq!(FreelistManager::validate_chain(&pager).unwrap(), 4);

        // Horizon TxId(5) cannot reuse pages retired at TxId(5) (needs retired_tx_id < min_active)
        let before_total = pager.total_page_count();
        let fresh = PageAllocator::allocate_page(&mut pager, TxId::new(5), TxId::new(6)).unwrap();
        assert_eq!(fresh.raw(), before_total);
        let mut fresh_raw = vec![0u8; cfg.page_size.bytes()];
        SlottedPageMut::init(
            &mut fresh_raw,
            fresh,
            PageType::ColLeaf,
            PageFlags::EMPTY,
            TxId::new(6),
        )
        .unwrap();
        pager.write_page(fresh, &fresh_raw).unwrap();

        // Horizon TxId(6) CAN reuse pages retired at TxId(5)
        let reused = PageAllocator::allocate_page(&mut pager, TxId::new(6), TxId::new(7)).unwrap();
        assert!(reused.raw() < before_total);
        assert_eq!(FreelistManager::validate_chain(&pager).unwrap(), 3);

        pager.commit_header().unwrap();
    }

    // Reopen and validate freelist survived commit
    {
        let pager = Pager::open_existing(io, path, &cfg).unwrap();
        assert_eq!(FreelistManager::validate_chain(&pager).unwrap(), 3);
    }
}

#[test]
fn overflow_cycle_is_rejected() {
    let vfs = FaultVfs::new();
    let io = Arc::new(FaultVfsIo(Arc::clone(&vfs)));
    let path = Path::new("/mem/cycle.oryq");
    let cfg = EngineConfig::default();

    let mut pager = Pager::create_new(io, path, &cfg, TEST_UUID).unwrap();
    let p2 = PageAllocator::allocate_page(&mut pager, TxId::new(1), TxId::new(1)).unwrap();

    // Write an overflow page whose next_page points back to itself (cycle)
    let mut raw = vec![0u8; cfg.page_size.bytes()];
    OverflowHeader::encode_page(&mut raw, p2, TxId::new(1), Some(p2), b"loop").unwrap();
    pager.write_page(p2, &raw).unwrap();

    let oref = OverflowRef {
        first_page: p2,
        total_len: 100,
        original_tag: 0x06,
    };
    let err = OverflowStore::read_blob(&pager, &oref, None).unwrap_err();
    assert_eq!(err.category(), ErrorCategory::Corruption);
}

#[test]
fn golden_binary_format_layout_verification() {
    // 1. StorageHeader exact byte offsets
    let hdr = StorageHeader::new_initial(PageSize::Size8192, TEST_UUID);
    let mut hbuf = [0u8; STORAGE_HEADER_SIZE];
    hdr.encode(&mut hbuf);
    assert_eq!(&hbuf[0..8], &ORYQ_MAGIC);
    assert_eq!(u16::from_le_bytes([hbuf[8], hbuf[9]]), 1);
    assert_eq!(u16::from_le_bytes([hbuf[10], hbuf[11]]), 13); // log2(8192)
    assert_eq!(u64::from_le_bytes(hbuf[12..20].try_into().unwrap()), 1);
    assert_eq!(&hbuf[36..52], &TEST_UUID);
    assert_eq!(u64::from_le_bytes(hbuf[52..60].try_into().unwrap()), 2);
    assert_eq!(u64::from_le_bytes(hbuf[60..68].try_into().unwrap()), 1);
    assert_eq!(hbuf[108], 0);
    assert!(hbuf[109..248].iter().all(|&b| b == 0));
    assert_eq!(StorageHeader::decode(&hbuf).unwrap(), hdr);

    // 2. WAL header + chained frame + CommitPayload exact byte offsets
    let mut whdr = WalHeader::new(PageSize::Size8192, 7, TEST_UUID, 11, 0xCAFE_BABE_1234_5678);
    let mut wbuf = [0u8; WAL_HEADER_SIZE];
    whdr.encode(&mut wbuf);
    assert_eq!(&wbuf[0..8], &WAL_MAGIC);
    assert_eq!(WalHeader::decode(&wbuf).unwrap(), whdr);

    let commit = CommitPayload {
        first_frame_seq: 1,
        mutation_frame_count: 1,
        db_total_pages: 3,
        catalog_root: 1,
        freelist_root: 0,
        freelist_page_count: 0,
    };
    let mut cbuf = [0u8; WAL_COMMIT_PAYLOAD_SIZE];
    commit.encode(&mut cbuf);
    assert_eq!(CommitPayload::decode(&cbuf), commit);

    let mut fhdr = WalFrameHeader {
        frame_seq: 1,
        tx_id: TxId::new(1),
        logical_page_id: 0,
        frame_kind: WalFrameKind::Commit,
        payload_byte_len: WAL_COMMIT_PAYLOAD_SIZE as u32,
        salt1: whdr.salt1,
        cumulative_checksum: 0,
    };
    let mut fbuf = [0u8; WAL_FRAME_HEADER_SIZE];
    fhdr.encode(&cbuf, whdr.header_checksum, &mut fbuf).unwrap();
    let decoded = WalFrameHeader::decode_and_verify(&fbuf, &cbuf, whdr.header_checksum).unwrap();
    assert_eq!(decoded, fhdr);

    // Wrong prev_checksum must fail closed
    assert_eq!(
        WalFrameHeader::decode_and_verify(&fbuf, &cbuf, whdr.header_checksum ^ 1)
            .unwrap_err()
            .category(),
        ErrorCategory::Corruption
    );
}
