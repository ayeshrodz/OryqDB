//! Snapshot-isolated [`ReadTransaction`] and [`ReadCollection`] handles.

use std::cell::RefCell;
use std::collections::HashMap;

use oryq_document::{encode_document, DocumentBuf, Value, ValueView};
use oryq_engine::{BTree, PageStore, ReadTxn, TreeKind};
use oryq_index::{encode_record_id, lookup_primary_id, IndexDescriptor};
use oryq_query::{
    execute_count, execute_find, execute_pipeline_json, explain_analyze, parse_filter_json,
    parse_projection_json, parse_sort_json, AccessPlan, ExplainAnalyze, ExplainPlan, FilterExpr,
    FindOptions, QueryContext, QueryCursor,
};
use oryq_types::{ErrorCategory, ErrorEnvelope, Result, SnapshotId};

use crate::adapters::CoreQuerySource;
use crate::catalog::{list_collection_names, load_collection_entry, CatalogEntry};
use crate::write::decode_collection_record_from_store;

const DEFAULT_QUERY_MEMORY_LIMIT: usize = 64 * 1024 * 1024;

const MODULE_ID: &str = "core";

/// Snapshot-isolated read-only transaction over an OryqDB [`crate::Database`].
#[derive(Debug)]
pub struct ReadTransaction<'db> {
    tx: ReadTxn<'db>,
    catalog_cache: RefCell<HashMap<String, CatalogEntry>>,
}

impl<'db> ReadTransaction<'db> {
    pub(crate) fn new(tx: ReadTxn<'db>) -> Self {
        Self {
            tx,
            catalog_cache: RefCell::new(HashMap::new()),
        }
    }

    #[inline]
    pub fn snapshot_id(&self) -> SnapshotId {
        self.tx.snapshot_id()
    }

    pub fn list_collections(&self) -> Result<Vec<String>> {
        list_collection_names(&self.tx, self.tx.catalog_root_page_id())
    }

    pub fn collection(&self, name: &str) -> Result<ReadCollection<'_, 'db>> {
        if let Some(entry) = self.catalog_cache.borrow().get(name) {
            return Ok(ReadCollection {
                tx: &self.tx,
                entry: entry.clone(),
            });
        }
        let entry = load_collection_entry(&self.tx, self.tx.catalog_root_page_id(), name)?
            .ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::NotFound,
                    MODULE_ID,
                    "ReadTransaction::collection",
                    format!("collection '{name}' does not exist"),
                )
            })?;
        self.catalog_cache
            .borrow_mut()
            .insert(name.to_owned(), entry.clone());
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

    pub(crate) fn raw_tx(&self) -> &ReadTxn<'_> {
        self.tx
    }

    pub(crate) fn catalog_entry(&self) -> &CatalogEntry {
        &self.entry
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
        // Fast path for direct primary `_id` equality lookups without sort/projection/skip/zero-limit.
        if options.sort.is_none()
            && options.projection.is_none()
            && options.skip == 0
            && options.limit != Some(0)
        {
            if let Some(FilterExpr::Eq(path, val)) = &options.filter {
                if path.as_str() == "_id" {
                    let plan = ExplainPlan {
                        collection: self.entry.name.clone(),
                        access: AccessPlan::PrimaryIdLookup {
                            target: val.clone(),
                        },
                        residual_filter: None,
                    };
                    let rows = match find_by_id_in_store(self.tx, &self.entry, val)? {
                        Some(doc) => vec![doc],
                        None => Vec::new(),
                    };
                    return Ok(QueryCursor::from_rows(rows, plan));
                }
            }
        }
        let source = CoreQuerySource::with_entry(
            self.tx,
            self.tx.catalog_root_page_id(),
            self.entry.clone(),
        );
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
        let source = CoreQuerySource::with_entry(
            self.tx,
            self.tx.catalog_root_page_id(),
            self.entry.clone(),
        );
        let ctx = QueryContext::default_with_limit(DEFAULT_QUERY_MEMORY_LIMIT);
        execute_count(&source, &self.entry.name, filter, &ctx)
    }

    /// Executes `options` and returns the chosen plan alongside execution counters.
    pub fn explain_analyze(&self, options: &FindOptions) -> Result<ExplainAnalyze> {
        let source = CoreQuerySource::with_entry(
            self.tx,
            self.tx.catalog_root_page_id(),
            self.entry.clone(),
        );
        let ctx = QueryContext::default_with_limit(DEFAULT_QUERY_MEMORY_LIMIT);
        explain_analyze(&source, &self.entry.name, options, &ctx)
    }

    /// Executes a JSON aggregation pipeline within this pinned read snapshot.
    pub fn aggregate_json(&self, pipeline_json: &str) -> Result<Vec<DocumentBuf>> {
        let source = CoreQuerySource::with_entry(
            self.tx,
            self.tx.catalog_root_page_id(),
            self.entry.clone(),
        );
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
                results.push((
                    decode_collection_record_from_store(self.tx, &raw)?,
                    hit.score,
                ));
            }
        }
        Ok(results)
    }

    /// Executes a top-`limit` nearest-neighbor vector similarity search against a named
    /// `index.vec` index (`--features vec`), returning `(document, distance)` pairs ordered by
    /// ascending distance.
    #[cfg(feature = "vec")]
    pub fn search_vector(
        &self,
        index_name: &str,
        query: &[f32],
        limit: usize,
    ) -> Result<Vec<(DocumentBuf, f64)>> {
        let desc = self
            .entry
            .indexes
            .iter()
            .find(|i| i.name == index_name && i.provider_id.as_str() == oryq_vec::VEC_PROVIDER_ID)
            .ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::NotFound,
                    MODULE_ID,
                    "ReadCollection::search_vector",
                    format!(
                        "vector index '{index_name}' not found on collection '{}'",
                        self.entry.name
                    ),
                )
            })?;
        let hits = oryq_vec::search_vec(self.tx, desc, query, limit)?;
        let data_tree = BTree::open(self.entry.data_root_page_id, TreeKind::Collection);
        let mut results = Vec::with_capacity(hits.len());
        for hit in hits {
            let rec_key = encode_record_id(hit.record_id);
            if let Some(raw) = data_tree.get(self.tx, &rec_key)? {
                results.push((
                    decode_collection_record_from_store(self.tx, &raw)?,
                    hit.distance,
                ));
            }
        }
        Ok(results)
    }
}

