//! Core adapters closing dependency inversions between domain crates without circular imports.

pub mod record_compactor;

pub use record_compactor::DocumentRecordCompactor;
