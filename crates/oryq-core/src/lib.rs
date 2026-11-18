#![forbid(unsafe_code)]

//! `oryq-core` public facade crate scaffold (scheduled for `W08` and `W14`).

pub use oryq_types::{
    BudgetDomain, BudgetHandle, CancellationSource, CancellationToken, CollectionId, ErrorCategory,
    ErrorEnvelope, IndexId, PageId, RecordId, Result, SnapshotId, TxId,
};

/// Returns the current workspace version string.
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
