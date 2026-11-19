//! Index descriptor, capability facts, and transactional maintenance result contracts
//! (`docs/modules/index.md` §3 & §4).

use oryq_document::FieldPath;
use oryq_types::{CapabilityId, IndexId, PageId};

use crate::key_codec::SortOrder;

/// Logical kind of an index descriptor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexKind {
    Primary,
    Secondary,
    Compound,
    Pluggable(String),
}

/// One indexed field path and sort direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexFieldSpec {
    pub path: FieldPath,
    pub order: SortOrder,
}

/// Immutable catalog descriptor for an active index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDescriptor {
    pub index_id: IndexId,
    pub name: String,
    pub kind: IndexKind,
    pub fields: Vec<IndexFieldSpec>,
    pub unique: bool,
    pub sparse: bool,
    pub root_page_id: PageId,
    pub required_feature_bit: Option<u8>,
    pub provider_id: CapabilityId,
    pub data_version: u16,
    pub options_json: Option<String>,
}

impl IndexDescriptor {
    /// Constructs the canonical primary `_id` index descriptor for a collection.
    pub fn primary(index_id: IndexId, root_page_id: PageId) -> Self {
        Self {
            index_id,
            name: "_id_".to_owned(),
            kind: IndexKind::Primary,
            fields: vec![IndexFieldSpec {
                path: FieldPath::parse("_id").expect("_id is a valid path"),
                order: SortOrder::Asc,
            }],
            unique: true,
            sparse: false,
            root_page_id,
            required_feature_bit: None,
            provider_id: CapabilityId::new("index.primary").expect("valid capability id"),
            data_version: 1,
            options_json: None,
        }
    }

    /// Constructs a single/nested-path secondary index descriptor.
    pub fn secondary(
        index_id: IndexId,
        name: String,
        path: FieldPath,
        order: SortOrder,
        unique: bool,
        sparse: bool,
        root_page_id: PageId,
    ) -> Self {
        Self {
            index_id,
            name,
            kind: IndexKind::Secondary,
            fields: vec![IndexFieldSpec { path, order }],
            unique,
            sparse,
            root_page_id,
            required_feature_bit: None,
            provider_id: CapabilityId::new("index.secondary").expect("valid capability id"),
            data_version: 1,
            options_json: None,
        }
    }
}

/// Planner-visible read capability facts declared by an [`crate::provider::IndexProvider`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexReadCapabilities {
    /// Whether index scan results are exact (no residual document recheck needed for equality).
    pub exact_equality: bool,
    /// Whether the index supports ordered range scans.
    pub ordered_range: bool,
    /// Whether the index can emit multiple keys per document (multikey array index).
    pub multikey: bool,
    /// Whether missing-field documents are omitted from the index.
    pub sparse: bool,
}

/// Result returned by transactional index maintenance operations (`on_insert`/`on_update`/`on_delete`).
///
/// Core stages `new_root_page_id` into its transactional catalog within the same statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaintenanceResult {
    pub new_root_page_id: PageId,
    pub entries_added: usize,
    pub entries_removed: usize,
}
