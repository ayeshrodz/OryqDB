//! Snapshot admission and single-writer coordination boundary.

pub mod single_process;

pub use single_process::{SingleProcessCoordinator, SnapshotGuard, WriterPermit};
