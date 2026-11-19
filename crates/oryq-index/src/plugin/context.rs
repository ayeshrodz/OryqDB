//! Restricted read/write storage contexts for index providers.
//!
//! Providers receive scoped page-store access bound to a [`NamespaceId`] without
//! authority to commit, checkpoint, or obtain raw file handles.

use std::sync::Arc;

use oryq_engine::{PageSize, PageStore, PageStoreMut, ReadTxn, WriteTxn};
use oryq_types::{NamespaceId, PageId, Result, SnapshotId, TxId};

/// Restricted read-only storage context bound to a caller's [`ReadTxn`] snapshot.
#[derive(Debug)]
pub struct IndexReadContext<'a, 'tx> {
    namespace_id: NamespaceId,
    tx: &'a ReadTxn<'tx>,
}

impl<'a, 'tx> IndexReadContext<'a, 'tx> {
    pub fn new(namespace_id: NamespaceId, tx: &'a ReadTxn<'tx>) -> Self {
        Self { namespace_id, tx }
    }

    #[inline]
    pub fn namespace_id(&self) -> NamespaceId {
        self.namespace_id
    }

    #[inline]
    pub fn snapshot_id(&self) -> SnapshotId {
        self.tx.snapshot_id()
    }
}

impl PageStore for IndexReadContext<'_, '_> {
    fn page_size(&self) -> PageSize {
        self.tx.page_size()
    }

    fn read_page(&self, page_id: PageId) -> Result<Arc<[u8]>> {
        self.tx.read_page(page_id)
    }
}

/// Restricted transactional write storage context bound to a caller's [`WriteTxn`].
///
/// Intentionally exposes only page allocation/read/write/free (`PageStoreMut`) and
/// cannot commit, rollback, checkpoint, or access OS file handles.
#[derive(Debug)]
pub struct IndexWriteContext<'a, 'tx> {
    namespace_id: NamespaceId,
    tx: &'a mut WriteTxn<'tx>,
}

impl<'a, 'tx> IndexWriteContext<'a, 'tx> {
    pub fn new(namespace_id: NamespaceId, tx: &'a mut WriteTxn<'tx>) -> Self {
        Self { namespace_id, tx }
    }

    #[inline]
    pub fn namespace_id(&self) -> NamespaceId {
        self.namespace_id
    }
}

impl PageStore for IndexWriteContext<'_, '_> {
    fn page_size(&self) -> PageSize {
        self.tx.page_size()
    }

    fn read_page(&self, page_id: PageId) -> Result<Arc<[u8]>> {
        self.tx.read_page(page_id)
    }
}

impl PageStoreMut for IndexWriteContext<'_, '_> {
    fn writer_tx_id(&self) -> TxId {
        self.tx.writer_tx_id()
    }

    fn write_page(&mut self, page_id: PageId, bytes: &[u8]) -> Result<()> {
        self.tx.write_page(page_id, bytes)
    }

    fn allocate_page(&mut self) -> Result<PageId> {
        self.tx.allocate_page()
    }

    fn free_page(&mut self, page_id: PageId) -> Result<()> {
        self.tx.free_page(page_id)
    }
}
