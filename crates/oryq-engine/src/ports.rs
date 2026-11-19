//! Narrow engine boundary ports exported to upper layers (`oryq-core` / `oryq-index`).

use std::fmt::Debug;

use oryq_types::{BudgetHandle, Result, TxId};

/// Pure injected compaction callback supplied by `oryq-core` when compacting
/// state-normalized slotted-page delta chains (`W12`).
///
/// The engine supplies immutable base/patch byte slices and a budget handle;
/// implementations must be pure and cannot acquire database locks.
pub trait RecordCompactor: Debug + Send + Sync {
    fn compact(&self, base: &[u8], patches: &[&[u8]], budget: &BudgetHandle) -> Result<Vec<u8>>;
}

/// Durable acknowledgment returned by [`crate::transaction::WriteTxn::commit`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommitReceipt {
    pub tx_id: TxId,
    pub wal_end_frame_seq: u64,
}

/// Summary returned by [`crate::transaction::StorageEngine::checkpoint`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointOutcome {
    pub checkpointed_tx_id: TxId,
    pub pages_copied: usize,
    pub wal_reset: bool,
}
