//! Bounded `.oryq-wal` scanning and restartable physical replay without domain callbacks.

pub mod replay;
pub mod scan;

pub use replay::{replay_into_version_index, CommittedDbMeta};
pub use scan::{scan_wal, RecoveredPageFrame, RecoveredTx, WalScanResult};
