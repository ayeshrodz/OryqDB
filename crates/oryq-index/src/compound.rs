//! Compound (up to 16 fields, mixed `Asc`/`Desc`) & multi-key array index provider
//! with parallel-array prohibition and exact `RecordId` deduplication (`docs/indexing.md` §4.2–§4.4).

use std::collections::BTreeSet;

use oryq_document::{DocumentView, ValueView};
use oryq_engine::{BTree, PageSize, PageStore, TreeKind, WriteTxn};
use oryq_types::{
    CapabilityId, ErrorCategory, ErrorEnvelope, PageId, RecordId, Result, VersionRange,
};

use crate::descriptor::{IndexDescriptor, IndexKind, IndexReadCapabilities, MaintenanceResult};
use crate::key_codec::{encode_value_key, max_index_key_bytes};
use crate::provider::IndexProvider;
use crate::secondary::extract::{build_secondary_full_key, split_record_id_suffix};

const MODULE_ID: &str = "index";
const RECORD_ID_BYTES: usize = 8;

/// Built-in capability ID for compound and multi-key array indexes.
pub const COMPOUND_PROVIDER_ID: &str = "index.compound";
/// Maximum number of fields in a single compound index.
pub const MAX_COMPOUND_FIELDS: usize = 16;

/// Extracts all unique compound/multi-key value prefixes for `doc` under `desc`.
///
/// Enforces:
/// - `1..=MAX_COMPOUND_FIELDS` fields in `desc.fields`
/// - Parallel-array prohibition: if `>= 2` indexed fields resolve to arrays on `doc`,
///   returns `ErrorCategory::InvalidArgument` (`cannot index parallel arrays`).
/// - Sparse omission: if `desc.sparse` and all indexed fields are missing, returns empty `Vec`.
/// - Within-document deduplication via `BTreeSet<Vec<u8>>`.
pub fn extract_compound_key_prefixes(
    desc: &IndexDescriptor,
    doc: &DocumentView<'_>,
    page_size: PageSize,
) -> Result<Vec<Vec<u8>>> {
    if desc.fields.is_empty() || desc.fields.len() > MAX_COMPOUND_FIELDS {
        return Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "compound::extract_compound_key_prefixes",
            format!(
                "compound index must have 1..={MAX_COMPOUND_FIELDS} fields, got {}",
                desc.fields.len()
            ),
        ));
    }

    let max_prefix = max_index_key_bytes(page_size).saturating_sub(RECORD_ID_BYTES);
    let mut array_field_count = 0usize;
    let mut any_present = false;
    let mut per_field_encoded: Vec<Vec<Vec<u8>>> = Vec::with_capacity(desc.fields.len());

    for field in &desc.fields {
        let extracted = crate::secondary::extract::extract_path_values(doc, &field.path);
        if extracted.encountered_array {
            array_field_count += 1;
            if array_field_count >= 2 {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "compound::extract_compound_key_prefixes",
                    format!(
                        "cannot index parallel arrays in compound index '{}'",
                        desc.name
                    ),
                ));
            }
        }
        if !extracted.values.is_empty() || extracted.empty_array.is_some() {
            any_present = true;
            let mut elem_keys = Vec::with_capacity(
                extracted.values.len() + usize::from(extracted.empty_array.is_some()),
            );
            for el in &extracted.values {
                elem_keys.push(encode_value_key(el, field.order, max_prefix)?);
            }
            if let Some(empty_arr) = extracted.empty_array {
                elem_keys.push(encode_value_key(&empty_arr, field.order, max_prefix)?);
            }
            per_field_encoded.push(elem_keys);
        } else {
            per_field_encoded.push(vec![encode_value_key(
                &ValueView::Null,
                field.order,
                max_prefix,
            )?]);
        }
    }

    if desc.sparse && !any_present {
        return Ok(Vec::new());
    }

    // Cartesian expansion (at most ONE field has len > 1 due to parallel-array prohibition).
    let mut tuples: Vec<Vec<u8>> = vec![Vec::new()];
    for field_vals in &per_field_encoded {
        let mut next = Vec::with_capacity(tuples.len() * field_vals.len());
        for prefix in &tuples {
            for enc in field_vals {
                let mut combined = Vec::with_capacity(prefix.len() + enc.len());
                combined.extend_from_slice(prefix);
                combined.extend_from_slice(enc);
                if combined.len() > max_prefix {
                    return Err(ErrorEnvelope::new(
                        ErrorCategory::Overflow,
                        MODULE_ID,
                        "compound::extract_compound_key_prefixes",
                        format!(
                            "compound index key size {} exceeds max prefix bytes {max_prefix}",
                            combined.len()
                        ),
                    ));
                }
                next.push(combined);
            }
        }
        tuples = next;
    }

    // Deduplicate identical keys emitted by duplicate array elements in the same document.
    let dedup: BTreeSet<Vec<u8>> = tuples.into_iter().collect();
    Ok(dedup.into_iter().collect())
}

