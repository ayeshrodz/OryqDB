//! Core adapters closing dependency inversions between domain crates without circular imports.

pub mod query_source;
pub mod record_compactor;

pub use query_source::CoreQuerySource;
pub use record_compactor::DocumentRecordCompactor;
