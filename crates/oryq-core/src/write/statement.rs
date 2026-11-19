//! Statement-scoped atomic CRUD and DDL orchestration (`docs/modules/core.md` §4 & §5).
//!
//! Every statement runs inside a [`oryq_engine::SavepointId`]: if any index maintenance
//! or catalog update fails, the entire statement rolls back cleanly without affecting
//! earlier statements in the same [`WriteTxn`].

use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_document::{
    apply_update, encode_document, validate_replacement_doc, DocumentBuf, ObjectId, UpdateSpec,
    Value,
};
use oryq_engine::btree::read_internal_node;
use oryq_engine::{BTree, PageStore, PageStoreMut, SlottedPageView, TreeKind, WriteTxn};
use oryq_index::{encode_record_id, lookup_primary_id, IndexDescriptor, PRIMARY_PROVIDER_ID};
use oryq_types::{
    CapabilityId, CollectionId, ErrorCategory, ErrorEnvelope, FeatureBitmask, IndexId, PageId,
    RecordId, Result,
};

use crate::catalog::{
    delete_collection_entry, load_catalog_meta, load_collection_entry, save_catalog_meta,
    save_collection_entry, validate_collection_name, CatalogEntry,
};
use crate::composition::CoreRegistries;

const MODULE_ID: &str = "core";
const RECORD_KIND_INLINE: u8 = 0x00;

/// Generates a 12-byte monotonically sortable [`ObjectId`] (`docs/document-model.md` §4.2):
/// `[4-byte unix_seconds_be][5-byte process_tag][3-byte atomic_counter_be]`.
pub fn generate_object_id() -> ObjectId {
    static COUNTER: AtomicU32 = AtomicU32::new(1);
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);
    let pid = std::process::id();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed) & 0x00FF_FFFF;

    let mut raw = [0u8; 12];
    raw[0..4].copy_from_slice(&secs.to_be_bytes());
    raw[4..8].copy_from_slice(&pid.to_be_bytes());
    raw[8] = 0xA5;
    raw[9] = ((seq >> 16) & 0xFF) as u8;
    raw[10] = ((seq >> 8) & 0xFF) as u8;
    raw[11] = (seq & 0xFF) as u8;
    ObjectId::from_bytes(raw)
}

/// Encodes an inline collection B+ tree value `[storage_kind=0x00][oryqdoc_bytes]`.
pub fn encode_collection_record(doc: &DocumentBuf) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + doc.len());
    out.push(RECORD_KIND_INLINE);
    out.extend_from_slice(doc.as_bytes());
    out
}

/// Decodes a collection B+ tree value into an immutable validated [`DocumentBuf`].
pub fn decode_collection_record(raw: &[u8]) -> Result<DocumentBuf> {
    let (&kind, body) = raw.split_first().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::Corruption,
            MODULE_ID,
            "write::decode_collection_record",
            "empty collection record envelope",
        )
    })?;
    if kind != RECORD_KIND_INLINE {
        return Err(ErrorEnvelope::new(
            ErrorCategory::UnsupportedFeature,
            MODULE_ID,
            "write::decode_collection_record",
            format!("unsupported collection record storage_kind 0x{kind:02x}"),
        ));
    }
    DocumentBuf::from_bytes(body)
}

/// Creates a new empty collection with its collection B+ tree and primary `_id` index.
pub fn create_collection_stmt(
    tx: &mut WriteTxn<'_>,
    registries: &CoreRegistries,
    name: &str,
) -> Result<CatalogEntry> {
    validate_collection_name(name)?;
    let sp = tx.savepoint();
    let res = (|| -> Result<CatalogEntry> {
        let catalog_root = tx.catalog_root_page_id();
        if load_collection_entry(tx, catalog_root, name)?.is_some() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::DuplicateKey,
                MODULE_ID,
                "write::create_collection_stmt",
                format!("collection '{name}' already exists"),
            ));
        }

        let mut meta = load_catalog_meta(tx, catalog_root)?;
        let collection_id = CollectionId::new(meta.next_collection_id)?;
        let primary_index_id = IndexId::new(meta.next_index_id)?;
        meta.next_collection_id = meta.next_collection_id.checked_add(1).ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "write::create_collection_stmt",
                "collection_id counter overflow",
            )
        })?;
        meta.next_index_id = meta.next_index_id.checked_add(1).ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "write::create_collection_stmt",
                "index_id counter overflow",
            )
        })?;

        let data_tree = BTree::create_empty(tx, TreeKind::Collection)?;
        let primary_cap = CapabilityId::new(PRIMARY_PROVIDER_ID)?;
        let primary_provider = registries.indexes.require(&primary_cap)?;
        let mut primary_desc = IndexDescriptor::primary(primary_index_id, PageId::new(1));
        primary_desc.root_page_id = primary_provider.create_empty(tx, &primary_desc)?;

        let entry = CatalogEntry {
            collection_id,
            name: name.to_owned(),
            data_root_page_id: data_tree.root(),
            next_record_id: RecordId::new(1)?,
            required_write_features: FeatureBitmask::EMPTY,
            indexes: vec![primary_desc],
        };

        let root_after_meta = save_catalog_meta(tx, catalog_root, &meta)?;
        let root_after_entry = save_collection_entry(tx, root_after_meta, &entry)?;
        tx.set_catalog_root_page_id(root_after_entry);
        Ok(entry)
    })();

    match res {
        Ok(entry) => {
            tx.release_savepoint(sp)?;
            Ok(entry)
        }
        Err(err) => {
            let _ = tx.rollback_to(sp);
            Err(err)
        }
    }
}

