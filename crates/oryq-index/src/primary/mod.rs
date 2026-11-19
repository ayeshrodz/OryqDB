//! Primary `_id -> RecordId` index provider (`docs/indexing.md` §2 & §4.5).

pub mod read;
pub mod write;

pub use read::{lookup_primary_id, scan_primary_record_ids};
pub use write::{PrimaryIndexProvider, PRIMARY_PROVIDER_ID};
