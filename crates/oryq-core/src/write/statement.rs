//! Statement-scoped atomic CRUD and DDL orchestration (`docs/modules/core.md` §4 & §5).
//!
//! Every statement runs inside a [`oryq_engine::SavepointId`]: if any index maintenance
//! or catalog update fails, the entire statement rolls back cleanly without affecting
//! earlier statements in the same [`WriteTxn`].

use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use oryq_document::{
    apply_update, encode_document, validate_replacement_doc, DocumentBuf, FieldPath, ObjectId,
    UpdateSpec, Value,
};
use oryq_engine::btree::read_internal_node;
use oryq_engine::{
    BTree, PageStore, PageStoreMut, RecordCompactor, SlottedPageView, TreeKind, WriteTxn,
};
use oryq_index::{
    decode_record_id, encode_record_id, lookup_primary_id, IndexDescriptor, IndexFieldSpec,
    IndexKind, SortOrder, COMPOUND_PROVIDER_ID, PRIMARY_PROVIDER_ID,
};
use oryq_types::{
    BudgetDomain, BudgetHandle, CapabilityId, CollectionId, ErrorCategory, ErrorEnvelope,
    FeatureBitmask, IndexId, PageId, RecordId, Result,
};

use crate::adapters::DocumentRecordCompactor;
use crate::catalog::{
    delete_collection_entry, load_catalog_meta, load_collection_entry, save_catalog_meta,
    save_collection_entry, validate_collection_name, CatalogEntry,
};
use crate::composition::CoreRegistries;

const MODULE_ID: &str = "core";
const RECORD_KIND_INLINE: u8 = 0x00;
const RECORD_KIND_DELTA_CHAIN: u8 = 0x01;
/// Maximum number of chained physical delta patches before automatic compaction (`docs/file-format.md` §4.2).
pub const MAX_DELTA_CHAIN: u8 = 4;

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

/// Decodes a collection B+ tree value (inline `0x00` or delta-chained `0x01`) into an
/// immutable validated [`DocumentBuf`].
pub fn decode_collection_record(raw: &[u8]) -> Result<DocumentBuf> {
    let (&kind, body) = raw.split_first().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::Corruption,
            MODULE_ID,
            "write::decode_collection_record",
            "empty collection record envelope",
        )
    })?;
    match kind {
        RECORD_KIND_INLINE => DocumentBuf::from_bytes(body),
        RECORD_KIND_DELTA_CHAIN => {
            let (base, patches, _bloom) = parse_delta_chain_record(body)?;
            let patch_slices: Vec<&[u8]> = patches.iter().map(|p| p.as_slice()).collect();
            let compactor = DocumentRecordCompactor;
            let budget = BudgetHandle::new(BudgetDomain::Transaction, 16 * 1024 * 1024);
            let compacted = compactor.compact(&base, &patch_slices, &budget)?;
            DocumentBuf::from_bytes(compacted)
        }
        other => Err(ErrorEnvelope::new(
            ErrorCategory::UnsupportedFeature,
            MODULE_ID,
            "write::decode_collection_record",
            format!("unsupported collection record storage_kind 0x{other:02x}"),
        )),
    }
}

/// Encodes a delta-chained record envelope `[0x01][count: u8][bloom: u64 LE][base_len: u32 LE][base][patch_i_len: u32 LE][patch_i]...`.
fn encode_delta_chain_record(base: &[u8], patches: &[Vec<u8>], bloom_mask: u64) -> Vec<u8> {
    let total_patches: usize = patches.iter().map(|p| 4 + p.len()).sum();
    let mut out = Vec::with_capacity(1 + 1 + 8 + 4 + base.len() + total_patches);
    out.push(RECORD_KIND_DELTA_CHAIN);
    out.push(patches.len() as u8);
    out.extend_from_slice(&bloom_mask.to_le_bytes());
    out.extend_from_slice(&(base.len() as u32).to_le_bytes());
    out.extend_from_slice(base);
    for p in patches {
        out.extend_from_slice(&(p.len() as u32).to_le_bytes());
        out.extend_from_slice(p);
    }
    out
}