/// Scans a compound/multi-key index for all `RecordId`s matching leading equality prefix values,
/// returning deduplicated `RecordId`s.
pub fn scan_compound_prefix_eq(
    store: &impl PageStore,
    desc: &IndexDescriptor,
    prefix_values: &[ValueView<'_>],
) -> Result<Vec<RecordId>> {
    if prefix_values.is_empty() || prefix_values.len() > desc.fields.len() {
        return Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "compound::scan_compound_prefix_eq",
            "prefix_values length must be in 1..=desc.fields.len()",
        ));
    }
    let max_prefix = max_index_key_bytes(store.page_size()).saturating_sub(RECORD_ID_BYTES);
    let mut prefix = Vec::new();
    for (val, field) in prefix_values.iter().zip(desc.fields.iter()) {
        let enc = encode_value_key(val, field.order, max_prefix)?;
        prefix.extend_from_slice(&enc);
    }

    let tree = BTree::open(desc.root_page_id, TreeKind::Index);
    let mut cur = tree.cursor(store);
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    if cur.seek_ge(&prefix)? {
        while let Some((k, _)) = cur.current() {
            let (entry_prefix, rid) = split_record_id_suffix(k)?;
            if !entry_prefix.starts_with(&prefix) {
                break;
            }
            if seen.insert(rid) {
                out.push(rid);
            }
            if !cur.step_next()? {
                break;
            }
        }
    }
    Ok(out)
}

fn check_unique_collision(
    store: &impl PageStore,
    tree: &BTree,
    prefix: &[u8],
    self_rid: RecordId,
    index_name: &str,
) -> Result<()> {
    let mut start = Vec::with_capacity(prefix.len() + RECORD_ID_BYTES);
    start.extend_from_slice(prefix);
    start.extend_from_slice(&[0x00; RECORD_ID_BYTES]);
    let mut cur = tree.cursor(store);
    if cur.seek_ge(&start)? {
        if let Some((k, _)) = cur.current() {
            let (existing_prefix, existing_rid) = split_record_id_suffix(k)?;
            if existing_prefix == prefix && existing_rid != self_rid {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::DuplicateKey,
                    MODULE_ID,
                    "CompoundIndexProvider::check_unique_collision",
                    format!("duplicate key violation on unique compound index '{index_name}'"),
                ));
            }
        }
    }
    Ok(())
}

/// Built-in [`IndexProvider`] for compound (`1..=16` fields) and multi-key array indexes.
#[derive(Debug, Default, Clone, Copy)]
pub struct CompoundIndexProvider;

impl IndexProvider for CompoundIndexProvider {
    fn provider_id(&self) -> CapabilityId {
        CapabilityId::new(COMPOUND_PROVIDER_ID).expect("valid static capability id")
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
        if desc.kind != IndexKind::Compound {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "CompoundIndexProvider::validate_descriptor",
                "CompoundIndexProvider requires IndexKind::Compound",
            ));
        }
        if desc.fields.is_empty() || desc.fields.len() > MAX_COMPOUND_FIELDS {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "CompoundIndexProvider::validate_descriptor",
                format!("compound index requires 1..={MAX_COMPOUND_FIELDS} fields"),
            ));
        }
        if !self.supported_data_versions().contains(desc.data_version) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::UnsupportedFeature,
                MODULE_ID,
                "CompoundIndexProvider::validate_descriptor",
                format!(
                    "unsupported compound index data_version {}",
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
        let prefixes = extract_compound_key_prefixes(desc, doc, tx.page_size())?;
        let mut tree = BTree::open(desc.root_page_id, TreeKind::Index);
        let mut added = 0usize;
        for prefix in &prefixes {
            if desc.unique {
                check_unique_collision(tx, &tree, prefix, record_id, &desc.name)?;
            }
            let full_key = build_secondary_full_key(prefix, record_id);
            tree.insert(tx, &full_key, &[])?;
            added += 1;
        }
        Ok(MaintenanceResult {
            new_root_page_id: tree.root(),
            entries_added: added,
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
            extract_compound_key_prefixes(desc, before, tx.page_size())?
                .into_iter()
                .collect();
        let new_set: BTreeSet<Vec<u8>> =
            extract_compound_key_prefixes(desc, after, tx.page_size())?
                .into_iter()
                .collect();

        if old_set == new_set {
            return Ok(MaintenanceResult {
                new_root_page_id: desc.root_page_id,
                entries_added: 0,
                entries_removed: 0,
            });
        }

        let mut tree = BTree::open(desc.root_page_id, TreeKind::Index);
        let mut removed = 0usize;
        for old_prefix in old_set.difference(&new_set) {
            let full_key = build_secondary_full_key(old_prefix, record_id);
            if tree.delete(tx, &full_key)?.is_some() {
                removed += 1;
            }
        }
        let mut added = 0usize;
        for new_prefix in new_set.difference(&old_set) {
            if desc.unique {
                check_unique_collision(tx, &tree, new_prefix, record_id, &desc.name)?;
            }
            let full_key = build_secondary_full_key(new_prefix, record_id);
            tree.insert(tx, &full_key, &[])?;
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
        let prefixes = extract_compound_key_prefixes(desc, doc, tx.page_size())?;
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
