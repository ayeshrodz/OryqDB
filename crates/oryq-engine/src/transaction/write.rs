//! Exclusive write transaction with private dirty page buffers, O(1) undo-log nested
//! statement savepoints, and atomic WAL commit/rollback (`docs/transactions.md` §3 & §4.2).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use oryq_types::{ErrorCategory, ErrorEnvelope, PageId, Result, TxId};

use crate::btree::{PageStore, PageStoreMut};
use crate::config::PageSize;
use crate::coordinator::WriterPermit;
use crate::format::{FreelistEntry, FreelistTrunkHeader, PageHeader};
use crate::ports::CommitReceipt;
use crate::recovery::CommittedDbMeta;
use crate::transaction::savepoint::{SavepointId, SavepointMark};
use crate::transaction::StorageEngine;

const MODULE_ID: &str = "engine";

/// Exclusive write transaction buffering all mutations privately until [`Self::commit`].
///
/// Dropping a `WriteTxn` without calling `commit()` automatically rolls back all
/// uncommitted dirty pages and releases writer ownership.
#[derive(Debug)]
pub struct WriteTxn<'a> {
    engine: &'a StorageEngine,
    _permit: WriterPermit,
    base_tx_id: TxId,
    tx_id: TxId,
    dirty_pages: HashMap<PageId, Arc<[u8]>>,
    undo_log: Vec<(PageId, Option<Arc<[u8]>>)>,
    retired_pages: Vec<PageId>,
    meta: CommittedDbMeta,
    savepoints: Vec<SavepointMark>,
    committed: bool,
}

impl<'a> WriteTxn<'a> {
    pub(crate) fn new(
        engine: &'a StorageEngine,
        permit: WriterPermit,
        base_meta: CommittedDbMeta,
    ) -> Result<Self> {
        let base_tx_id = permit.base_tx_id();
        let tx_id = base_tx_id.checked_next()?;
        Ok(Self {
            engine,
            _permit: permit,
            base_tx_id,
            tx_id,
            dirty_pages: HashMap::new(),
            undo_log: Vec::new(),
            retired_pages: Vec::new(),
            meta: base_meta,
            savepoints: Vec::new(),
            committed: false,
        })
    }

    #[inline]
    pub fn tx_id(&self) -> TxId {
        self.tx_id
    }

    #[inline]
    pub fn catalog_root_page_id(&self) -> PageId {
        self.meta.catalog_root_page_id
    }

    #[inline]
    pub fn set_catalog_root_page_id(&mut self, root: PageId) {
        self.meta.catalog_root_page_id = root;
    }

    #[inline]
    pub fn total_page_count(&self) -> u64 {
        self.meta.total_page_count
    }

    /// Creates an O(1) nested statement savepoint marker inside this write transaction.
    pub fn savepoint(&mut self) -> SavepointId {
        let id = SavepointId::new(self.savepoints.len());
        self.savepoints.push(SavepointMark {
            undo_len: self.undo_log.len(),
            retired_len: self.retired_pages.len(),
            meta: self.meta,
        });
        id
    }

