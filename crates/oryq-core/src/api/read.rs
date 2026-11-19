//! Snapshot-isolated [`ReadTransaction`] and [`ReadCollection`] handles.

use oryq_document::{encode_document, DocumentBuf, Value};
use oryq_engine::{BTree, PageStore, ReadTxn, TreeKind};
use oryq_index::{encode_record_id, lookup_primary_id, IndexDescriptor};
use oryq_types::{ErrorCategory, ErrorEnvelope, Result, SnapshotId};

use crate::catalog::{list_collection_names, load_collection_entry, CatalogEntry};
use crate::write::decode_collection_record;

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
