//! Single-field and nested-path secondary B+ tree index provider
//! (`docs/indexing.md` §2, §4.1, §4.5 & `docs/modules/index.md` §4).

pub mod extract;
pub mod read;
pub mod write;

pub use extract::{build_secondary_full_key, extract_secondary_key_prefix, split_record_id_suffix};
pub use read::{scan_secondary_all, scan_secondary_eq, scan_secondary_range};
pub use write::{SecondaryIndexProvider, SECONDARY_PROVIDER_ID};
