//! Object-safe [`IndexProvider`] boundary (`docs/modules/index.md` §3).

use std::fmt::Debug;

use oryq_document::DocumentView;
use oryq_engine::WriteTxn;
use oryq_types::{CapabilityId, PageId, RecordId, Result, VersionRange};

use crate::descriptor::{IndexDescriptor, IndexReadCapabilities, MaintenanceResult};

/// Transaction-independent index provider registered in [`crate::registry::IndexProviderRegistry`].
///
/// All mutations execute inside the caller's [`WriteTxn`] (and statement savepoint);
/// providers never commit independently.
pub trait IndexProvider: Debug + Send + Sync + 'static {
    /// Stable provider capability identifier (e.g. `"index.primary"`).
    fn provider_id(&self) -> CapabilityId;

    /// Supported persistent data format versions for this provider.
    fn supported_data_versions(&self) -> VersionRange;

    /// Declares planner-visible read capabilities for `desc`.
    fn capabilities(&self, desc: &IndexDescriptor) -> IndexReadCapabilities;

    /// Validates `desc` before creation or reopen.
    fn validate_descriptor(&self, desc: &IndexDescriptor) -> Result<()>;

    /// Initializes a valid empty index root page inside `tx` so reopen/drop work on empty collections.
    fn create_empty(&self, tx: &mut WriteTxn<'_>, desc: &IndexDescriptor) -> Result<PageId>;

    /// Applies an insert of `(record_id, doc)` inside `tx`, returning updated root metadata.
    fn on_insert(
        &self,
        tx: &mut WriteTxn<'_>,
        desc: &IndexDescriptor,
        record_id: RecordId,
        doc: &DocumentView<'_>,
    ) -> Result<MaintenanceResult>;

    /// Applies a differential update from `before` to `after` for `record_id` inside `tx`.
    fn on_update(
        &self,
        tx: &mut WriteTxn<'_>,
        desc: &IndexDescriptor,
        record_id: RecordId,
        before: &DocumentView<'_>,
        after: &DocumentView<'_>,
    ) -> Result<MaintenanceResult>;

    /// Applies a delete of `(record_id, doc)` inside `tx`.
    fn on_delete(
        &self,
        tx: &mut WriteTxn<'_>,
        desc: &IndexDescriptor,
        record_id: RecordId,
        doc: &DocumentView<'_>,
    ) -> Result<MaintenanceResult>;

    /// Drops and frees all pages owned by this index inside `tx`.
    fn drop_index(&self, tx: &mut WriteTxn<'_>, desc: &IndexDescriptor) -> Result<()>;
}
