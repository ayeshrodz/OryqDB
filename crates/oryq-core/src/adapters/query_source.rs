//! [`CoreQuerySource`] adapter implementing [`oryq_query::QuerySource`] over a snapshot-pinned
//! [`PageStore`] and catalog root page.

use std::fmt;

use oryq_document::{encode_document, DocumentBuf, Value};
use oryq_engine::{BTree, PageStore, TreeKind};
use oryq_index::{
    decode_record_id, encode_record_id, lookup_primary_id, scan_compound_prefix_eq,
    scan_secondary_eq, scan_secondary_range, IndexDescriptor, IndexKind,
};
use oryq_query::QuerySource;
use oryq_types::{ErrorCategory, ErrorEnvelope, PageId, RecordId, Result};

use crate::catalog::{load_collection_entry, CatalogEntry};
use crate::write::decode_collection_record;

const MODULE_ID: &str = "core";

/// Snapshot-pinned [`QuerySource`] backed by any [`PageStore`] (e.g. `ReadTxn` or `WriteTxn`).
pub struct CoreQuerySource<'a, S: PageStore> {
    store: &'a S,
    catalog_root: PageId,
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
        }
    }

    fn load_entry(&self, collection: &str) -> Result<CatalogEntry> {
        load_collection_entry(self.store, self.catalog_root, collection)?.ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::NotFound,
                MODULE_ID,
                "CoreQuerySource::load_entry",
                format!("collection '{collection}' does not exist"),
            )
        })
    }
}

impl<S: PageStore> QuerySource for CoreQuerySource<'_, S> {
    fn collection_indexes(&self, collection: &str) -> Result<Vec<IndexDescriptor>> {
        Ok(self.load_entry(collection)?.indexes)
    }

    fn scan_collection(&self, collection: &str) -> Result<Vec<(RecordId, DocumentBuf)>> {
        let entry = self.load_entry(collection)?;
        let tree = BTree::open(entry.data_root_page_id, TreeKind::Collection);
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
        let entry = self.load_entry(collection)?;
        let tree = BTree::open(entry.data_root_page_id, TreeKind::Collection);
        let key = encode_record_id(record_id);
        match tree.get(self.store, &key)? {
            Some(raw) => Ok(Some(decode_collection_record(&raw)?)),
            None => Ok(None),
        }
    }

    fn probe_primary_id(&self, collection: &str, id_value: &Value) -> Result<Option<RecordId>> {
        let entry = self.load_entry(collection)?;
        let primary_root = entry.primary_index()?.root_page_id;
        let (doc, _) = encode_document(&[("_id".to_owned(), id_value.clone())], None)?;
        let view = doc.view().get_field("_id").unwrap();
        lookup_primary_id(self.store, primary_root, &view)
    }

    fn scan_secondary_eq(&self, desc: &IndexDescriptor, target: &Value) -> Result<Vec<RecordId>> {
        let (doc, _) = encode_document(&[("v".to_owned(), target.clone())], None)?;
        let view = doc.view().get_field("v").unwrap();
        if desc.kind == IndexKind::Compound {
            scan_compound_prefix_eq(self.store, desc, &[view])
        } else {
            scan_secondary_eq(self.store, desc, &view)
        }
    }

    fn scan_secondary_range(
        &self,
        desc: &IndexDescriptor,
        lower: Option<(&Value, bool)>,
        upper: Option<(&Value, bool)>,
    ) -> Result<Vec<RecordId>> {
        let lo_doc = match lower {
            Some((v, inc)) => {
                let (d, _) = encode_document(&[("v".to_owned(), v.clone())], None)?;
                Some((d, inc))
            }
            None => None,
        };
        let hi_doc = match upper {
            Some((v, inc)) => {
                let (d, _) = encode_document(&[("v".to_owned(), v.clone())], None)?;
                Some((d, inc))
            }
            None => None,
        };
        let lo_view = lo_doc
            .as_ref()
            .map(|(d, inc)| (d.view().get_field("v").unwrap(), *inc));
        let hi_view = hi_doc
            .as_ref()
            .map(|(d, inc)| (d.view().get_field("v").unwrap(), *inc));
        let lo_arg = lo_view.as_ref().map(|(v, inc)| (v, *inc));
        let hi_arg = hi_view.as_ref().map(|(v, inc)| (v, *inc));
        scan_secondary_range(self.store, desc, lo_arg, hi_arg)
    }
}
