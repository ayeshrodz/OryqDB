//! Storage-independent [`QuerySource`] boundary implemented by `oryq-core`
//! (`docs/modules/query.md` §3).

use std::fmt::Debug;

use oryq_document::{DocumentBuf, Value};
use oryq_index::IndexDescriptor;
use oryq_types::{RecordId, Result};

/// Snapshot-bound collection and index access port consumed by `oryq-query`.
///
/// Query execution never opens transactions or imports `oryq-core`; all reads
/// execute within the caller's single pinned snapshot.
pub trait QuerySource: Debug {
    /// Returns all active [`IndexDescriptor`]s for `collection`.
    fn collection_indexes(&self, collection: &str) -> Result<Vec<IndexDescriptor>>;

    /// Full ordered scan of `(RecordId, DocumentBuf)` in `collection`.
    fn scan_collection(&self, collection: &str) -> Result<Vec<(RecordId, DocumentBuf)>>;

    /// Resolves a single document by internal [`RecordId`].
    fn get_by_record_id(
        &self,
        collection: &str,
        record_id: RecordId,
    ) -> Result<Option<DocumentBuf>>;

    /// Probes the mandatory primary `_id` index for `id_value`.
    fn probe_primary_id(&self, collection: &str, id_value: &Value) -> Result<Option<RecordId>>;

    /// Scans a secondary index for exact equality with `target`.
    fn scan_secondary_eq(&self, desc: &IndexDescriptor, target: &Value) -> Result<Vec<RecordId>>;

    /// Scans a secondary index across optional `(lower, inclusive)` and `(upper, inclusive)` bounds.
    fn scan_secondary_range(
        &self,
        desc: &IndexDescriptor,
        lower: Option<(&Value, bool)>,
        upper: Option<(&Value, bool)>,
    ) -> Result<Vec<RecordId>>;
}
