//! Secondary single/nested-field [`IndexProvider`] implementation with uniqueness,
//! sparse omission, and differential transactional maintenance.

use std::collections::BTreeSet;

use oryq_document::DocumentView;
use oryq_engine::{BTree, PageStore, TreeKind, WriteTxn};
use oryq_types::{
    CapabilityId, ErrorCategory, ErrorEnvelope, PageId, RecordId, Result, VersionRange,
};

use crate::descriptor::{IndexDescriptor, IndexKind, IndexReadCapabilities, MaintenanceResult};
use crate::provider::IndexProvider;
use crate::secondary::extract::{
    build_secondary_full_key, extract_secondary_key_prefixes, split_record_id_suffix,
};

const MODULE_ID: &str = "index";
pub const SECONDARY_PROVIDER_ID: &str = "index.secondary";

/// Built-in single/nested-path secondary B+ tree index provider.
#[derive(Debug, Default)]
pub struct SecondaryIndexProvider;

impl IndexProvider for SecondaryIndexProvider {
    fn provider_id(&self) -> CapabilityId {
        CapabilityId::new(SECONDARY_PROVIDER_ID).expect("valid static capability id")
    }

    fn supported_data_versions(&self) -> VersionRange {
        VersionRange::new(1, 1).expect("valid range")
    }

    fn capabilities(&self, desc: &IndexDescriptor) -> IndexReadCapabilities {
        IndexReadCapabilities {
            exact_equality: !desc.sparse,
            ordered_range: true,
            multikey: true,
            sparse: desc.sparse,
        }
    }

    fn validate_descriptor(&self, desc: &IndexDescriptor) -> Result<()> {
        if desc.kind != IndexKind::Secondary {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "SecondaryIndexProvider::validate_descriptor",
                "SecondaryIndexProvider requires IndexKind::Secondary",
            ));
        }
        if desc.fields.len() != 1 {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "SecondaryIndexProvider::validate_descriptor",
                "secondary index requires exactly 1 field path",
            ));
        }
        if desc.fields[0].path.as_str() == "_id" {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "SecondaryIndexProvider::validate_descriptor",
                "cannot create a secondary index on '_id' (already covered by primary index)",
            ));
        }
        if !self.supported_data_versions().contains(desc.data_version) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::UnsupportedFeature,
                MODULE_ID,
                "SecondaryIndexProvider::validate_descriptor",
                format!(
                    "unsupported secondary index data_version {}",
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
        let prefixes = extract_secondary_key_prefixes(desc, doc, tx.page_size())?;
        if prefixes.is_empty() {
            return Ok(MaintenanceResult {
                new_root_page_id: desc.root_page_id,
                entries_added: 0,
                entries_removed: 0,
            });
        }

        let mut tree = BTree::open(desc.root_page_id, TreeKind::Index);
        for prefix in &prefixes {
            if desc.unique {
                check_unique_collision(tx, &tree, prefix, record_id, &desc.name)?;
            }
            let full_key = build_secondary_full_key(prefix, record_id);
            tree.insert(tx, &full_key, &[])?;
        }
        Ok(MaintenanceResult {
            new_root_page_id: tree.root(),
            entries_added: prefixes.len(),
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
        let old_set: BTreeSet<Vec<u8>> =
            extract_secondary_key_prefixes(desc, before, tx.page_size())?
                .into_iter()
                .collect();
        let new_set: BTreeSet<Vec<u8>> =
            extract_secondary_key_prefixes(desc, after, tx.page_size())?
                .into_iter()
                .collect();

        // Differential bypass: if the extracted secondary keys did not change, perform zero B+ tree I/O.
        if old_set == new_set {
            return Ok(MaintenanceResult {
                new_root_page_id: desc.root_page_id,
                entries_added: 0,
                entries_removed: 0,
            });
        }

        let mut tree = BTree::open(desc.root_page_id, TreeKind::Index);
        let mut removed = 0usize;
        let mut added = 0usize;

        for old_p in old_set.difference(&new_set) {
            let old_key = build_secondary_full_key(old_p, record_id);
            if tree.delete(tx, &old_key)?.is_some() {
                removed += 1;
            }
        }

        for new_p in new_set.difference(&old_set) {
            if desc.unique {
                check_unique_collision(tx, &tree, new_p, record_id, &desc.name)?;
            }
            let new_key = build_secondary_full_key(new_p, record_id);
            tree.insert(tx, &new_key, &[])?;
            added += 1;
        }

        Ok(MaintenanceResult {
            new_root_page_id: tree.root(),
            entries_added: added,
            entries_removed: removed,
        })
    }

    fn on_delete(
        &self,
        tx: &mut WriteTxn<'_>,
        desc: &IndexDescriptor,
        record_id: RecordId,
        doc: &DocumentView<'_>,
    ) -> Result<MaintenanceResult> {
        let prefixes = extract_secondary_key_prefixes(desc, doc, tx.page_size())?;
        if prefixes.is_empty() {
            return Ok(MaintenanceResult {
                new_root_page_id: desc.root_page_id,
                entries_added: 0,
                entries_removed: 0,
            });
        }

        let mut tree = BTree::open(desc.root_page_id, TreeKind::Index);
        let mut removed = 0usize;
        for prefix in &prefixes {
            let full_key = build_secondary_full_key(prefix, record_id);
            if tree.delete(tx, &full_key)?.is_some() {
                removed += 1;
            }
        }
        Ok(MaintenanceResult {
            new_root_page_id: tree.root(),
            entries_added: 0,
            entries_removed: removed,
        })
    }

    fn drop_index(&self, tx: &mut WriteTxn<'_>, desc: &IndexDescriptor) -> Result<()> {
        BTree::open(desc.root_page_id, TreeKind::Index).free_all_pages(tx)
    }
}

fn check_unique_collision(
    store: &impl PageStore,
    tree: &BTree,
    prefix: &[u8],
    self_record_id: RecordId,
    index_name: &str,
) -> Result<()> {
    let mut seek_key = Vec::with_capacity(prefix.len() + 8);
    seek_key.extend_from_slice(prefix);
    seek_key.extend_from_slice(&[0x00; 8]);

    let mut cur = tree.cursor(store);
    if cur.seek_ge(&seek_key)? {
        if let Some((existing_key, _)) = cur.current() {
            let (existing_prefix, existing_rid) = split_record_id_suffix(existing_key)?;
            if existing_prefix == prefix && existing_rid != self_record_id {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::DuplicateKey,
                    MODULE_ID,
                    "SecondaryIndexProvider::check_unique_collision",
                    format!("duplicate key violation on unique index '{index_name}'"),
                ));
            }
        }
    }
    Ok(())
}
