//! Snapshot admission, single-writer, and optional multi-process (`.oryq-shm`) coordination boundary.

pub mod shared_process;
pub mod single_process;

pub use shared_process::{
    shm_path_for, SharedProcessConfig, SharedProcessCoordinator, SharedSnapshotGuard,
    SharedWriterPermit, ShmInspectionReport, StaleReclaimReport, WalResetGuard, MAX_SHM_READERS,
};
pub use single_process::{SingleProcessCoordinator, SnapshotGuard, WriterPermit};