fn parse_delta_chain_record(body: &[u8]) -> Result<(Vec<u8>, Vec<Vec<u8>>, u64)> {
    if body.len() < 1 + 8 + 4 {
        return Err(ErrorEnvelope::new(
            ErrorCategory::Corruption,
            MODULE_ID,
            "write::parse_delta_chain_record",
            "truncated delta-chain record header",
        ));
    }
    let count = body[0] as usize;
    if count == 0 || count > MAX_DELTA_CHAIN as usize {
        return Err(ErrorEnvelope::new(
            ErrorCategory::Corruption,
            MODULE_ID,
            "write::parse_delta_chain_record",
            format!("invalid delta chain length {count}"),
        ));
    }
    let bloom = u64::from_le_bytes(body[1..9].try_into().unwrap());
    let base_len = u32::from_le_bytes(body[9..13].try_into().unwrap()) as usize;
    let mut pos = 13;
    if pos + base_len > body.len() {
        return Err(ErrorEnvelope::new(
            ErrorCategory::Corruption,
            MODULE_ID,
            "write::parse_delta_chain_record",
            "truncated base document in delta-chain record",
        ));
    }
    let base = body[pos..pos + base_len].to_vec();
    pos += base_len;

    let mut patches = Vec::with_capacity(count);
    for _ in 0..count {
        if pos + 4 > body.len() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "write::parse_delta_chain_record",
                "truncated patch length in delta-chain record",
            ));
        }
        let plen = u32::from_le_bytes(body[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        if pos + plen > body.len() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "write::parse_delta_chain_record",
                "truncated patch payload in delta-chain record",
            ));
        }
        patches.push(body[pos..pos + plen].to_vec());
        pos += plen;
    }
    Ok((base, patches, bloom))
}

/// Appends a state-normalized physical patch to `raw_existing` if the chain stays within
/// `MAX_DELTA_CHAIN` (4), or compacts automatically back to `RECORD_KIND_INLINE` on the 5th patch.
fn append_or_compact_record(
    raw_existing: &[u8],
    patch_bytes: &[u8],
    patch_bloom: u64,
    fallback_compacted: &DocumentBuf,
) -> Result<Vec<u8>> {
    if patch_bytes.is_empty() {
        return Ok(encode_collection_record(fallback_compacted));
    }
    let (&kind, body) = raw_existing.split_first().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::Corruption,
            MODULE_ID,
            "write::append_or_compact_record",
            "empty existing collection record",
        )
    })?;

    match kind {
        RECORD_KIND_INLINE => Ok(encode_delta_chain_record(
            body,
            &[patch_bytes.to_vec()],
            patch_bloom,
        )),
        RECORD_KIND_DELTA_CHAIN => {
            let (base, mut patches, existing_bloom) = parse_delta_chain_record(body)?;
            if patches.len() >= MAX_DELTA_CHAIN as usize {
                // 5th patch triggers automatic compaction via RecordCompactor!
                // Patches are stored newest-first (head_delta_slot order).
                patches.insert(0, patch_bytes.to_vec());
                let patch_slices: Vec<&[u8]> = patches.iter().map(|p| p.as_slice()).collect();
                let budget = BudgetHandle::new(BudgetDomain::Transaction, 16 * 1024 * 1024);
                let compacted = DocumentRecordCompactor.compact(&base, &patch_slices, &budget)?;
                let doc = DocumentBuf::from_bytes(compacted)?;
                Ok(encode_collection_record(&doc))
            } else {
                patches.insert(0, patch_bytes.to_vec());
                Ok(encode_delta_chain_record(
                    &base,
                    &patches,
                    existing_bloom | patch_bloom,
                ))
            }
        }
        _ => Ok(encode_collection_record(fallback_compacted)),
    }
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

        let new_rec_val = append_or_compact_record(
            &raw,
            &mutated.patch_bytes,
            mutated.bloom_mask,
            &mutated.updated_doc,
        )?;
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