/// Drops a collection, freeing its data B+ tree and all registered index trees atomically.
pub fn drop_collection_stmt(
    tx: &mut WriteTxn<'_>,
    registries: &CoreRegistries,
    name: &str,
) -> Result<bool> {
    validate_collection_name(name)?;
    let sp = tx.savepoint();
    let res = (|| -> Result<bool> {
        let catalog_root = tx.catalog_root_page_id();
        let (new_root, removed) = delete_collection_entry(tx, catalog_root, name)?;
        let Some(entry) = removed else {
            return Ok(false);
        };
        for desc in &entry.indexes {
            let provider = registries.indexes.require(&desc.provider_id)?;
            provider.drop_index(tx, desc)?;
        }
        free_btree_pages(tx, entry.data_root_page_id)?;
        tx.set_catalog_root_page_id(new_root);
        Ok(true)
    })();

    match res {
        Ok(dropped) => {
            tx.release_savepoint(sp)?;
            Ok(dropped)
        }
        Err(err) => {
            let _ = tx.rollback_to(sp);
            Err(err)
        }
    }
}

/// Atomically inserts `doc` (auto-generating an `ObjectId` `_id` if omitted) into `collection_name`.
pub fn insert_doc_stmt(
    tx: &mut WriteTxn<'_>,
    registries: &CoreRegistries,
    collection_name: &str,
    doc: &DocumentBuf,
    max_document_bytes: usize,
) -> Result<(Value, RecordId)> {
    let doc_with_id = ensure_document_id(doc)?;
    if doc_with_id.len() > max_document_bytes {
        return Err(ErrorEnvelope::new(
            ErrorCategory::Overflow,
            MODULE_ID,
            "write::insert_doc_stmt",
            format!(
                "document size {} exceeds max_document_bytes {max_document_bytes}",
                doc_with_id.len()
            ),
        ));
    }
    let id_value = doc_with_id
        .view()
        .get_field("_id")
        .expect("_id guaranteed present")
        .to_owned_value();

    let sp = tx.savepoint();
    let res = (|| -> Result<(Value, RecordId)> {
        let catalog_root = tx.catalog_root_page_id();
        let mut entry =
            load_collection_entry(tx, catalog_root, collection_name)?.ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::NotFound,
                    MODULE_ID,
                    "write::insert_doc_stmt",
                    format!("collection '{collection_name}' does not exist"),
                )
            })?;

        let record_id = entry.next_record_id;
        entry.next_record_id = record_id.checked_next()?;

        let mut data_tree = BTree::open(entry.data_root_page_id, TreeKind::Collection);
        let rec_key = encode_record_id(record_id);
        let rec_val = encode_collection_record(&doc_with_id);
        data_tree.insert(tx, &rec_key, &rec_val)?;
        entry.data_root_page_id = data_tree.root();

        let view = doc_with_id.view();
        for desc in &mut entry.indexes {
            let provider = registries.indexes.require(&desc.provider_id)?;
            let maint = provider.on_insert(tx, desc, record_id, &view)?;
            desc.root_page_id = maint.new_root_page_id;
        }

        let new_catalog_root = save_collection_entry(tx, catalog_root, &entry)?;
        tx.set_catalog_root_page_id(new_catalog_root);
        Ok((id_value, record_id))
    })();

    match res {
        Ok(ok) => {
            tx.release_savepoint(sp)?;
            Ok(ok)
        }
        Err(err) => {
            let _ = tx.rollback_to(sp);
            Err(err)
        }
    }
}

