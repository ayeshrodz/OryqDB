//! Transactional [`oryq_index::IndexProvider`] implementation (`index.vec`) backed by
//! restricted [`IndexWriteContext`] B+ tree storage.

use oryq_document::{DocumentView, FieldPath, ValueView};
use oryq_engine::btree::{build_leaf_page, LeafEntry};
use oryq_engine::{BTree, PageStore, PageStoreMut, TreeKind, WriteTxn};
use oryq_index::{
    IndexDescriptor, IndexFieldSpec, IndexKind, IndexProvider, IndexReadCapabilities,
    IndexWriteContext, MaintenanceResult, SortOrder,
};
use oryq_types::{
    CapabilityId, ErrorCategory, ErrorEnvelope, IndexId, NamespaceId, PageId, RecordId, Result,
    VersionRange,
};

use crate::codec::{
    encode_vec_key, encode_vector_payload, encoded_vector_payload_len, VecIndexMeta, KEY_META,
};
use crate::distance::{validate_vector, VectorIndexOptions};

const MODULE_ID: &str = "vec";

/// Built-in capability ID for the optional vector similarity search index provider.
pub const VEC_PROVIDER_ID: &str = "index.vec";
/// Persistent write-feature bit allocated to `oryq.index.vector.v1` in `architecture/capabilities.toml`.
pub const VEC_FEATURE_BIT: u8 = 1;

/// Constructs a canonical [`IndexDescriptor`] for a vector similarity index over a single field.
pub fn vec_index_descriptor(
    index_id: IndexId,
    name: String,
    field: FieldPath,
    options: VectorIndexOptions,
    root_page_id: PageId,
) -> Result<IndexDescriptor> {
    options.validate()?;
    Ok(IndexDescriptor {
        index_id,
        name,
        kind: IndexKind::Pluggable("vec".to_owned()),
        fields: vec![IndexFieldSpec {
            path: field,
            order: SortOrder::Asc,
        }],
        unique: false,
        sparse: true,
        root_page_id,
        required_feature_bit: Some(VEC_FEATURE_BIT),
        provider_id: CapabilityId::new(VEC_PROVIDER_ID).expect("valid static capability id"),
        data_version: 1,
        options_json: Some(options.to_json()),
    })
}

pub(crate) fn parse_descriptor_options(desc: &IndexDescriptor) -> Result<VectorIndexOptions> {
    let raw = desc.options_json.as_deref().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "parse_descriptor_options",
            "vector index descriptor missing options_json",
        )
    })?;
    VectorIndexOptions::from_json(raw)
}

/// Extracts and validates an optional vector field from `doc`.
///
/// - Missing field or `ValueView::Null` -> `Ok(None)` (sparse index skips document).
/// - Array of numeric values (`Float64` or `Int64`) with matching length and finite components -> `Ok(Some(vec))`.
/// - Non-array, wrong length, non-numeric element, or non-finite float -> `Err(InvalidArgument)`.
fn extract_doc_vector(
    desc: &IndexDescriptor,
    options: &VectorIndexOptions,
    doc: &DocumentView<'_>,
) -> Result<Option<Vec<f32>>> {
    let field = desc.fields.first().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "extract_doc_vector",
            "vector index descriptor missing field path",
        )
    })?;
    match doc.get_path(&field.path) {
        None | Some(ValueView::Null) => Ok(None),
        Some(ValueView::Array(arr)) => {
            let mut out = Vec::with_capacity(options.dimensions);
            for item in arr.iter() {
                let val = match item {
                    ValueView::Float64(f) => f as f32,
                    ValueView::Int64(i) => i as f32,
                    _ => {
                        return Err(ErrorEnvelope::new(
                            ErrorCategory::InvalidArgument,
                            MODULE_ID,
                            "extract_doc_vector",
                            format!(
                                "vector field '{}' must contain only numeric elements",
                                field.path.as_str()
                            ),
                        ));
                    }
                };
                out.push(val);
            }
            validate_vector(&out, options.dimensions, "extract_doc_vector")?;
            Ok(Some(out))
        }
        Some(_) => Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "extract_doc_vector",
            format!(
                "vector field '{}' must be a numeric array or null",
                field.path.as_str()
            ),
        )),
    }
}

