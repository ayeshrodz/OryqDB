//! Exclusive ACID [`WriteTransaction`] and [`WriteCollection`] handles.

use oryq_document::{
    encode_document, from_json_str, parse_json_value_str, DocumentBuf, UpdateSpec, Value,
};
use oryq_engine::{CommitReceipt, SavepointId, WriteTxn};
use oryq_index::{IndexDescriptor, SortOrder};
use oryq_query::{execute_find, parse_filter_json, FilterExpr, FindOptions, QueryContext};
use oryq_types::{ErrorCategory, ErrorEnvelope, RecordId, Result, TxId};

use crate::adapters::CoreQuerySource;
use crate::api::read::{find_all_in_store, find_by_id_in_store, DEFAULT_QUERY_MEMORY_LIMIT};
use crate::catalog::{list_collection_names, load_collection_entry, CatalogEntry};
use crate::composition::CoreRegistries;
use crate::write::{
    create_collection_stmt, create_compound_index_stmt, create_secondary_index_stmt,
    delete_by_id_with_entry, drop_collection_stmt, drop_index_stmt, insert_doc_with_entry,
    replace_by_id_with_entry, update_by_id_with_entry,
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
        let entry = load_collection_entry(&self.tx, self.tx.catalog_root_page_id(), name)?
            .ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::NotFound,
                    MODULE_ID,
                    "WriteTransaction::collection",
                    format!("collection '{name}' does not exist"),
                )
            })?;
        Ok(WriteCollection {
            tx: &mut self.tx,
            registries: self.registries,
            entry,
            max_document_bytes: self.max_document_bytes,
        })
    }

    /// Creates a nested statement savepoint inside this write transaction.
    pub fn savepoint(&mut self) -> SavepointId {
        self.tx.savepoint()
    }

    /// Rolls back mutations performed since `savepoint`.
    pub fn rollback_to(&mut self, savepoint: SavepointId) -> Result<()> {
        self.tx.rollback_to(savepoint)
    }

    /// Releases `savepoint`, keeping its mutations in the enclosing transaction.
    pub fn release_savepoint(&mut self, savepoint: SavepointId) -> Result<()> {
        self.tx.release_savepoint(savepoint)
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
    entry: CatalogEntry,
    max_document_bytes: usize,
}

impl WriteCollection<'_, '_> {
    pub fn name(&self) -> &str {
        &self.entry.name
    }

    fn refresh_entry(&mut self) -> Result<()> {
        if let Some(updated) =
            load_collection_entry(self.tx, self.tx.catalog_root_page_id(), &self.entry.name)?
        {
            self.entry = updated;
        }
        Ok(())
    }

    pub fn insert(&mut self, doc: &DocumentBuf) -> Result<(Value, RecordId)> {
        insert_doc_with_entry(
            self.tx,
            self.registries,
            &mut self.entry,
            doc,
            self.max_document_bytes,
        )
    }

    pub fn insert_json(&mut self, json: &str) -> Result<(Value, RecordId)> {
        let (doc, _) = from_json_str(json, None)?;
        self.insert(&doc)
    }

    /// Inserts multiple documents atomically inside this write transaction (rolls back the
    /// entire batch via an internal savepoint if any document fails).
    pub fn insert_many(&mut self, docs: &[DocumentBuf]) -> Result<Vec<(Value, RecordId)>> {
        let sp = self.tx.savepoint();
        let saved_entry = self.entry.clone();
        let mut results = Vec::with_capacity(docs.len());
        for doc in docs {
            match self.insert(doc) {
                Ok(res) => results.push(res),
                Err(err) => {
                    let _ = self.tx.rollback_to(sp);
                    self.entry = saved_entry;
                    return Err(err);
                }
            }
        }
        self.tx.release_savepoint(sp)?;
        Ok(results)
    }

