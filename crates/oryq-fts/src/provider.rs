//! Transactional [`oryq_index::IndexProvider`] implementation (`index.fts`) backed by
//! restricted [`IndexWriteContext`] storage.

use std::collections::{BTreeMap, HashSet};

use oryq_document::{DocumentView, FieldPath, ValueView};
use oryq_engine::btree::read_internal_node;
use oryq_engine::{BTree, PageStore, PageStoreMut, SlottedPageView, TreeKind, WriteTxn};
use oryq_index::{
    IndexDescriptor, IndexFieldSpec, IndexKind, IndexProvider, IndexReadCapabilities,
    IndexWriteContext, MaintenanceResult, SortOrder,
};
use oryq_types::{
    CapabilityId, ErrorCategory, ErrorEnvelope, IndexId, NamespaceId, PageId, RecordId, Result,
    VersionRange,
};

use crate::codec::{decode_u32_le, encode_doc_len_key, encode_posting_key, CorpusStats, KEY_STATS};
use crate::tokenizer::count_terms;

const MODULE_ID: &str = "fts";

/// Built-in capability ID for the optional full-text search index provider.
pub const FTS_PROVIDER_ID: &str = "index.fts";
/// Persistent write-feature bit allocated to `oryq.index.fts.v1` in `architecture/capabilities.toml`.
pub const FTS_FEATURE_BIT: u8 = 0;

/// Constructs a canonical [`IndexDescriptor`] for an FTS index over one or more text fields.
pub fn fts_index_descriptor(
    index_id: IndexId,
    name: String,
    fields: Vec<FieldPath>,
    root_page_id: PageId,
) -> IndexDescriptor {
    IndexDescriptor {
        index_id,
        name,
        kind: IndexKind::Pluggable("fts".to_owned()),
        fields: fields
            .into_iter()
            .map(|path| IndexFieldSpec {
                path,
                order: SortOrder::Asc,
            })
            .collect(),
        unique: false,
        sparse: true,
        root_page_id,
        required_feature_bit: Some(FTS_FEATURE_BIT),
        provider_id: CapabilityId::new(FTS_PROVIDER_ID).expect("valid static capability id"),
        data_version: 1,
        options_json: None,
    }
}

fn extract_doc_terms(
    desc: &IndexDescriptor,
    doc: &DocumentView<'_>,
) -> (BTreeMap<String, u32>, u32) {
    let mut texts: Vec<String> = Vec::new();
    for field in &desc.fields {
        match doc.get_path(&field.path) {
            Some(ValueView::String(s)) => texts.push(s.to_owned()),
            Some(ValueView::Array(arr)) => {
                for item in arr.iter() {
                    if let ValueView::String(s) = item {
                        texts.push(s.to_owned());
                    }
                }
            }
            _ => {}
        }
    }
    count_terms(texts.iter().map(String::as_str))
}

fn load_stats(tree: &BTree, store: &impl PageStore) -> Result<CorpusStats> {
    match tree.get(store, KEY_STATS)? {
        Some(raw) => CorpusStats::decode(&raw),
        None => Ok(CorpusStats::default()),
    }
}

/// Optional full-text search [`IndexProvider`] (`index.fts`) implementing Okapi BM25 inverted indexing.
#[derive(Debug, Default, Clone, Copy)]
pub struct FtsIndexProvider;

impl IndexProvider for FtsIndexProvider {
    fn provider_id(&self) -> CapabilityId {
        CapabilityId::new(FTS_PROVIDER_ID).expect("valid static capability id")
    }

    fn supported_data_versions(&self) -> VersionRange {
        VersionRange::new(1, 1).expect("valid range")
    }

    fn capabilities(&self, _desc: &IndexDescriptor) -> IndexReadCapabilities {
        IndexReadCapabilities {
            exact_equality: false,
            ordered_range: false,
            multikey: true,
            sparse: true,
        }
    }

