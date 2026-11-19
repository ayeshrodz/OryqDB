//! Primary `_id -> RecordId` [`IndexProvider`] implementation enforcing mandatory
//! primary uniqueness, immutable `_id` checks, and transactional B+ tree maintenance.

use std::collections::HashSet;

use oryq_document::DocumentView;
use oryq_engine::btree::read_internal_node;
use oryq_engine::{BTree, PageStore, PageStoreMut, SlottedPageView, TreeKind, WriteTxn};
use oryq_types::{
    CapabilityId, ErrorCategory, ErrorEnvelope, PageId, RecordId, Result, VersionRange,
};

use crate::descriptor::{IndexDescriptor, IndexKind, IndexReadCapabilities, MaintenanceResult};
use crate::key_codec::{decode_record_id, encode_primary_id_key, encode_record_id};
use crate::provider::IndexProvider;

const MODULE_ID: &str = "index";
pub const PRIMARY_PROVIDER_ID: &str = "index.primary";

/// Built-in primary `_id -> RecordId` index provider.
#[derive(Debug, Default)]
pub struct PrimaryIndexProvider;

impl IndexProvider for PrimaryIndexProvider {
    fn provider_id(&self) -> CapabilityId {
        CapabilityId::new(PRIMARY_PROVIDER_ID).expect("valid static capability id")
    }

    fn supported_data_versions(&self) -> VersionRange {
        VersionRange::new(1, 1).expect("valid range")
    }

    fn capabilities(&self, _desc: &IndexDescriptor) -> IndexReadCapabilities {
        IndexReadCapabilities {
            exact_equality: true,
            ordered_range: true,
            multikey: false,
            sparse: false,
        }
    }

    fn validate_descriptor(&self, desc: &IndexDescriptor) -> Result<()> {
        if desc.kind != IndexKind::Primary {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "PrimaryIndexProvider::validate_descriptor",
                "PrimaryIndexProvider requires IndexKind::Primary",
            ));
        }
        if !desc.unique || desc.sparse {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "PrimaryIndexProvider::validate_descriptor",
                "primary index must be unique and non-sparse",
            ));
        }
        if desc.fields.len() != 1 || desc.fields[0].path.as_str() != "_id" {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "PrimaryIndexProvider::validate_descriptor",
                "primary index must index exactly '_id'",
            ));
        }
        if !self.supported_data_versions().contains(desc.data_version) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::UnsupportedFeature,
                MODULE_ID,
                "PrimaryIndexProvider::validate_descriptor",
                format!(
                    "unsupported primary index data_version {}",
                    desc.data_version
                ),
            ));
        }
        Ok(())
    }

    fn create_empty(&self, tx: &mut WriteTxn<'_>, desc: &IndexDescriptor) -> Result<PageId> {
        self.validate_descriptor(desc)?;
        let tree = BTree::create_empty(tx, TreeKind::Index)?;
        Ok(tree.root())
    }

    fn on_insert(
        &self,
        tx: &mut WriteTxn<'_>,
        desc: &IndexDescriptor,
        record_id: RecordId,
        doc: &DocumentView<'_>,
    ) -> Result<MaintenanceResult> {
        let id_val = doc.get_field("_id").ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "PrimaryIndexProvider::on_insert",
                "document is missing required '_id' field",
            )
        })?;
        let key = encode_primary_id_key(&id_val, tx.page_size())?;
        let mut tree = BTree::open(desc.root_page_id, TreeKind::Index);

        if tree.get(tx, &key)?.is_some() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::DuplicateKey,
                MODULE_ID,
                "PrimaryIndexProvider::on_insert",
                "duplicate primary '_id' key in collection",
            ));
        }

        let val = encode_record_id(record_id);
        tree.insert(tx, &key, &val)?;
        Ok(MaintenanceResult {
            new_root_page_id: tree.root(),
            entries_added: 1,
            entries_removed: 0,
        })
    }

    fn on_update(
        &self,
        tx: &mut WriteTxn<'_>,
        desc: &IndexDescriptor,
        record_id: RecordId,
        before: &DocumentView<'_>,
        after: &DocumentView<'_>,
    ) -> Result<MaintenanceResult> {
        let before_id = before.get_field("_id").ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "PrimaryIndexProvider::on_update",
                "existing document missing '_id'",
            )
        })?;
        let after_id = after.get_field("_id").ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "PrimaryIndexProvider::on_update",
                "updated document missing '_id'",
            )
        })?;
        let before_key = encode_primary_id_key(&before_id, tx.page_size())?;
        let after_key = encode_primary_id_key(&after_id, tx.page_size())?;
        if before_key != after_key {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "PrimaryIndexProvider::on_update",
                "primary '_id' is immutable during update",
            ));
        }

        // Verify the mapping still points to `record_id`.
        let tree = BTree::open(desc.root_page_id, TreeKind::Index);
        if let Some(existing) = tree.get(tx, &before_key)? {
            let existing_rid = decode_record_id(&existing)?;
            if existing_rid != record_id {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "PrimaryIndexProvider::on_update",
                    format!(
                        "primary index points to RecordId({}) instead of RecordId({})",
                        existing_rid.raw(),
                        record_id.raw()
                    ),
                ));
            }
        }

        Ok(MaintenanceResult {
            new_root_page_id: desc.root_page_id,
            entries_added: 0,
            entries_removed: 0,
        })
    }

    fn on_delete(
        &self,
        tx: &mut WriteTxn<'_>,
        desc: &IndexDescriptor,
        record_id: RecordId,
        doc: &DocumentView<'_>,
    ) -> Result<MaintenanceResult> {
        let id_val = doc.get_field("_id").ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "PrimaryIndexProvider::on_delete",
                "deleted document missing '_id'",
            )
        })?;
        let key = encode_primary_id_key(&id_val, tx.page_size())?;
        let mut tree = BTree::open(desc.root_page_id, TreeKind::Index);

        if let Some(removed) = tree.delete(tx, &key)? {
            let removed_rid = decode_record_id(&removed)?;
            if removed_rid != record_id {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "PrimaryIndexProvider::on_delete",
                    format!(
                        "primary index delete removed RecordId({}) instead of RecordId({})",
                        removed_rid.raw(),
                        record_id.raw()
                    ),
                ));
            }
            Ok(MaintenanceResult {
                new_root_page_id: tree.root(),
                entries_added: 0,
                entries_removed: 1,
            })
        } else {
            Ok(MaintenanceResult {
                new_root_page_id: tree.root(),
                entries_added: 0,
                entries_removed: 0,
            })
        }
    }

    fn drop_index(&self, tx: &mut WriteTxn<'_>, desc: &IndexDescriptor) -> Result<()> {
        let mut stack = vec![desc.root_page_id];
        let mut visited = HashSet::new();
        while let Some(pid) = stack.pop() {
            if !visited.insert(pid) {
                continue;
            }
            let bytes = tx.read_page(pid)?;
            let view = SlottedPageView::parse(&bytes, pid)?;
            if TreeKind::is_internal(view.header().page_type) {
                let node = read_internal_node(&bytes, pid)?;
                stack.push(node.leftmost_child);
                for (_, child) in node.separators {
                    stack.push(child);
                }
            }
            tx.free_page(pid)?;
        }
        Ok(())
    }
}
