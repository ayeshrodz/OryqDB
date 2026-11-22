//! Snapshot-isolated [`ReadTransaction`] and [`ReadCollection`] handles.

use oryq_document::{encode_document, DocumentBuf, Value};
use oryq_engine::{BTree, PageStore, ReadTxn, TreeKind};
use oryq_index::{encode_record_id, lookup_primary_id, IndexDescriptor};
use oryq_query::{
    execute_count, execute_find, execute_pipeline_json, explain_analyze, parse_filter_json,
    parse_projection_json, parse_sort_json, ExplainAnalyze, FilterExpr, FindOptions, QueryContext,
    QueryCursor,
};
use oryq_types::{ErrorCategory, ErrorEnvelope, Result, SnapshotId};

use crate::adapters::CoreQuerySource;
use crate::catalog::{list_collection_names, load_collection_entry, CatalogEntry};
use crate::write::decode_collection_record;

const DEFAULT_QUERY_MEMORY_LIMIT: usize = 64 * 1024 * 1024;

const MODULE_ID: &str = "core";

/// Snapshot-isolated read-only transaction over an OryqDB [`crate::Database`].
#[derive(Debug)]
pub struct ReadTransaction<'db> {
    tx: ReadTxn<'db>,
}

impl<'db> ReadTransaction<'db> {
    pub(crate) fn new(tx: ReadTxn<'db>) -> Self {
        Self { tx }
    }

    #[inline]
    pub fn snapshot_id(&self) -> SnapshotId {
        self.tx.snapshot_id()
    }

    pub fn list_collections(&self) -> Result<Vec<String>> {
        list_collection_names(&self.tx, self.tx.catalog_root_page_id())
    }

    pub fn collection(&self, name: &str) -> Result<ReadCollection<'_, 'db>> {
        let entry = load_collection_entry(&self.tx, self.tx.catalog_root_page_id(), name)?
            .ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::NotFound,
                    MODULE_ID,
                    "ReadTransaction::collection",
                    format!("collection '{name}' does not exist"),
                )
            })?;
        Ok(ReadCollection {
            tx: &self.tx,
            entry,
        })
    }
}

/// Snapshot-isolated read handle for a single collection.
#[derive(Debug)]
pub struct ReadCollection<'tx, 'db> {
    tx: &'tx ReadTxn<'db>,
    entry: CatalogEntry,
}

impl ReadCollection<'_, '_> {
    pub fn name(&self) -> &str {
        &self.entry.name
    }

    pub fn find_by_id(&self, id: &Value) -> Result<Option<DocumentBuf>> {
        find_by_id_in_store(self.tx, &self.entry, id)
    }

    pub fn find_all(&self) -> Result<Vec<DocumentBuf>> {
        find_all_in_store(self.tx, &self.entry)
    }

    pub fn count(&self) -> Result<usize> {
        let tree = BTree::open(self.entry.data_root_page_id, TreeKind::Collection);
        Ok(tree.scan_all(self.tx)?.len())
    }

    pub fn list_indexes(&self) -> &[IndexDescriptor] {
        &self.entry.indexes
    }

    /// Executes a structured [`FindOptions`] query within this read snapshot.
    pub fn find(&self, options: &FindOptions) -> Result<QueryCursor> {
        let source = CoreQuerySource::new(self.tx, self.tx.catalog_root_page_id());
        let ctx = QueryContext::default_with_limit(DEFAULT_QUERY_MEMORY_LIMIT);
        execute_find(&source, &self.entry.name, options, &ctx)
    }

    /// Convenience JSON wrapper for `find()` supporting optional filter, projection, sort, skip, and limit.
    pub fn find_json(
        &self,
        filter_json: Option<&str>,
        projection_json: Option<&str>,
        sort_json: Option<&str>,
        skip: u64,
        limit: Option<u64>,
    ) -> Result<QueryCursor> {
        let filter = match filter_json {
            Some(s) if !s.trim().is_empty() => Some(parse_filter_json(s)?),
            _ => None,
        };
        let projection = match projection_json {
            Some(s) if !s.trim().is_empty() => Some(parse_projection_json(s)?),
            _ => None,
        };
        let sort = match sort_json {
            Some(s) if !s.trim().is_empty() => Some(parse_sort_json(s)?),
            _ => None,
        };
        self.find(&FindOptions {
            filter,
            projection,
            sort,
            skip,
            limit,
        })
    }