fn load_meta(
    tree: &BTree,
    store: &impl PageStore,
    options: &VectorIndexOptions,
) -> Result<VecIndexMeta> {
    match tree.get(store, KEY_META)? {
        Some(raw) => VecIndexMeta::decode(&raw),
        None => Ok(VecIndexMeta {
            vector_count: 0,
            dimensions: options.dimensions as u32,
            metric: options.metric,
            quantization: options.quantization,
        }),
    }
}

/// Optional vector similarity search [`IndexProvider`] (`index.vec`).
#[derive(Debug, Default, Clone, Copy)]
pub struct VectorIndexProvider;

impl IndexProvider for VectorIndexProvider {
    fn provider_id(&self) -> CapabilityId {
        CapabilityId::new(VEC_PROVIDER_ID).expect("valid static capability id")
    }

    fn supported_data_versions(&self) -> VersionRange {
        VersionRange::new(1, 1).expect("valid range")
    }

    fn capabilities(&self, _desc: &IndexDescriptor) -> IndexReadCapabilities {
        IndexReadCapabilities {
            exact_equality: false,
            ordered_range: false,
            multikey: false,
            sparse: true,
        }
    }

    fn validate_descriptor(&self, desc: &IndexDescriptor) -> Result<()> {
        if desc.kind != IndexKind::Pluggable("vec".to_owned()) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "VectorIndexProvider::validate_descriptor",
                "VectorIndexProvider requires IndexKind::Pluggable(\"vec\")",
            ));
        }
        if desc.fields.len() != 1 {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "VectorIndexProvider::validate_descriptor",
                "vector index requires exactly one target field path",
            ));
        }
        if desc.unique {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "VectorIndexProvider::validate_descriptor",
                "vector index cannot be unique",
            ));
        }
        if desc.required_feature_bit != Some(VEC_FEATURE_BIT) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "VectorIndexProvider::validate_descriptor",
                format!("vector index requires required_feature_bit == Some({VEC_FEATURE_BIT})"),
            ));
        }
        if !self.supported_data_versions().contains(desc.data_version) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::UnsupportedFeature,
                MODULE_ID,
                "VectorIndexProvider::validate_descriptor",
                format!(
                    "unsupported vector index data_version {}",
                    desc.data_version
                ),
            ));
        }
        let _ = parse_descriptor_options(desc)?;
        Ok(())
    }

    fn create_empty(&self, tx: &mut WriteTxn<'_>, desc: &IndexDescriptor) -> Result<PageId> {
        self.validate_descriptor(desc)?;
        let options = parse_descriptor_options(desc)?;
        let ns = NamespaceId::new(desc.index_id.raw())?;
        let mut ctx = IndexWriteContext::new(ns, tx);

        // Fail fast if a single serialized vector entry cannot fit on one B+ tree leaf page.
        let payload_len = encoded_vector_payload_len(options.dimensions, options.quantization);
        let probe = [LeafEntry {
            key: vec![0u8; 9],
            value: vec![0u8; payload_len],
        }];
        if build_leaf_page(
            ctx.page_size(),
            PageId::new(1),
            TreeKind::Index,
            true,
            ctx.writer_tx_id(),
            &probe,
        )?
        .is_none()
        {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "VectorIndexProvider::create_empty",
                format!(
                    "vector entry ({payload_len}-byte payload for {} dims / {:?}) exceeds {:?} leaf page capacity; reduce dimensions, use Int8 quantization, or increase page_size",
                    options.dimensions,
                    options.quantization,
                    ctx.page_size()
                ),
            ));
        }

        let mut tree = BTree::create_empty(&mut ctx, TreeKind::Index)?;
        let meta = VecIndexMeta {
            vector_count: 0,
            dimensions: options.dimensions as u32,
            metric: options.metric,
            quantization: options.quantization,
        };
        tree.insert(&mut ctx, KEY_META, &meta.encode())?;
        Ok(tree.root())
    }

    fn on_insert(
        &self,
        tx: &mut WriteTxn<'_>,
        desc: &IndexDescriptor,
        record_id: RecordId,
        doc: &DocumentView<'_>,
    ) -> Result<MaintenanceResult> {
        let options = parse_descriptor_options(desc)?;
        let Some(vec) = extract_doc_vector(desc, &options, doc)? else {
            return Ok(MaintenanceResult {
                new_root_page_id: desc.root_page_id,
                entries_added: 0,
                entries_removed: 0,
            });
        };

        let ns = NamespaceId::new(desc.index_id.raw())?;
        let mut ctx = IndexWriteContext::new(ns, tx);
        let mut tree = BTree::open(desc.root_page_id, TreeKind::Index);

        let key = encode_vec_key(record_id);
        let payload = encode_vector_payload(&vec, options.quantization);
        let existed = tree.insert(&mut ctx, &key, &payload)?.is_some();
        if !existed {
            let mut meta = load_meta(&tree, &ctx, &options)?;
            meta.vector_count = meta.vector_count.saturating_add(1);
            tree.insert(&mut ctx, KEY_META, &meta.encode())?;
        }

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
        let options = parse_descriptor_options(desc)?;
        let old_vec = extract_doc_vector(desc, &options, before)?;
        let new_vec = extract_doc_vector(desc, &options, after)?;

        if old_vec == new_vec {
            return Ok(MaintenanceResult {
                new_root_page_id: desc.root_page_id,
                entries_added: 0,
                entries_removed: 0,
            });
        }

        let ns = NamespaceId::new(desc.index_id.raw())?;
        let mut ctx = IndexWriteContext::new(ns, tx);
        let mut tree = BTree::open(desc.root_page_id, TreeKind::Index);
        let key = encode_vec_key(record_id);

        match new_vec {
            Some(vec) => {
                let payload = encode_vector_payload(&vec, options.quantization);
                let existed = tree.insert(&mut ctx, &key, &payload)?.is_some();
                if !existed {
                    let mut meta = load_meta(&tree, &ctx, &options)?;
                    meta.vector_count = meta.vector_count.saturating_add(1);
                    tree.insert(&mut ctx, KEY_META, &meta.encode())?;
                }
                Ok(MaintenanceResult {
                    new_root_page_id: tree.root(),
                    entries_added: 1,
                    entries_removed: usize::from(existed),
                })
            }
            None => {
                let removed = tree.delete(&mut ctx, &key)?.is_some();
                if removed {
                    let mut meta = load_meta(&tree, &ctx, &options)?;
                    meta.vector_count = meta.vector_count.saturating_sub(1);
                    tree.insert(&mut ctx, KEY_META, &meta.encode())?;
                }
                Ok(MaintenanceResult {
                    new_root_page_id: tree.root(),
                    entries_added: 0,
                    entries_removed: usize::from(removed),
                })
            }
        }
    }

    fn on_delete(
        &self,
        tx: &mut WriteTxn<'_>,
        desc: &IndexDescriptor,
        record_id: RecordId,
        _doc: &DocumentView<'_>,
    ) -> Result<MaintenanceResult> {
        let options = parse_descriptor_options(desc)?;
        let ns = NamespaceId::new(desc.index_id.raw())?;
        let mut ctx = IndexWriteContext::new(ns, tx);
        let mut tree = BTree::open(desc.root_page_id, TreeKind::Index);
        let key = encode_vec_key(record_id);

        let removed = tree.delete(&mut ctx, &key)?.is_some();
        if removed {
            let mut meta = load_meta(&tree, &ctx, &options)?;
            meta.vector_count = meta.vector_count.saturating_sub(1);
            tree.insert(&mut ctx, KEY_META, &meta.encode())?;
        }

        Ok(MaintenanceResult {
            new_root_page_id: tree.root(),
            entries_added: 0,
            entries_removed: usize::from(removed),
        })
    }

    fn drop_index(&self, tx: &mut WriteTxn<'_>, desc: &IndexDescriptor) -> Result<()> {
        let ns = NamespaceId::new(desc.index_id.raw())?;
        let mut ctx = IndexWriteContext::new(ns, tx);
        BTree::open(desc.root_page_id, TreeKind::Index).free_all_pages(&mut ctx)
    }
}
