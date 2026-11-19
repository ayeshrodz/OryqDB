//! Nested statement savepoint snapshots for [`crate::transaction::WriteTxn`].

use std::collections::HashMap;

use oryq_types::PageId;

use crate::recovery::CommittedDbMeta;

/// Opaque handle representing a nested statement savepoint inside a [`crate::transaction::WriteTxn`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SavepointId(usize);

impl SavepointId {
    #[inline]
    pub(crate) const fn new(depth: usize) -> Self {
        Self(depth)
    }

    #[inline]
    pub(crate) const fn depth(self) -> usize {
        self.0
    }
}

/// Captured transaction-private state prior to a savepoint.
#[derive(Debug, Clone)]
pub(crate) struct SavepointSnapshot {
    pub dirty_pages: HashMap<PageId, Vec<u8>>,
    pub retired_pages: Vec<PageId>,
    pub meta: CommittedDbMeta,
}
