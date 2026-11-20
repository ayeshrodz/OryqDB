#![forbid(unsafe_code)]

//! `oryq-index`: Canonical memcomparable key encoding, index descriptors,
//! owner-local provider registry, restricted plugin storage contexts,
//! primary `_id -> RecordId` index, and secondary single/nested-path indexes.

pub mod compound;
pub mod descriptor;
pub mod key_codec;
pub mod plugin;
pub mod primary;
pub mod provider;
pub mod registry;
pub mod secondary;

pub use compound::{
    decode_prefix_block, encode_prefix_block, extract_compound_key_prefixes,
    scan_compound_prefix_eq, CompoundIndexProvider, COMPOUND_PROVIDER_ID, MAX_COMPOUND_FIELDS,
    PREFIX_RESTART_INTERVAL,
};

pub use descriptor::{
    IndexDescriptor, IndexFieldSpec, IndexKind, IndexReadCapabilities, MaintenanceResult,
};
pub use key_codec::{
    decode_record_id, encode_primary_id_key, encode_record_id, encode_value_key,
    max_index_key_bytes, SortOrder, MAX_PRIMARY_STRING_ID_BYTES,
};
pub use oryq_types::{IndexId, NamespaceId, RecordId, Result};
pub use plugin::{IndexReadContext, IndexWriteContext};
pub use primary::{
    lookup_primary_id, scan_primary_record_ids, PrimaryIndexProvider, PRIMARY_PROVIDER_ID,
};
pub use provider::IndexProvider;
pub use registry::IndexProviderRegistry;
pub use secondary::{
    build_secondary_full_key, extract_secondary_key_prefix, scan_secondary_all, scan_secondary_eq,
    scan_secondary_range, split_record_id_suffix, SecondaryIndexProvider, SECONDARY_PROVIDER_ID,
};