    /// Rolls back all changes made since `savepoint` (and discards any deeper nested savepoints)
    /// by replaying only the pages modified after `savepoint` in reverse order.
    pub fn rollback_to(&mut self, savepoint: SavepointId) -> Result<()> {
        let idx = savepoint.depth();
        if idx >= self.savepoints.len() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "WriteTxn::rollback_to",
                "invalid or already-released savepoint handle",
            ));
        }
        let mark = self.savepoints[idx];
        self.savepoints.truncate(idx);
        for (pid, prev) in self.undo_log.drain(mark.undo_len..).rev() {
            match prev {
                Some(old_bytes) => {
                    self.dirty_pages.insert(pid, old_bytes);
                }
                None => {
                    self.dirty_pages.remove(&pid);
                }
            }
        }
        if self.savepoints.is_empty() {
            self.undo_log.clear();
        }
        self.retired_pages.truncate(mark.retired_len);
        self.meta = mark.meta;
        Ok(())
    }

    /// Releases `savepoint` (and any deeper nested savepoints), keeping their mutations.
    pub fn release_savepoint(&mut self, savepoint: SavepointId) -> Result<()> {
        let idx = savepoint.depth();
        if idx >= self.savepoints.len() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "WriteTxn::release_savepoint",
                "invalid or already-released savepoint handle",
            ));
        }
        self.savepoints.truncate(idx);
        if self.savepoints.is_empty() {
            self.undo_log.clear();
        }
        Ok(())
    }

    /// Durably commits all buffered page mutations to `.oryq-wal`, syncs, and publishes
    /// the new snapshot atomically.
    pub fn commit(mut self) -> Result<CommitReceipt> {
        self.flush_retired_pages_to_freelist()?;
        self.undo_log.clear();
        let mut ordered: BTreeMap<PageId, Arc<[u8]>> = BTreeMap::new();
        for (pid, mut bytes) in self.dirty_pages.drain() {
            if let Some(slice) = Arc::get_mut(&mut bytes) {
                PageHeader::stamp_checksum(slice);
                ordered.insert(pid, bytes);
            } else {
                let mut cloned = bytes.to_vec();
                PageHeader::stamp_checksum(&mut cloned);
                ordered.insert(pid, Arc::from(cloned));
            }
        }
        self.meta.latest_tx_id = self.tx_id;
        let receipt = self
            .engine
            .commit_write_txn(self.tx_id, ordered, self.meta)?;
        self.committed = true;
        Ok(receipt)
    }

    /// Reclaims one retired page from the MVCC freelist whose `retired_tx_id <= horizon`
    /// (meaning no active or future reader can ever reach its pre-retirement contents).
    fn reclaim_from_freelist(&mut self) -> Result<Option<PageId>> {
        if self.meta.freelist_trunk_page_id == 0 || self.meta.freelist_page_count == 0 {
            return Ok(None);
        }
        let horizon = self.engine.retention_horizon()?;
        let page_size = self.engine.page_size().bytes();
        let mut visited = HashSet::new();
        let mut prev_trunk: Option<(PageId, Vec<FreelistEntry>)> = None;
        let mut current_raw = self.meta.freelist_trunk_page_id;

        while current_raw != 0 {
            let trunk_pid = PageId::new(current_raw);
            if !visited.insert(trunk_pid) {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "WriteTxn::reclaim_from_freelist",
                    format!(
                        "cycle detected in freelist trunk chain at page {}",
                        trunk_pid.raw()
                    ),
                ));
            }
            let bytes = self.read_page(trunk_pid)?;
            let (_, trunk_hdr, mut entries) = FreelistTrunkHeader::decode_page(&bytes, trunk_pid)?;

            if let Some(pos) = entries.iter().position(|e| e.retired_tx_id <= horizon) {
                let reclaimed = entries.swap_remove(pos);
                if entries.is_empty() {
                    // Unlink emptied trunk (whether head or interior) so empty trunks never
                    // accumulate or leak, and stage `trunk_pid` itself for retirement.
                    match prev_trunk {
                        None => {
                            self.meta.freelist_trunk_page_id =
                                trunk_hdr.next_trunk.map_or(0, PageId::raw);
                        }
                        Some((prev_pid, prev_entries)) => {
                            let mut buf = vec![0u8; page_size];
                            FreelistTrunkHeader::encode_page(
                                &mut buf,
                                prev_pid,
                                self.tx_id,
                                trunk_hdr.next_trunk,
                                &prev_entries,
                            )?;
                            self.write_page(prev_pid, &buf)?;
                        }
                    }
                    if let Some(prev_dirty) = self.dirty_pages.remove(&trunk_pid) {
                        if !self.savepoints.is_empty() {
                            self.undo_log.push((trunk_pid, Some(prev_dirty)));
                        }
                    }
                    self.retired_pages.push(trunk_pid);
                } else {
                    let mut buf = vec![0u8; page_size];
                    FreelistTrunkHeader::encode_page(
                        &mut buf,
                        trunk_pid,
                        self.tx_id,
                        trunk_hdr.next_trunk,
                        &entries,
                    )?;
                    self.write_page(trunk_pid, &buf)?;
                }
                self.meta.freelist_page_count = self.meta.freelist_page_count.saturating_sub(1);
                return Ok(Some(reclaimed.page_id));
            }

            prev_trunk = Some((trunk_pid, entries));
            current_raw = trunk_hdr.next_trunk.map_or(0, PageId::raw);
        }

        Ok(None)
    }

    /// Collects all trunk and entry `PageId`s currently present in the freelist chain so
    /// `flush_retired_pages_to_freelist` enforces global duplicate-freedom across all trunks.
    fn collect_existing_freelist_pages(&self) -> Result<HashSet<PageId>> {
        let mut all = HashSet::new();
        let mut visited_trunks = HashSet::new();
        let mut current_raw = self.meta.freelist_trunk_page_id;
        while current_raw != 0 {
            let trunk_pid = PageId::new(current_raw);
            if !visited_trunks.insert(trunk_pid) {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "WriteTxn::collect_existing_freelist_pages",
                    format!(
                        "cycle detected in freelist trunk chain at page {}",
                        trunk_pid.raw()
                    ),
                ));
            }
            all.insert(trunk_pid);
            let bytes = self.read_page(trunk_pid)?;
            let (_, trunk_hdr, entries) = FreelistTrunkHeader::decode_page(&bytes, trunk_pid)?;
            for e in entries {
                all.insert(e.page_id);
            }
            current_raw = trunk_hdr.next_trunk.map_or(0, PageId::raw);
        }
        Ok(all)
    }

    /// Flushes all pages retired during this `WriteTxn` into `PageType::Freelist` trunk pages
    /// tagged with `retired_tx_id = self.tx_id` before committing to `.oryq-wal`.
    fn flush_retired_pages_to_freelist(&mut self) -> Result<()> {
        if self.retired_pages.is_empty() {
            return Ok(());
        }
        let mut existing = self.collect_existing_freelist_pages()?;
        let mut queue: VecDeque<PageId> = std::mem::take(&mut self.retired_pages).into();
        let page_size = self.engine.page_size().bytes();
        let max_entries = FreelistTrunkHeader::max_entries(page_size);
        let mut buf = vec![0u8; page_size];

        while let Some(page_id) = queue.pop_front() {
            if page_id == PageId::HEADER
                || page_id == self.meta.catalog_root_page_id
                || page_id.raw() >= self.meta.total_page_count
                || existing.contains(&page_id)
            {
                continue;
            }
            // Drop any intra-transaction dirty leaf/overflow image for a page retired before commit.
            self.dirty_pages.remove(&page_id);

            let head_raw = self.meta.freelist_trunk_page_id;
            if head_raw != 0 {
                let head_pid = PageId::new(head_raw);
                let bytes = self.read_page(head_pid)?;
                let (_, trunk_hdr, mut entries) =
                    FreelistTrunkHeader::decode_page(&bytes, head_pid)?;
                if entries.len() < max_entries {
                    entries.push(FreelistEntry {
                        page_id,
                        retired_tx_id: self.tx_id,
                    });
                    FreelistTrunkHeader::encode_page(
                        &mut buf,
                        head_pid,
                        self.tx_id,
                        trunk_hdr.next_trunk,
                        &entries,
                    )?;
                    self.write_page(head_pid, &buf)?;
                    self.meta.freelist_page_count += 1;
                    existing.insert(page_id);
                    continue;
                }
            }

            // Allocate a fresh trunk page (reusing an older horizon-safe freelist page if available,
            // otherwise extending `total_page_count`). If unlinking an emptied trunk staged new
            // retired pages, enqueue them into the same worklist so none are ever dropped.
            let new_trunk_pid = self.allocate_page_raw_or_reclaim()?;
            debug_assert_ne!(new_trunk_pid, page_id);
            existing.remove(&new_trunk_pid);
            if !self.retired_pages.is_empty() {
                for extra in self.retired_pages.drain(..) {
                    existing.remove(&extra);
                    queue.push_back(extra);
                }
            }

            let prev_head = if self.meta.freelist_trunk_page_id == 0 {
                None
            } else {
                Some(PageId::new(self.meta.freelist_trunk_page_id))
            };
            let entries = [FreelistEntry {
                page_id,
                retired_tx_id: self.tx_id,
            }];
            FreelistTrunkHeader::encode_page(
                &mut buf,
                new_trunk_pid,
                self.tx_id,
                prev_head,
                &entries,
            )?;
            self.write_page(new_trunk_pid, &buf)?;
            self.meta.freelist_trunk_page_id = new_trunk_pid.raw();
            self.meta.freelist_page_count += 1;
            existing.insert(new_trunk_pid);
            existing.insert(page_id);
        }

        Ok(())
    }

    fn allocate_page_raw_or_reclaim(&mut self) -> Result<PageId> {
        if let Some(reused) = self.reclaim_from_freelist()? {
            return Ok(reused);
        }
        let next_raw = self.meta.total_page_count;
        self.meta.total_page_count = next_raw.checked_add(1).ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "WriteTxn::allocate_page",
                "total_page_count overflow",
            )
        })?;
        Ok(PageId::new(next_raw))
    }

    /// Explicitly discards all uncommitted changes and releases writer ownership.
    pub fn rollback(mut self) {
        self.dirty_pages.clear();
        self.undo_log.clear();
        self.retired_pages.clear();
        self.savepoints.clear();
        self.committed = true;
    }
}