/// Atomically creates and backfills a secondary index on `collection_name` inside a savepoint.
#[allow(clippy::too_many_arguments)]
pub fn create_secondary_index_stmt(
    tx: &mut WriteTxn<'_>,
    registries: &CoreRegistries,
    collection_name: &str,
    index_name: Option<&str>,
    raw_path: &str,
    order: SortOrder,
    unique: bool,
    sparse: bool,
) -> Result<IndexDescriptor> {
    let field_path = FieldPath::parse(raw_path)?;
    let name = match index_name {
        Some(n) if !n.is_empty() => n.to_owned(),
        _ => format!(
            "{}_{}",
            field_path.as_str().replace('.', "_"),
            match order {
                SortOrder::Asc => "1",
                SortOrder::Desc => "-1",
            }
        ),
    };

    let sp = tx.savepoint();
    let res = (|| -> Result<IndexDescriptor> {
        let catalog_root = tx.catalog_root_page_id();
        let mut entry =
            load_collection_entry(tx, catalog_root, collection_name)?.ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::NotFound,
                    MODULE_ID,
                    "write::create_secondary_index_stmt",
                    format!("collection '{collection_name}' does not exist"),
                )
            })?;

        if entry.indexes.iter().any(|i| i.name == name) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::DuplicateKey,
                MODULE_ID,
                "write::create_secondary_index_stmt",
                format!("index '{name}' already exists on collection '{collection_name}'"),
            ));
        }

        let mut meta = load_catalog_meta(tx, catalog_root)?;
        let index_id = IndexId::new(meta.next_index_id)?;
        meta.next_index_id = meta.next_index_id.checked_add(1).ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "write::create_secondary_index_stmt",
                "index_id counter overflow",
            )
        })?;

        let mut desc = IndexDescriptor::secondary(
            index_id,
            name,
            field_path,
            order,
            unique,
            sparse,
            PageId::new(1),
        );
        let provider = registries.indexes.require(&desc.provider_id)?;
        desc.root_page_id = provider.create_empty(tx, &desc)?;

        // Backfill all existing documents in the collection inside the same savepoint.
        let data_tree = BTree::open(entry.data_root_page_id, TreeKind::Collection);
        let existing = data_tree.scan_all(tx)?;
        for (raw_rid, raw_rec) in existing {
            let rid = decode_record_id(&raw_rid)?;
            let doc = decode_collection_record(&raw_rec)?;
            let maint = provider.on_insert(tx, &desc, rid, &doc.view())?;
            desc.root_page_id = maint.new_root_page_id;
        }

        entry.indexes.push(desc.clone());
        let root_after_meta = save_catalog_meta(tx, catalog_root, &meta)?;
        let root_after_entry = save_collection_entry(tx, root_after_meta, &entry)?;
        tx.set_catalog_root_page_id(root_after_entry);
        Ok(desc)
    })();

    match res {
        Ok(desc) => {
            tx.release_savepoint(sp)?;
            Ok(desc)
        }
        Err(err) => {
            let _ = tx.rollback_to(sp);
            Err(err)
        }
    }
}

/// Atomically drops a non-primary index from `collection_name` inside a savepoint.
pub fn drop_index_stmt(
    tx: &mut WriteTxn<'_>,
    registries: &CoreRegistries,
    collection_name: &str,
    index_name: &str,
) -> Result<bool> {
    let sp = tx.savepoint();
    let res = (|| -> Result<bool> {
        let catalog_root = tx.catalog_root_page_id();
        let mut entry =
            load_collection_entry(tx, catalog_root, collection_name)?.ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::NotFound,
                    MODULE_ID,
                    "write::drop_index_stmt",
                    format!("collection '{collection_name}' does not exist"),
                )
            })?;

        let Some(pos) = entry.indexes.iter().position(|i| i.name == index_name) else {
            return Ok(false);
        };
        if entry.indexes[pos].kind == IndexKind::Primary {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "write::drop_index_stmt",
                "cannot drop mandatory primary '_id_' index",
            ));
        }
        let desc = entry.indexes.remove(pos);
        let provider = registries.indexes.require(&desc.provider_id)?;
        provider.drop_index(tx, &desc)?;

        let new_root = save_collection_entry(tx, catalog_root, &entry)?;
        tx.set_catalog_root_page_id(new_root);
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

