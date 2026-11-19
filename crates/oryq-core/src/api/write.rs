//! Exclusive ACID [`WriteTransaction`] and [`WriteCollection`] handles.

use oryq_document::{from_json_str, DocumentBuf, UpdateSpec, Value};
use oryq_engine::{CommitReceipt, WriteTxn};
use oryq_types::{ErrorCategory, ErrorEnvelope, RecordId, Result, TxId};

use crate::api::read::{find_all_in_store, find_by_id_in_store};
use crate::catalog::{list_collection_names, load_collection_entry};
use crate::composition::CoreRegistries;
use crate::write::{
    create_collection_stmt, delete_by_id_stmt, drop_collection_stmt, insert_doc_stmt,
    replace_by_id_stmt, update_by_id_stmt,
};

const MODULE_ID: &str = "core";

/// Exclusive ACID write transaction over an OryqDB [`crate::Database`].
///
/// Dropping without calling [`Self::commit`] automatically rolls back all changes.
#[derive(Debug)]
pub struct WriteTransaction<'db> {
    tx: WriteTxn<'db>,
    registries: &'db CoreRegistries,
    max_document_bytes: usize,
}

impl<'db> WriteTransaction<'db> {
    pub(crate) fn new(
        tx: WriteTxn<'db>,
        registries: &'db CoreRegistries,
        max_document_bytes: usize,
    ) -> Self {
        Self {
            tx,
            registries,
            max_document_bytes,
        }
    }

    #[inline]
    pub fn tx_id(&self) -> TxId {
        self.tx.tx_id()
    }

    pub fn create_collection(&mut self, name: &str) -> Result<()> {
        let _ = create_collection_stmt(&mut self.tx, self.registries, name)?;
        Ok(())
    }

    pub fn drop_collection(&mut self, name: &str) -> Result<bool> {
        drop_collection_stmt(&mut self.tx, self.registries, name)
    }

    pub fn list_collections(&self) -> Result<Vec<String>> {
        list_collection_names(&self.tx, self.tx.catalog_root_page_id())
    }

    pub fn collection(&mut self, name: &str) -> Result<WriteCollection<'_, 'db>> {
        if load_collection_entry(&self.tx, self.tx.catalog_root_page_id(), name)?.is_none() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::NotFound,
                MODULE_ID,
                "WriteTransaction::collection",
                format!("collection '{name}' does not exist"),
            ));
        }
        Ok(WriteCollection {
            tx: &mut self.tx,
            registries: self.registries,
            name: name.to_owned(),
            max_document_bytes: self.max_document_bytes,
        })
    }

    /// Durably commits all mutations in this transaction to `.oryq-wal`.
    pub fn commit(self) -> Result<CommitReceipt> {
        self.tx.commit()
    }

    /// Explicitly discards all mutations in this transaction.
    pub fn rollback(self) {
        self.tx.rollback();
    }
}

/// Exclusive write handle for a single collection inside a [`WriteTransaction`].
#[derive(Debug)]
pub struct WriteCollection<'tx, 'db> {
    tx: &'tx mut WriteTxn<'db>,
    registries: &'db CoreRegistries,
    name: String,
    max_document_bytes: usize,
}

impl WriteCollection<'_, '_> {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn insert(&mut self, doc: &DocumentBuf) -> Result<(Value, RecordId)> {
        insert_doc_stmt(
            self.tx,
            self.registries,
            &self.name,
            doc,
            self.max_document_bytes,
        )
    }

    pub fn insert_json(&mut self, json: &str) -> Result<(Value, RecordId)> {
        let (doc, _) = from_json_str(json, None)?;
        self.insert(&doc)
    }

    pub fn find_by_id(&self, id: &Value) -> Result<Option<DocumentBuf>> {
        let entry = load_collection_entry(self.tx, self.tx.catalog_root_page_id(), &self.name)?
            .ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::NotFound,
                    MODULE_ID,
                    "WriteCollection::find_by_id",
                    format!("collection '{}' does not exist", self.name),
                )
            })?;
        find_by_id_in_store(self.tx, &entry, id)
    }

    pub fn find_all(&self) -> Result<Vec<DocumentBuf>> {
        let entry = load_collection_entry(self.tx, self.tx.catalog_root_page_id(), &self.name)?
            .ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::NotFound,
                    MODULE_ID,
                    "WriteCollection::find_all",
                    format!("collection '{}' does not exist", self.name),
                )
            })?;
        find_all_in_store(self.tx, &entry)
    }

    pub fn update_by_id(&mut self, id: &Value, update_json: &str) -> Result<bool> {
        let spec = UpdateSpec::from_json_str(update_json, &self.registries.updates)?;
        update_by_id_stmt(
            self.tx,
            self.registries,
            &self.name,
            id,
            &spec,
            self.max_document_bytes,
        )
    }

    pub fn replace_by_id(&mut self, id: &Value, replacement_json: &str) -> Result<bool> {
        let (replacement, _) = from_json_str(replacement_json, None)?;
        replace_by_id_stmt(
            self.tx,
            self.registries,
            &self.name,
            id,
            &replacement,
            self.max_document_bytes,
        )
    }

    pub fn delete_by_id(&mut self, id: &Value) -> Result<bool> {
        delete_by_id_stmt(self.tx, self.registries, &self.name, id)
    }
}