    /// Counts documents matching `filter` within this read snapshot.
    pub fn count_matching(&self, filter: Option<&FilterExpr>) -> Result<u64> {
        let source = CoreQuerySource::new(self.tx, self.tx.catalog_root_page_id());
        let ctx = QueryContext::default_with_limit(DEFAULT_QUERY_MEMORY_LIMIT);
        execute_count(&source, &self.entry.name, filter, &ctx)
    }

    /// Executes `options` and returns the chosen plan alongside execution counters.
    pub fn explain_analyze(&self, options: &FindOptions) -> Result<ExplainAnalyze> {
        let source = CoreQuerySource::new(self.tx, self.tx.catalog_root_page_id());
        let ctx = QueryContext::default_with_limit(DEFAULT_QUERY_MEMORY_LIMIT);
        explain_analyze(&source, &self.entry.name, options, &ctx)
    }

    /// Executes a JSON aggregation pipeline within this pinned read snapshot.
    pub fn aggregate_json(&self, pipeline_json: &str) -> Result<Vec<DocumentBuf>> {
        let source = CoreQuerySource::new(self.tx, self.tx.catalog_root_page_id());
        let ctx = QueryContext::default_with_limit(DEFAULT_QUERY_MEMORY_LIMIT);
        execute_pipeline_json(&source, &self.entry.name, pipeline_json, &ctx)
    }

    /// Executes an Okapi BM25 full-text search against a named `index.fts` index (`--features fts`).
    #[cfg(feature = "fts")]
    pub fn search_fts(
        &self,
        index_name: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<(DocumentBuf, f64)>> {
        let desc = self
            .entry
            .indexes
            .iter()
            .find(|i| i.name == index_name && i.provider_id.as_str() == oryq_fts::FTS_PROVIDER_ID)
            .ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::NotFound,
                    MODULE_ID,
                    "ReadCollection::search_fts",
                    format!(
                        "FTS index '{index_name}' not found on collection '{}'",
                        self.entry.name
                    ),
                )
            })?;
        let hits = oryq_fts::search_fts(self.tx, desc, query, limit)?;
        let data_tree = BTree::open(self.entry.data_root_page_id, TreeKind::Collection);
        let mut results = Vec::with_capacity(hits.len());
        for hit in hits {
            let rec_key = encode_record_id(hit.record_id);
            if let Some(raw) = data_tree.get(self.tx, &rec_key)? {
                results.push((decode_collection_record(&raw)?, hit.score));
            }
        }
        Ok(results)
    }
}

pub(crate) fn find_by_id_in_store(
    store: &impl PageStore,
    entry: &CatalogEntry,
    id: &Value,
) -> Result<Option<DocumentBuf>> {
    let primary_root = entry.primary_index()?.root_page_id;
    let (id_doc, _) = encode_document(&[("_id".into(), id.clone())], None)?;
    let id_view = id_doc.view().get_field("_id").unwrap();
    let Some(record_id) = lookup_primary_id(store, primary_root, &id_view)? else {
        return Ok(None);
    };
    let data_tree = BTree::open(entry.data_root_page_id, TreeKind::Collection);
    let rec_key = encode_record_id(record_id);
    match data_tree.get(store, &rec_key)? {
        Some(raw) => Ok(Some(decode_collection_record(&raw)?)),
        None => Err(ErrorEnvelope::new(
            ErrorCategory::Corruption,
            MODULE_ID,
            "api::find_by_id_in_store",
            format!(
                "primary index points to missing RecordId({})",
                record_id.raw()
            ),
        )),
    }
}

pub(crate) fn find_all_in_store(
    store: &impl PageStore,
    entry: &CatalogEntry,
) -> Result<Vec<DocumentBuf>> {
    let data_tree = BTree::open(entry.data_root_page_id, TreeKind::Collection);
    let pairs = data_tree.scan_all(store)?;
    let mut docs = Vec::with_capacity(pairs.len());
    for (_, raw) in pairs {
        docs.push(decode_collection_record(&raw)?);
    }
    Ok(docs)
}
