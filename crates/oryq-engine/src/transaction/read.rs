//! Snapshot-isolated read transaction implementing [`PageStore`].

use std::sync::Arc;

use oryq_types::{PageId, Result, SnapshotId};

use crate::btree::PageStore;
use crate::config::PageSize;
use crate::coordinator::SnapshotGuard;
use crate::recovery::CommittedDbMeta;
use crate::transaction::StorageEngine;

/// Immutable, snapshot-isolated read transaction pinned at `snapshot_id()`.
#[derive(Debug)]
pub struct ReadTxn<'a> {
    engine: &'a StorageEngine,
    guard: SnapshotGuard,
    meta: CommittedDbMeta,
}

impl<'a> ReadTxn<'a> {
    pub(crate) fn new(
        engine: &'a StorageEngine,
        guard: SnapshotGuard,
        meta: CommittedDbMeta,
    ) -> Self {
        Self {
            engine,
            guard,
            meta,
        }
    }

    #[inline]
    pub fn snapshot_id(&self) -> SnapshotId {
        self.guard.snapshot_id()
    }

    #[inline]
    pub fn catalog_root_page_id(&self) -> PageId {
        self.meta.catalog_root_page_id
    }

    #[inline]
    pub fn total_page_count(&self) -> u64 {
        self.meta.total_page_count
    }
}

impl PageStore for ReadTxn<'_> {
    fn page_size(&self) -> PageSize {
        self.engine.page_size()
    }

    fn read_page(&self, page_id: PageId) -> Result<Arc<[u8]>> {
        self.engine
            .read_page_at_snapshot(page_id, self.guard.snapshot_id())
    }
}
