//! Nested statement savepoint markers for [`crate::transaction::WriteTxn`].

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

/// O(1) savepoint marker referencing positions in `WriteTxn`'s undo log and retired-page list.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SavepointMark {
    pub undo_len: usize,
    pub retired_len: usize,
    pub meta: CommittedDbMeta,
}