    fn validate_descriptor(&self, desc: &IndexDescriptor) -> Result<()> {
        if desc.kind != IndexKind::Pluggable("fts".to_owned()) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "FtsIndexProvider::validate_descriptor",
                "FtsIndexProvider requires IndexKind::Pluggable(\"fts\")",
            ));
        }
        if desc.fields.is_empty() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "FtsIndexProvider::validate_descriptor",
                "FTS index requires at least one text field path",
            ));
        }
        if desc.unique {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "FtsIndexProvider::validate_descriptor",
                "FTS index cannot be unique",
            ));
        }
        if desc.required_feature_bit != Some(FTS_FEATURE_BIT) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "FtsIndexProvider::validate_descriptor",
                format!("FTS index requires required_feature_bit == Some({FTS_FEATURE_BIT})"),
            ));
        }
        if !self.supported_data_versions().contains(desc.data_version) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::UnsupportedFeature,
                MODULE_ID,
                "FtsIndexProvider::validate_descriptor",
                format!("unsupported FTS index data_version {}", desc.data_version),
            ));
        }
        Ok(())
    }

    fn create_empty(&self, tx: &mut WriteTxn<'_>, desc: &IndexDescriptor) -> Result<PageId> {
        self.validate_descriptor(desc)?;
        let ns = NamespaceId::new(desc.index_id.raw())?;
        let mut ctx = IndexWriteContext::new(ns, tx);
        let mut tree = BTree::create_empty(&mut ctx, TreeKind::Index)?;
        let stats = CorpusStats::default().encode();
        tree.insert(&mut ctx, KEY_STATS, &stats)?;
        Ok(tree.root())
    }

    fn on_insert(
        &self,
        tx: &mut WriteTxn<'_>,
        desc: &IndexDescriptor,
        record_id: RecordId,
        doc: &DocumentView<'_>,
    ) -> Result<MaintenanceResult> {
        let (freqs, total_tokens) = extract_doc_terms(desc, doc);
        if total_tokens == 0 {
            return Ok(MaintenanceResult {
                new_root_page_id: desc.root_page_id,
                entries_added: 0,
                entries_removed: 0,
            });
        }

        let ns = NamespaceId::new(desc.index_id.raw())?;
        let mut ctx = IndexWriteContext::new(ns, tx);
        let mut tree = BTree::open(desc.root_page_id, TreeKind::Index);
        let mut stats = load_stats(&tree, &ctx)?;

        for (term, tf) in &freqs {
            let key = encode_posting_key(term, record_id);
            tree.insert(&mut ctx, &key, &tf.to_le_bytes())?;
        }
        let dl_key = encode_doc_len_key(record_id);
        tree.insert(&mut ctx, &dl_key, &total_tokens.to_le_bytes())?;

        stats.doc_count = stats.doc_count.saturating_add(1);
        stats.total_tokens = stats.total_tokens.saturating_add(u64::from(total_tokens));
        tree.insert(&mut ctx, KEY_STATS, &stats.encode())?;

        Ok(MaintenanceResult {
            new_root_page_id: tree.root(),
            entries_added: freqs.len(),
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
        let (old_freqs, _) = extract_doc_terms(desc, before);
        let (new_freqs, new_total) = extract_doc_terms(desc, after);

        if old_freqs == new_freqs {
            return Ok(MaintenanceResult {
                new_root_page_id: desc.root_page_id,
                entries_added: 0,
                entries_removed: 0,
            });
        }

        let ns = NamespaceId::new(desc.index_id.raw())?;
        let mut ctx = IndexWriteContext::new(ns, tx);
        let mut tree = BTree::open(desc.root_page_id, TreeKind::Index);
        let mut stats = load_stats(&tree, &ctx)?;

        let mut removed = 0usize;
        for term in old_freqs.keys() {
            if !new_freqs.contains_key(term) {
                let key = encode_posting_key(term, record_id);
                if tree.delete(&mut ctx, &key)?.is_some() {
                    removed += 1;
                }
            }
        }

        let mut added = 0usize;
        for (term, &new_tf) in &new_freqs {
            if old_freqs.get(term).copied() != Some(new_tf) {
                let key = encode_posting_key(term, record_id);
                tree.insert(&mut ctx, &key, &new_tf.to_le_bytes())?;
                added += 1;
            }
        }

        let dl_key = encode_doc_len_key(record_id);
        let prev_dl = match tree.get(&ctx, &dl_key)? {
            Some(raw) => Some(decode_u32_le(&raw, "FtsIndexProvider::on_update")?),
            None => None,
        };

        if let Some(prev) = prev_dl {
            stats.total_tokens = stats.total_tokens.saturating_sub(u64::from(prev));
            if new_total == 0 {
                tree.delete(&mut ctx, &dl_key)?;
                stats.doc_count = stats.doc_count.saturating_sub(1);
            } else {
                tree.insert(&mut ctx, &dl_key, &new_total.to_le_bytes())?;
                stats.total_tokens = stats.total_tokens.saturating_add(u64::from(new_total));
            }
        } else if new_total > 0 {
            tree.insert(&mut ctx, &dl_key, &new_total.to_le_bytes())?;
            stats.doc_count = stats.doc_count.saturating_add(1);
            stats.total_tokens = stats.total_tokens.saturating_add(u64::from(new_total));
        }

        tree.insert(&mut ctx, KEY_STATS, &stats.encode())?;

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
        let (old_freqs, _) = extract_doc_terms(desc, doc);
        let ns = NamespaceId::new(desc.index_id.raw())?;
        let mut ctx = IndexWriteContext::new(ns, tx);
        let mut tree = BTree::open(desc.root_page_id, TreeKind::Index);

        let dl_key = encode_doc_len_key(record_id);
        let Some(raw_dl) = tree.get(&ctx, &dl_key)? else {
            return Ok(MaintenanceResult {
                new_root_page_id: desc.root_page_id,
                entries_added: 0,
                entries_removed: 0,
            });
        };
        let prev_dl = decode_u32_le(&raw_dl, "FtsIndexProvider::on_delete")?;
        tree.delete(&mut ctx, &dl_key)?;

        let mut removed = 0usize;
        for term in old_freqs.keys() {
            let key = encode_posting_key(term, record_id);
            if tree.delete(&mut ctx, &key)?.is_some() {
                removed += 1;
            }
        }

        let mut stats = load_stats(&tree, &ctx)?;
        stats.doc_count = stats.doc_count.saturating_sub(1);
        stats.total_tokens = stats.total_tokens.saturating_sub(u64::from(prev_dl));
        tree.insert(&mut ctx, KEY_STATS, &stats.encode())?;

        Ok(MaintenanceResult {
            new_root_page_id: tree.root(),
            entries_added: 0,
            entries_removed: removed,
        })
    }

    fn drop_index(&self, tx: &mut WriteTxn<'_>, desc: &IndexDescriptor) -> Result<()> {
        let ns = NamespaceId::new(desc.index_id.raw())?;
        let mut ctx = IndexWriteContext::new(ns, tx);
        let mut stack = vec![desc.root_page_id];
        let mut visited = HashSet::new();
        while let Some(pid) = stack.pop() {
            if !visited.insert(pid) {
                continue;
            }
            let bytes = ctx.read_page(pid)?;
            let view = SlottedPageView::parse(&bytes, pid)?;
            if TreeKind::is_internal(view.header().page_type) {
                let node = read_internal_node(&bytes, pid)?;
                stack.push(node.leftmost_child);
                for (_, child) in node.separators {
                    stack.push(child);
                }
            }
            ctx.free_page(pid)?;
        }
        Ok(())
    }
}