/// Atomically applies an [`UpdateSpec`] to the document matching `id_value` in `collection_name`.
pub fn update_by_id_stmt(
    tx: &mut WriteTxn<'_>,
    registries: &CoreRegistries,
    collection_name: &str,
    id_value: &Value,
    update_spec: &UpdateSpec,
    max_document_bytes: usize,
) -> Result<bool> {
    let sp = tx.savepoint();
    let res = (|| -> Result<bool> {
        let catalog_root = tx.catalog_root_page_id();
        let mut entry =
            load_collection_entry(tx, catalog_root, collection_name)?.ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::NotFound,
                    MODULE_ID,
                    "write::update_by_id_stmt",
                    format!("collection '{collection_name}' does not exist"),
                )
            })?;

        let primary_root = entry.primary_index()?.root_page_id;
        let (id_doc, _) = encode_document(&[("_id".into(), id_value.clone())], None)?;
        let id_view = id_doc.view().get_field("_id").unwrap();
        let Some(record_id) = lookup_primary_id(tx, primary_root, &id_view)? else {
            return Ok(false);
        };

        let mut data_tree = BTree::open(entry.data_root_page_id, TreeKind::Collection);
        let rec_key = encode_record_id(record_id);
        let raw = data_tree.get(tx, &rec_key)?.ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "write::update_by_id_stmt",
                format!(
                    "primary index points to missing RecordId({}) in collection '{collection_name}'",
                    record_id.raw()
                ),
            )
        })?;
        let existing_doc = decode_collection_record(&raw)?;
        let (mutated, _) =
            apply_update(&existing_doc.view(), update_spec, &registries.updates, None)?;

        if mutated.updated_doc.len() > max_document_bytes {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "write::update_by_id_stmt",
                format!(
                    "updated document size {} exceeds max_document_bytes {max_document_bytes}",
                    mutated.updated_doc.len()
                ),
            ));
        }

        if !mutated.modified {
            return Ok(true);
        }

        let new_rec_val = encode_collection_record(&mutated.updated_doc);
        data_tree.insert(tx, &rec_key, &new_rec_val)?;
        entry.data_root_page_id = data_tree.root();

        let before_view = existing_doc.view();
        let after_view = mutated.updated_doc.view();
        for desc in &mut entry.indexes {
            let provider = registries.indexes.require(&desc.provider_id)?;
            let maint = provider.on_update(tx, desc, record_id, &before_view, &after_view)?;
            desc.root_page_id = maint.new_root_page_id;
        }

        let new_catalog_root = save_collection_entry(tx, catalog_root, &entry)?;
        tx.set_catalog_root_page_id(new_catalog_root);
        Ok(true)
    })();

    match res {
        Ok(ok) => {
            tx.release_savepoint(sp)?;
            Ok(ok)
        }
        Err(err) => {
            let _ = tx.rollback_to(sp);
            Err(err)
        }
    }
}

/// Atomically replaces the document matching `id_value` after verifying `_id` immutability.
pub fn replace_by_id_stmt(
    tx: &mut WriteTxn<'_>,
    registries: &CoreRegistries,
    collection_name: &str,
    id_value: &Value,
    replacement: &DocumentBuf,
    max_document_bytes: usize,
) -> Result<bool> {
    if replacement.len() > max_document_bytes {
        return Err(ErrorEnvelope::new(
            ErrorCategory::Overflow,
            MODULE_ID,
            "write::replace_by_id_stmt",
            format!(
                "replacement document size {} exceeds max_document_bytes {max_document_bytes}",
                replacement.len()
            ),
        ));
    }

    let sp = tx.savepoint();
    let res = (|| -> Result<bool> {
        let catalog_root = tx.catalog_root_page_id();
        let mut entry =
            load_collection_entry(tx, catalog_root, collection_name)?.ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::NotFound,
                    MODULE_ID,
                    "write::replace_by_id_stmt",
                    format!("collection '{collection_name}' does not exist"),
                )
            })?;

        let primary_root = entry.primary_index()?.root_page_id;
        let (id_doc, _) = encode_document(&[("_id".into(), id_value.clone())], None)?;
        let id_view = id_doc.view().get_field("_id").unwrap();
        let Some(record_id) = lookup_primary_id(tx, primary_root, &id_view)? else {
            return Ok(false);
        };

        let mut data_tree = BTree::open(entry.data_root_page_id, TreeKind::Collection);
        let rec_key = encode_record_id(record_id);
        let raw = data_tree.get(tx, &rec_key)?.ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "write::replace_by_id_stmt",
                "primary index points to missing collection record",
            )
        })?;
        let existing_doc = decode_collection_record(&raw)?;
        validate_replacement_doc(&existing_doc.view(), &replacement.view())?;

        let new_rec_val = encode_collection_record(replacement);
        data_tree.insert(tx, &rec_key, &new_rec_val)?;
        entry.data_root_page_id = data_tree.root();

        let before_view = existing_doc.view();
        let after_view = replacement.view();
        for desc in &mut entry.indexes {
            let provider = registries.indexes.require(&desc.provider_id)?;
            let maint = provider.on_update(tx, desc, record_id, &before_view, &after_view)?;
            desc.root_page_id = maint.new_root_page_id;
        }

        let new_catalog_root = save_collection_entry(tx, catalog_root, &entry)?;
        tx.set_catalog_root_page_id(new_catalog_root);
        Ok(true)
    })();

    match res {
        Ok(ok) => {
            tx.release_savepoint(sp)?;
            Ok(ok)
        }
        Err(err) => {
            let _ = tx.rollback_to(sp);
            Err(err)
        }
    }
}