/// Atomically creates and backfills a compound/multi-key index inside a savepoint.
#[allow(clippy::too_many_arguments)]
pub fn create_compound_index_stmt(
    tx: &mut WriteTxn<'_>,
    registries: &CoreRegistries,
    collection_name: &str,
    name: Option<&str>,
    fields: &[(&str, SortOrder)],
    unique: bool,
    sparse: bool,
) -> Result<IndexDescriptor> {
    if fields.is_empty() {
        return Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "write::create_compound_index_stmt",
            "compound index requires at least one field",
        ));
    }
    let mut specs = Vec::with_capacity(fields.len());
    let mut auto_parts = Vec::with_capacity(fields.len());
    for &(raw_path, order) in fields {
        let path = FieldPath::parse(raw_path)?;
        let dir = match order {
            SortOrder::Asc => "1",
            SortOrder::Desc => "-1",
        };
        auto_parts.push(format!("{raw_path}_{dir}"));
        specs.push(IndexFieldSpec { path, order });
    }
    let idx_name = match name {
        Some(n) if !n.is_empty() => n.to_owned(),
        _ => auto_parts.join("_"),
    };

    let sp = tx.savepoint();
    let res = (|| -> Result<IndexDescriptor> {
        let catalog_root = tx.catalog_root_page_id();
        let mut entry =
            load_collection_entry(tx, catalog_root, collection_name)?.ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::NotFound,
                    MODULE_ID,
                    "write::create_compound_index_stmt",
                    format!("collection '{collection_name}' does not exist"),
                )
            })?;

        if entry.indexes.iter().any(|i| i.name == idx_name) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::DuplicateKey,
                MODULE_ID,
                "write::create_compound_index_stmt",
                format!("index '{idx_name}' already exists on collection '{collection_name}'"),
            ));
        }

        let mut meta = load_catalog_meta(tx, catalog_root)?;
        let index_id = IndexId::new(meta.next_index_id)?;
        meta.next_index_id = meta.next_index_id.checked_add(1).ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "write::create_compound_index_stmt",
                "next_index_id overflow",
            )
        })?;

        let provider_id = CapabilityId::new(COMPOUND_PROVIDER_ID)?;
        let provider = registries.indexes.require(&provider_id)?;
        let placeholder_root = PageId::new(1);
        let mut desc =
            IndexDescriptor::compound(index_id, idx_name, specs, unique, sparse, placeholder_root);
        desc.root_page_id = provider.create_empty(tx, &desc)?;

        let data_tree = BTree::open(entry.data_root_page_id, TreeKind::Collection);
        let existing = data_tree.scan_all(tx)?;
        for (raw_rid, raw_rec) in existing {
            let rid = decode_record_id(&raw_rid)?;
            let doc = decode_collection_record(&raw_rec)?;
            let maint = provider.on_insert(tx, &desc, rid, &doc.view())?;
            desc.root_page_id = maint.new_root_page_id;
        }

        entry.indexes.push(desc.clone());
        let root_after_meta = save_catalog_meta(tx, catalog_root, &meta)?;
        let root_after_entry = save_collection_entry(tx, root_after_meta, &entry)?;
        tx.set_catalog_root_page_id(root_after_entry);
        Ok(desc)
    })();

    match res {
        Ok(desc) => {
            tx.release_savepoint(sp)?;
            Ok(desc)
        }
        Err(err) => {
            let _ = tx.rollback_to(sp);
            Err(err)
        }
    }
}