    /// Parses a JSON array of document objects and inserts all of them atomically inside this
    /// write transaction (rolls back the entire batch via an internal savepoint on any error).
    pub fn insert_many_json(&mut self, json_array: &str) -> Result<Vec<(Value, RecordId)>> {
        let parsed = parse_json_value_str(json_array)?;
        let items = match parsed {
            Value::Array(items) => items,
            _ => {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "WriteCollection::insert_many_json",
                    "expected a JSON array of document objects",
                ));
            }
        };
        let mut docs = Vec::with_capacity(items.len());
        for item in items {
            let fields = match item {
                Value::Document(fields) => fields,
                _ => {
                    return Err(ErrorEnvelope::new(
                        ErrorCategory::InvalidArgument,
                        MODULE_ID,
                        "WriteCollection::insert_many_json",
                        "each element in insert_many_json must be a JSON object",
                    ));
                }
            };
            let (doc, _) = encode_document(&fields, None)?;
            docs.push(doc);
        }
        self.insert_many(&docs)
    }

    fn find_matching_ids(&self, filter: &FilterExpr, limit: Option<u64>) -> Result<Vec<Value>> {
        let source = CoreQuerySource::with_entry(
            self.tx,
            self.tx.catalog_root_page_id(),
            self.entry.clone(),
        );
        let opts = FindOptions {
            filter: Some(filter.clone()),
            limit,
            ..Default::default()
        };
        let ctx = QueryContext::default_with_limit(DEFAULT_QUERY_MEMORY_LIMIT);
        let cursor = execute_find(&source, &self.entry.name, &opts, &ctx)?;
        let mut ids = Vec::new();
        for doc in cursor.collect_all() {
            if let Some(id_view) = doc.view().get_field("_id") {
                ids.push(id_view.to_owned_value());
            }
        }
        Ok(ids)
    }

    pub fn find_by_id(&self, id: &Value) -> Result<Option<DocumentBuf>> {
        find_by_id_in_store(self.tx, &self.entry, id)
    }

    pub fn find_all(&self) -> Result<Vec<DocumentBuf>> {
        find_all_in_store(self.tx, &self.entry)
    }

    pub fn update_by_id(&mut self, id: &Value, update_json: &str) -> Result<bool> {
        let spec = UpdateSpec::from_json_str(update_json, &self.registries.updates)?;
        update_by_id_with_entry(
            self.tx,
            self.registries,
            &mut self.entry,
            id,
            &spec,
            self.max_document_bytes,
        )
    }

    /// Updates the first document matching `filter` using `update_json`.
    pub fn update_one(&mut self, filter: &FilterExpr, update_json: &str) -> Result<bool> {
        let spec = UpdateSpec::from_json_str(update_json, &self.registries.updates)?;
        let ids = self.find_matching_ids(filter, Some(1))?;
        let Some(id) = ids.first() else {
            return Ok(false);
        };
        update_by_id_with_entry(
            self.tx,
            self.registries,
            &mut self.entry,
            id,
            &spec,
            self.max_document_bytes,
        )
    }

    /// JSON convenience wrapper for [`Self::update_one`].
    pub fn update_one_json(&mut self, filter_json: &str, update_json: &str) -> Result<bool> {
        let filter = parse_filter_json(filter_json)?;
        self.update_one(&filter, update_json)
    }

    /// Updates all documents matching `filter` atomically using `update_json`, returning the
    /// number updated (rolls back the entire batch via an internal savepoint on any error).
    pub fn update_many(&mut self, filter: &FilterExpr, update_json: &str) -> Result<usize> {
        let spec = UpdateSpec::from_json_str(update_json, &self.registries.updates)?;
        let ids = self.find_matching_ids(filter, None)?;
        let sp = self.tx.savepoint();
        let saved_entry = self.entry.clone();
        let mut updated = 0usize;
        for id in &ids {
            match update_by_id_with_entry(
                self.tx,
                self.registries,
                &mut self.entry,
                id,
                &spec,
                self.max_document_bytes,
            ) {
                Ok(true) => updated += 1,
                Ok(false) => {}
                Err(err) => {
                    let _ = self.tx.rollback_to(sp);
                    self.entry = saved_entry;
                    return Err(err);
                }
            }
        }
        self.tx.release_savepoint(sp)?;
        Ok(updated)
    }

    /// JSON convenience wrapper for [`Self::update_many`].
    pub fn update_many_json(&mut self, filter_json: &str, update_json: &str) -> Result<usize> {
        let filter = parse_filter_json(filter_json)?;
        self.update_many(&filter, update_json)
    }

    pub fn replace_by_id(&mut self, id: &Value, replacement_json: &str) -> Result<bool> {
        let (replacement, _) = from_json_str(replacement_json, None)?;
        replace_by_id_with_entry(
            self.tx,
            self.registries,
            &mut self.entry,
            id,
            &replacement,
            self.max_document_bytes,
        )
    }

    /// Inserts `doc` if no document with its `_id` exists, or replaces the existing document.
    ///
    /// Returns `Ok(true)` if a new document was inserted, or `Ok(false)` if an existing
    /// document was replaced.
    pub fn upsert(&mut self, doc: &DocumentBuf) -> Result<bool> {
        if let Some(id_view) = doc.view().get_field("_id") {
            let id = id_view.to_owned_value();
            if replace_by_id_with_entry(
                self.tx,
                self.registries,
                &mut self.entry,
                &id,
                doc,
                self.max_document_bytes,
            )? {
                return Ok(false);
            }
        }
        let _ = self.insert(doc)?;
        Ok(true)
    }

    /// JSON convenience wrapper for [`Self::upsert`].
    pub fn upsert_json(&mut self, doc_json: &str) -> Result<bool> {
        let (doc, _) = from_json_str(doc_json, None)?;
        self.upsert(&doc)
    }

    pub fn delete_by_id(&mut self, id: &Value) -> Result<bool> {
        delete_by_id_with_entry(self.tx, self.registries, &mut self.entry, id)
    }

    /// Deletes the first document matching `filter`.
    pub fn delete_one(&mut self, filter: &FilterExpr) -> Result<bool> {
        let ids = self.find_matching_ids(filter, Some(1))?;
        let Some(id) = ids.first() else {
            return Ok(false);
        };
        delete_by_id_with_entry(self.tx, self.registries, &mut self.entry, id)
    }

    /// JSON convenience wrapper for [`Self::delete_one`].
    pub fn delete_one_json(&mut self, filter_json: &str) -> Result<bool> {
        let filter = parse_filter_json(filter_json)?;
        self.delete_one(&filter)
    }

    /// Deletes all documents matching `filter` atomically, returning the number deleted
    /// (rolls back the entire batch via an internal savepoint on any error).
    pub fn delete_many(&mut self, filter: &FilterExpr) -> Result<usize> {
        let ids = self.find_matching_ids(filter, None)?;
        let sp = self.tx.savepoint();
        let saved_entry = self.entry.clone();
        let mut deleted = 0usize;
        for id in &ids {
            match delete_by_id_with_entry(self.tx, self.registries, &mut self.entry, id) {
                Ok(true) => deleted += 1,
                Ok(false) => {}
                Err(err) => {
                    let _ = self.tx.rollback_to(sp);
                    self.entry = saved_entry;
                    return Err(err);
                }
            }
        }
        self.tx.release_savepoint(sp)?;
        Ok(deleted)
    }

    /// JSON convenience wrapper for [`Self::delete_many`].
    pub fn delete_many_json(&mut self, filter_json: &str) -> Result<usize> {
        let filter = parse_filter_json(filter_json)?;
        self.delete_many(&filter)
    }

    pub fn create_index(
        &mut self,
        path: &str,
        order: SortOrder,
        unique: bool,
        sparse: bool,
        name: Option<&str>,
    ) -> Result<IndexDescriptor> {
        let desc = create_secondary_index_stmt(
            self.tx,
            self.registries,
            &self.entry.name,
            name,
            path,
            order,
            unique,
            sparse,
        )?;
        self.refresh_entry()?;
        Ok(desc)
    }

    pub fn create_compound_index(
        &mut self,
        fields: &[(&str, SortOrder)],
        unique: bool,
        sparse: bool,
        name: Option<&str>,
    ) -> Result<IndexDescriptor> {
        let desc = create_compound_index_stmt(
            self.tx,
            self.registries,
            &self.entry.name,
            name,
            fields,
            unique,
            sparse,
        )?;
        self.refresh_entry()?;
        Ok(desc)
    }

    #[cfg(feature = "fts")]
    pub fn create_fts_index(
        &mut self,
        fields: &[&str],
        name: Option<&str>,
    ) -> Result<IndexDescriptor> {
        let desc = crate::write::create_fts_index_stmt(
            self.tx,
            self.registries,
            &self.entry.name,
            name,
            fields,
        )?;
        self.refresh_entry()?;
        Ok(desc)
    }

    #[cfg(feature = "vec")]
    pub fn create_vector_index(
        &mut self,
        field: &str,
        options: oryq_vec::VectorIndexOptions,
        name: Option<&str>,
    ) -> Result<IndexDescriptor> {
        let desc = crate::write::create_vector_index_stmt(
            self.tx,
            self.registries,
            &self.entry.name,
            name,
            field,
            options,
        )?;
        self.refresh_entry()?;
        Ok(desc)
    }

    pub fn drop_index(&mut self, index_name: &str) -> Result<bool> {
        let dropped = drop_index_stmt(self.tx, self.registries, &self.entry.name, index_name)?;
        if dropped {
            self.refresh_entry()?;
        }
        Ok(dropped)
    }

    pub fn list_indexes(&self) -> Result<Vec<IndexDescriptor>> {
        Ok(self.entry.indexes.clone())
    }
}