/// Atomically deletes the document matching `id_value` from collection and all indexes.
pub fn delete_by_id_stmt(
    tx: &mut WriteTxn<'_>,
    registries: &CoreRegistries,
    collection_name: &str,
    id_value: &Value,
) -> Result<bool> {
    let sp = tx.savepoint();
    let res = (|| -> Result<bool> {
        let catalog_root = tx.catalog_root_page_id();
        let mut entry =
            load_collection_entry(tx, catalog_root, collection_name)?.ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::NotFound,
                    MODULE_ID,
                    "write::delete_by_id_stmt",
                    format!("collection '{collection_name}' does not exist"),
                )
            })?;

        let primary_root = entry.primary_index()?.root_page_id;
        let (id_doc, _) = encode_document(&[("_id".into(), id_value.clone())], None)?;
        let id_view = id_doc.view().get_field("_id").unwrap();
        let Some(record_id) = lookup_primary_id(tx, primary_root, &id_view)? else {
            return Ok(false);
        };

        let mut data_tree = BTree::open(entry.data_root_page_id, TreeKind::Collection);
        let rec_key = encode_record_id(record_id);
        let Some(raw) = data_tree.delete(tx, &rec_key)? else {
            return Ok(false);
        };
        entry.data_root_page_id = data_tree.root();

        let existing_doc = decode_collection_record(&raw)?;
        let view = existing_doc.view();
        for desc in &mut entry.indexes {
            let provider = registries.indexes.require(&desc.provider_id)?;
            let maint = provider.on_delete(tx, desc, record_id, &view)?;
            desc.root_page_id = maint.new_root_page_id;
        }

        let new_catalog_root = save_collection_entry(tx, catalog_root, &entry)?;
        tx.set_catalog_root_page_id(new_catalog_root);
        Ok(true)
    })();

    match res {
        Ok(ok) => {
            tx.release_savepoint(sp)?;
            Ok(ok)
        }
        Err(err) => {
            let _ = tx.rollback_to(sp);
            Err(err)
        }
    }
}

fn ensure_document_id(doc: &DocumentBuf) -> Result<DocumentBuf> {
    let view = doc.view();
    if view.get_field("_id").is_some() {
        return Ok(doc.clone());
    }
    let mut fields: Vec<(String, Value)> = Vec::with_capacity(view.field_count() + 1);
    fields.push(("_id".to_owned(), Value::ObjectId(generate_object_id())));
    for (k, v) in view.iter() {
        fields.push((k.to_owned(), v.to_owned_value()));
    }
    let (encoded, _) = encode_document(&fields, None)?;
    Ok(encoded)
}

fn free_btree_pages(tx: &mut WriteTxn<'_>, root: PageId) -> Result<()> {
    let mut stack = vec![root];
    let mut visited = HashSet::new();
    while let Some(pid) = stack.pop() {
        if !visited.insert(pid) {
            continue;
        }
        let bytes = tx.read_page(pid)?;
        let view = SlottedPageView::parse(&bytes, pid)?;
        if TreeKind::is_internal(view.header().page_type) {
            let node = read_internal_node(&bytes, pid)?;
            stack.push(node.leftmost_child);
            for (_, child) in node.separators {
                stack.push(child);
            }
        }
        tx.free_page(pid)?;
    }
    Ok(())
}