/// Zero-allocation [`ValueView`] borrow for scalar [`Value`]s; falls back to `encode_document`
/// only for nested `Array` / `Document` values.
pub(crate) fn with_value_view<T>(
    val: &Value,
    f: impl FnOnce(&ValueView<'_>) -> Result<T>,
) -> Result<T> {
    match val {
        Value::Null => f(&ValueView::Null),
        Value::Bool(b) => f(&ValueView::Bool(*b)),
        Value::Int64(n) => f(&ValueView::Int64(*n)),
        Value::Float64(x) => f(&ValueView::Float64(*x)),
        Value::DateTimeUtcMs(ms) => f(&ValueView::DateTimeUtcMs(*ms)),
        Value::String(s) => f(&ValueView::String(s.as_str())),
        Value::Binary { subtype, bytes } => f(&ValueView::Binary {
            subtype: *subtype,
            bytes: bytes.as_slice(),
        }),
        Value::ObjectId(oid) => f(&ValueView::ObjectId(*oid)),
        Value::Uuid(u) => f(&ValueView::Uuid(*u)),
        Value::Decimal128(d) => f(&ValueView::Decimal128(*d)),
        Value::Array(_) | Value::Document(_) => {
            let (doc, _) = encode_document(&[("v".to_owned(), val.clone())], None)?;
            let view = doc.view().get_field("v").unwrap();
            f(&view)
        }
    }
}

pub(crate) fn find_by_id_in_store(
    store: &impl PageStore,
    entry: &CatalogEntry,
    id: &Value,
) -> Result<Option<DocumentBuf>> {
    let primary_root = entry.primary_index()?.root_page_id;
    let Some(record_id) = with_value_view(id, |id_view| {
        lookup_primary_id(store, primary_root, id_view)
    })?
    else {
        return Ok(None);
    };
    let data_tree = BTree::open(entry.data_root_page_id, TreeKind::Collection);
    let rec_key = encode_record_id(record_id);
    match data_tree.get(store, &rec_key)? {
        Some(raw) => Ok(Some(decode_collection_record_from_store(store, &raw)?)),
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
        docs.push(decode_collection_record_from_store(store, &raw)?);
    }
    Ok(docs)
}
