//! [`CoreQuerySource`] adapter implementing [`oryq_query::QuerySource`] over a snapshot-pinned
//! [`PageStore`] and catalog root page.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;

use oryq_document::{DocumentBuf, Value};
use oryq_engine::{BTree, PageStore, TreeKind};
use oryq_index::{
    decode_record_id, encode_record_id, lookup_primary_id, scan_compound_prefix_eq,
    scan_secondary_eq, scan_secondary_range, IndexDescriptor, IndexKind,
};
use oryq_query::QuerySource;
use oryq_types::{ErrorCategory, ErrorEnvelope, PageId, RecordId, Result};

use crate::api::read::with_value_view;
use crate::catalog::{load_collection_entry, CatalogEntry};
use crate::write::decode_collection_record;

const MODULE_ID: &str = "core";

/// Snapshot-pinned [`QuerySource`] backed by any [`PageStore`] (e.g. `ReadTxn` or `WriteTxn`).
///
/// Caches decoded [`CatalogEntry`] records per collection for the lifetime of this single-snapshot
/// source so multi-row index scans and `$lookup` stages never re-read the catalog B+ tree per row.
pub struct CoreQuerySource<'a, S: PageStore> {
    store: &'a S,
    catalog_root: PageId,
    entry_cache: RefCell<HashMap<String, CatalogEntry>>,
}

impl<'a, S: PageStore> fmt::Debug for CoreQuerySource<'a, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CoreQuerySource")
            .field("catalog_root", &self.catalog_root)
            .finish()
    }
}

impl<'a, S: PageStore> CoreQuerySource<'a, S> {
    pub fn new(store: &'a S, catalog_root: PageId) -> Self {
        Self {
            store,
            catalog_root,
            entry_cache: RefCell::new(HashMap::new()),
        }
    }

    /// Creates a [`CoreQuerySource`] pre-seeded with an already-loaded [`CatalogEntry`] for the
    /// target collection, avoiding even the initial catalog B+ tree lookup.
    pub fn with_entry(store: &'a S, catalog_root: PageId, entry: CatalogEntry) -> Self {
        let mut cache = HashMap::with_capacity(1);
        cache.insert(entry.name.clone(), entry);
        Self {
            store,
            catalog_root,
            entry_cache: RefCell::new(cache),
        }
    }

    fn with_cached_entry<T>(
        &self,
        collection: &str,
        f: impl FnOnce(&CatalogEntry) -> Result<T>,
    ) -> Result<T> {
        if let Some(entry) = self.entry_cache.borrow().get(collection) {
            return f(entry);
        }
        let loaded =
            load_collection_entry(self.store, self.catalog_root, collection)?.ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::NotFound,
                    MODULE_ID,
                    "CoreQuerySource::load_entry",
                    format!("collection '{collection}' does not exist"),
                )
            })?;
        self.entry_cache
            .borrow_mut()
            .insert(collection.to_owned(), loaded);
        let cache = self.entry_cache.borrow();
        let entry = cache
            .get(collection)
            .expect("entry just inserted into cache");
        f(entry)
    }
}

impl<S: PageStore> QuerySource for CoreQuerySource<'_, S> {
    fn collection_indexes(&self, collection: &str) -> Result<Vec<IndexDescriptor>> {
        self.with_cached_entry(collection, |entry| Ok(entry.indexes.clone()))
    }

    fn scan_collection(&self, collection: &str) -> Result<Vec<(RecordId, DocumentBuf)>> {
        let data_root = self.with_cached_entry(collection, |e| Ok(e.data_root_page_id))?;
        let tree = BTree::open(data_root, TreeKind::Collection);
        let pairs = tree.scan_all(self.store)?;
        let mut out = Vec::with_capacity(pairs.len());
        for (raw_key, raw_val) in pairs {
            let rid = decode_record_id(&raw_key)?;
            let doc = decode_collection_record(&raw_val)?;
            out.push((rid, doc));
        }
        Ok(out)
    }

    fn get_by_record_id(
        &self,
        collection: &str,
        record_id: RecordId,
    ) -> Result<Option<DocumentBuf>> {
        let data_root = self.with_cached_entry(collection, |e| Ok(e.data_root_page_id))?;
        let tree = BTree::open(data_root, TreeKind::Collection);
        let key = encode_record_id(record_id);
        match tree.get(self.store, &key)? {
            Some(raw) => Ok(Some(decode_collection_record(&raw)?)),
            None => Ok(None),
        }
    }

    fn probe_primary_id(&self, collection: &str, id_value: &Value) -> Result<Option<RecordId>> {
        let primary_root =
            self.with_cached_entry(collection, |e| Ok(e.primary_index()?.root_page_id))?;
        with_value_view(id_value, |view| {
            lookup_primary_id(self.store, primary_root, view)
        })
    }

    fn scan_secondary_eq(&self, desc: &IndexDescriptor, target: &Value) -> Result<Vec<RecordId>> {
        with_value_view(target, |view| {
            if desc.kind == IndexKind::Compound {
                scan_compound_prefix_eq(self.store, desc, &[*view])
            } else {
                scan_secondary_eq(self.store, desc, view)
            }
        })
    }

    fn scan_secondary_range(
        &self,
        desc: &IndexDescriptor,
        lower: Option<(&Value, bool)>,
        upper: Option<(&Value, bool)>,
    ) -> Result<Vec<RecordId>> {
        match (lower, upper) {
            (Some((lv, linc)), Some((uv, uinc))) => with_value_view(lv, |lview| {
                with_value_view(uv, |uview| {
                    scan_secondary_range(self.store, desc, Some((lview, linc)), Some((uview, uinc)))
                })
            }),
            (Some((lv, linc)), None) => with_value_view(lv, |lview| {
                scan_secondary_range(self.store, desc, Some((lview, linc)), None)
            }),
            (None, Some((uv, uinc))) => with_value_view(uv, |uview| {
                scan_secondary_range(self.store, desc, None, Some((uview, uinc)))
            }),
            (None, None) => scan_secondary_range(self.store, desc, None, None),
        }
    }
}