impl PageStore for WriteTxn<'_> {
    fn page_size(&self) -> PageSize {
        self.engine.page_size()
    }

    fn read_page(&self, page_id: PageId) -> Result<Arc<[u8]>> {
        if let Some(dirty) = self.dirty_pages.get(&page_id) {
            return Ok(Arc::clone(dirty));
        }
        self.engine.read_page_at_snapshot(page_id, self.base_tx_id)
    }
}

impl PageStoreMut for WriteTxn<'_> {
    fn writer_tx_id(&self) -> TxId {
        self.tx_id
    }

    fn write_page(&mut self, page_id: PageId, bytes: &[u8]) -> Result<()> {
        if page_id == PageId::HEADER {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "WriteTxn::write_page",
                "cannot write Page 0 directly from WriteTxn",
            ));
        }
        if bytes.len() != self.engine.page_size().bytes() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "WriteTxn::write_page",
                "page slice length does not match configured page_size",
            ));
        }
        // Validate page header layout, slotted structure, and lsn_tx_id match before buffering.
        // Full-page `XXH3_64` checksums are stamped once per final dirty page in `commit()`.
        let hdr = PageHeader::decode_without_checksum(bytes, page_id)?;
        if crate::btree::TreeKind::is_leaf(hdr.page_type)
            || crate::btree::TreeKind::is_internal(hdr.page_type)
        {
            let _ = crate::format::SlottedPageView::parse_with_header(bytes, hdr)?;
        }
        if hdr.lsn_tx_id != self.tx_id {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "WriteTxn::write_page",
                format!(
                    "page {} lsn_tx_id {} does not match WriteTxn tx_id {}",
                    page_id.raw(),
                    hdr.lsn_tx_id.raw(),
                    self.tx_id.raw()
                ),
            ));
        }
        let prev = self.dirty_pages.insert(page_id, Arc::from(bytes));
        if !self.savepoints.is_empty() {
            self.undo_log.push((page_id, prev));
        }
        if page_id.raw() >= self.meta.total_page_count {
            self.meta.total_page_count = page_id.raw() + 1;
        }
        Ok(())
    }

    fn allocate_page(&mut self) -> Result<PageId> {
        self.allocate_page_raw_or_reclaim()
    }

    fn free_page(&mut self, page_id: PageId) -> Result<()> {
        if page_id == PageId::HEADER {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "WriteTxn::free_page",
                "cannot free Page 0",
            ));
        }
        if page_id.raw() >= self.meta.total_page_count {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "WriteTxn::free_page",
                format!(
                    "page_id {} out of bounds (total_page_count={})",
                    page_id.raw(),
                    self.meta.total_page_count
                ),
            ));
        }
        self.retired_pages.push(page_id);
        Ok(())
    }

    #[inline]
    fn defer_checksums(&self) -> bool {
        true
    }
}
