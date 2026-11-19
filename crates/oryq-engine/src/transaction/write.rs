//! Exclusive write transaction with private dirty page buffers, nested statement savepoints,
//! and atomic WAL commit/rollback (`docs/transactions.md` §3 & §4.2).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use oryq_types::{ErrorCategory, ErrorEnvelope, PageId, Result, TxId};

use crate::btree::{PageStore, PageStoreMut};
use crate::config::PageSize;
use crate::coordinator::WriterPermit;
use crate::format::PageHeader;
use crate::ports::CommitReceipt;
use crate::recovery::CommittedDbMeta;
use crate::transaction::savepoint::{SavepointId, SavepointSnapshot};
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
    dirty_pages: HashMap<PageId, Vec<u8>>,
    retired_pages: Vec<PageId>,
    meta: CommittedDbMeta,
    savepoints: Vec<SavepointSnapshot>,
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

    /// Creates a nested statement savepoint capturing current dirty pages, allocations,
    /// retired pages, and root metadata.
    pub fn savepoint(&mut self) -> SavepointId {
        let id = SavepointId::new(self.savepoints.len());
        self.savepoints.push(SavepointSnapshot {
            dirty_pages: self.dirty_pages.clone(),
            retired_pages: self.retired_pages.clone(),
            meta: self.meta,
        });
        id
    }

    /// Rolls back all changes made since `savepoint` (and discards any deeper nested savepoints).
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
        let snap = self.savepoints[idx].clone();
        self.savepoints.truncate(idx);
        self.dirty_pages = snap.dirty_pages;
        self.retired_pages = snap.retired_pages;
        self.meta = snap.meta;
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
        Ok(())
    }

    /// Durably commits all buffered page mutations to `.oryq-wal`, syncs, and publishes
    /// the new snapshot atomically.
    pub fn commit(mut self) -> Result<CommitReceipt> {
        let ordered: BTreeMap<PageId, Vec<u8>> = self.dirty_pages.drain().collect();
        self.meta.latest_tx_id = self.tx_id;
        let receipt = self
            .engine
            .commit_write_txn(self.tx_id, ordered, self.meta)?;
        self.committed = true;
        Ok(receipt)
    }

    /// Explicitly discards all uncommitted changes and releases writer ownership.
    pub fn rollback(mut self) {
        self.dirty_pages.clear();
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
            return Ok(Arc::from(dirty.as_slice()));
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
        // Validate page header checksum and lsn_tx_id match before buffering.
        let hdr = PageHeader::decode_and_verify(bytes, page_id)?;
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
        self.dirty_pages.insert(page_id, bytes.to_vec());
        if page_id.raw() >= self.meta.total_page_count {
            self.meta.total_page_count = page_id.raw() + 1;
        }
        Ok(())
    }

    fn allocate_page(&mut self) -> Result<PageId> {
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

    fn free_page(&mut self, page_id: PageId) -> Result<()> {
        if page_id == PageId::HEADER {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "WriteTxn::free_page",
                "cannot free Page 0",
            ));
        }
        self.retired_pages.push(page_id);
        Ok(())
    }
}
