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
use oryq_engine::btree::{read_internal_node, read_leaf_entries};
use oryq_engine::{
    BTree, OverflowRef, OverflowStore, PageSize, PageStore, PageStoreMut, RecordCompactor,
    SlottedPageView, TreeKind, WriteTxn, OVERFLOW_REF_SIZE,
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
use crate::api::read::with_value_view;
use crate::catalog::{
    delete_collection_entry, load_catalog_meta, load_collection_entry, save_catalog_meta,
    save_collection_entry, validate_collection_name, CatalogEntry,
};
use crate::composition::CoreRegistries;

const MODULE_ID: &str = "core";
const RECORD_KIND_INLINE: u8 = 0x00;
const RECORD_KIND_DELTA_CHAIN: u8 = 0x01;
const RECORD_KIND_OVERFLOW: u8 = 0x02;
/// Maximum number of chained physical delta patches before automatic compaction (`docs/file-format.md` §4.2).
pub const MAX_DELTA_CHAIN: u8 = 4;

/// Maximum inline collection record envelope length before spilling into `PageType::Overflow` pages.
#[inline]
pub const fn max_inline_record_bytes(page_size: PageSize) -> usize {
    // Leaves headroom for 32B PageHeader + 16B SlotEntry + 8B B+ tree KV lengths + 8B RecordId key.
    page_size.bytes().saturating_sub(128)
}

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

/// Persists `doc` either inline (`0x00`) when it fits comfortably in a collection leaf page,
/// or across chained `PageType::Overflow` pages (`0x02` + 16-byte [`OverflowRef`]).
fn store_collection_record(tx: &mut WriteTxn<'_>, doc: &DocumentBuf) -> Result<Vec<u8>> {
    let max_inline = max_inline_record_bytes(tx.page_size());
    if doc.len() < max_inline {
        return Ok(encode_collection_record(doc));
    }
    let oref = OverflowStore::write_in_store(tx, doc.as_bytes(), RECORD_KIND_INLINE)?;
    let mut out = [0u8; 1 + OVERFLOW_REF_SIZE];
    out[0] = RECORD_KIND_OVERFLOW;
    let mut ref_buf = [0u8; OVERFLOW_REF_SIZE];
    oref.encode(&mut ref_buf);
    out[1..].copy_from_slice(&ref_buf);
    Ok(out.to_vec())
}

/// Retires any chained `PageType::Overflow` pages referenced by an existing `0x02` record value.
fn free_record_overflow_if_any(tx: &mut WriteTxn<'_>, raw: &[u8]) -> Result<()> {
    if let Some((&RECORD_KIND_OVERFLOW, body)) = raw.split_first() {
        let raw_ref: &[u8; OVERFLOW_REF_SIZE] = body.try_into().map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "write::free_record_overflow_if_any",
                format!("invalid overflow record reference length {}", body.len()),
            )
        })?;
        let oref = OverflowRef::decode(raw_ref)?;
        OverflowStore::free_in_store(tx, &oref)?;
    }
    Ok(())
}

/// Decodes a collection B+ tree value (inline `0x00`, delta-chained `0x01`, or multi-page
/// overflow `0x02` via `store`) into an immutable validated [`DocumentBuf`].
pub fn decode_collection_record_from_store(
    store: &impl PageStore,
    raw: &[u8],
) -> Result<DocumentBuf> {
    if let Some((&RECORD_KIND_OVERFLOW, body)) = raw.split_first() {
        let raw_ref: &[u8; OVERFLOW_REF_SIZE] = body.try_into().map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "write::decode_collection_record_from_store",
                format!("invalid overflow record reference length {}", body.len()),
            )
        })?;
        let oref = OverflowRef::decode(raw_ref)?;
        let (blob, _) = OverflowStore::read_from_store(store, &oref, None)?;
        return DocumentBuf::from_bytes(blob);
    }
    decode_collection_record(raw)
}

/// Decodes an inline (`0x00`) or delta-chained (`0x01`) collection record into a [`DocumentBuf`].
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
/// `MAX_DELTA_CHAIN` (4) AND within `max_inline_bytes`, or compacts automatically back to
/// `RECORD_KIND_INLINE` (or spills to `RECORD_KIND_OVERFLOW` if the compacted doc exceeds leaf size).
fn append_or_compact_record(
    tx: &mut WriteTxn<'_>,
    raw_existing: &[u8],
    patch_bytes: &[u8],
    patch_bloom: u64,
    fallback_compacted: &DocumentBuf,
) -> Result<Vec<u8>> {
    let max_inline = max_inline_record_bytes(tx.page_size());
    if patch_bytes.is_empty() {
        return store_collection_record(tx, fallback_compacted);
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
        RECORD_KIND_INLINE => {
            let projected = 1 + 1 + 8 + 4 + body.len() + 4 + patch_bytes.len();
            if projected > max_inline {
                store_collection_record(tx, fallback_compacted)
            } else {
                Ok(encode_delta_chain_record(
                    body,
                    &[patch_bytes.to_vec()],
                    patch_bloom,
                ))
            }
        }
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
                store_collection_record(tx, &doc)
            } else {
                patches.insert(0, patch_bytes.to_vec());
                let total_patches: usize = patches.iter().map(|p| 4 + p.len()).sum();
                let projected = 1 + 1 + 8 + 4 + base.len() + total_patches;
                if projected > max_inline {
                    store_collection_record(tx, fallback_compacted)
                } else {
                    Ok(encode_delta_chain_record(
                        &base,
                        &patches,
                        existing_bloom | patch_bloom,
                    ))
                }
            }
        }
        RECORD_KIND_OVERFLOW => {
            free_record_overflow_if_any(tx, raw_existing)?;
            store_collection_record(tx, fallback_compacted)
        }
        _ => store_collection_record(tx, fallback_compacted),
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
        free_collection_tree_pages(tx, entry.data_root_page_id)?;
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
    let mut entry = load_collection_entry(tx, tx.catalog_root_page_id(), collection_name)?
        .ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::NotFound,
                MODULE_ID,
                "write::insert_doc_stmt",
                format!("collection '{collection_name}' does not exist"),
            )
        })?;
    insert_doc_with_entry(tx, registries, &mut entry, doc, max_document_bytes)
}

/// Inserts `doc` using a pre-loaded [`CatalogEntry`], updating `entry` in place only on success.
pub(crate) fn insert_doc_with_entry(
    tx: &mut WriteTxn<'_>,
    registries: &CoreRegistries,
    entry: &mut CatalogEntry,
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
    let mut next_entry = entry.clone();
    let res = (|| -> Result<(Value, RecordId)> {
        let catalog_root = tx.catalog_root_page_id();

        let record_id = next_entry.next_record_id;
        next_entry.next_record_id = record_id.checked_next()?;

        let mut data_tree = BTree::open(next_entry.data_root_page_id, TreeKind::Collection);
        let rec_key = encode_record_id(record_id);
        let rec_val = store_collection_record(tx, &doc_with_id)?;
        data_tree.insert(tx, &rec_key, &rec_val)?;
        next_entry.data_root_page_id = data_tree.root();

        let view = doc_with_id.view();
        for desc in &mut next_entry.indexes {
            let provider = registries.indexes.require(&desc.provider_id)?;
            let maint = provider.on_insert(tx, desc, record_id, &view)?;
            desc.root_page_id = maint.new_root_page_id;
        }

        let new_catalog_root = save_collection_entry(tx, catalog_root, &next_entry)?;
        tx.set_catalog_root_page_id(new_catalog_root);
        Ok((id_value, record_id))
    })();

    match res {
        Ok(ok) => {
            tx.release_savepoint(sp)?;
            *entry = next_entry;
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
    let mut entry = load_collection_entry(tx, tx.catalog_root_page_id(), collection_name)?
        .ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::NotFound,
                MODULE_ID,
                "write::update_by_id_stmt",
                format!("collection '{collection_name}' does not exist"),
            )
        })?;
    update_by_id_with_entry(
        tx,
        registries,
        &mut entry,
        id_value,
        update_spec,
        max_document_bytes,
    )
}

/// Applies `update_spec` using a pre-loaded [`CatalogEntry`], skipping `save_collection_entry`
/// whenever no tree root page changed.
pub(crate) fn update_by_id_with_entry(
    tx: &mut WriteTxn<'_>,
    registries: &CoreRegistries,
    entry: &mut CatalogEntry,
    id_value: &Value,
    update_spec: &UpdateSpec,
    max_document_bytes: usize,
) -> Result<bool> {
    let sp = tx.savepoint();
    let mut next_entry = entry.clone();
    let res = (|| -> Result<bool> {
        let catalog_root = tx.catalog_root_page_id();

        let primary_root = next_entry.primary_index()?.root_page_id;
        let Some(record_id) = with_value_view(id_value, |id_view| {
            lookup_primary_id(tx, primary_root, id_view)
        })?
        else {
            return Ok(false);
        };

        let mut data_tree = BTree::open(next_entry.data_root_page_id, TreeKind::Collection);
        let rec_key = encode_record_id(record_id);
        let raw = data_tree.get(tx, &rec_key)?.ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "write::update_by_id_stmt",
                format!(
                    "primary index points to missing RecordId({}) in collection '{}'",
                    record_id.raw(),
                    next_entry.name
                ),
            )
        })?;
        let existing_doc = decode_collection_record_from_store(tx, &raw)?;
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
            tx,
            &raw,
            &mutated.patch_bytes,
            mutated.bloom_mask,
            &mutated.updated_doc,
        )?;
        data_tree.insert(tx, &rec_key, &new_rec_val)?;
        let mut catalog_changed = data_tree.root() != next_entry.data_root_page_id;
        next_entry.data_root_page_id = data_tree.root();

        let before_view = existing_doc.view();
        let after_view = mutated.updated_doc.view();
        for desc in &mut next_entry.indexes {
            let provider = registries.indexes.require(&desc.provider_id)?;
            let maint = provider.on_update(tx, desc, record_id, &before_view, &after_view)?;
            if maint.new_root_page_id != desc.root_page_id {
                desc.root_page_id = maint.new_root_page_id;
                catalog_changed = true;
            }
        }

        if catalog_changed {
            let new_catalog_root = save_collection_entry(tx, catalog_root, &next_entry)?;
            tx.set_catalog_root_page_id(new_catalog_root);
        }
        Ok(true)
    })();

    match res {
        Ok(ok) => {
            tx.release_savepoint(sp)?;
            *entry = next_entry;
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
    let mut entry = load_collection_entry(tx, tx.catalog_root_page_id(), collection_name)?
        .ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::NotFound,
                MODULE_ID,
                "write::replace_by_id_stmt",
                format!("collection '{collection_name}' does not exist"),
            )
        })?;
    replace_by_id_with_entry(
        tx,
        registries,
        &mut entry,
        id_value,
        replacement,
        max_document_bytes,
    )
}

/// Replaces the document matching `id_value` using a pre-loaded [`CatalogEntry`].
pub(crate) fn replace_by_id_with_entry(
    tx: &mut WriteTxn<'_>,
    registries: &CoreRegistries,
    entry: &mut CatalogEntry,
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
    let mut next_entry = entry.clone();
    let res = (|| -> Result<bool> {
        let catalog_root = tx.catalog_root_page_id();

        let primary_root = next_entry.primary_index()?.root_page_id;
        let Some(record_id) = with_value_view(id_value, |id_view| {
            lookup_primary_id(tx, primary_root, id_view)
        })?
        else {
            return Ok(false);
        };

        let mut data_tree = BTree::open(next_entry.data_root_page_id, TreeKind::Collection);
        let rec_key = encode_record_id(record_id);
        let raw = data_tree.get(tx, &rec_key)?.ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "write::replace_by_id_stmt",
                "primary index points to missing collection record",
            )
        })?;
        let existing_doc = decode_collection_record_from_store(tx, &raw)?;
        validate_replacement_doc(&existing_doc.view(), &replacement.view())?;

        free_record_overflow_if_any(tx, &raw)?;
        let new_rec_val = store_collection_record(tx, replacement)?;
        data_tree.insert(tx, &rec_key, &new_rec_val)?;
        let mut catalog_changed = data_tree.root() != next_entry.data_root_page_id;
        next_entry.data_root_page_id = data_tree.root();

        let before_view = existing_doc.view();
        let after_view = replacement.view();
        for desc in &mut next_entry.indexes {
            let provider = registries.indexes.require(&desc.provider_id)?;
            let maint = provider.on_update(tx, desc, record_id, &before_view, &after_view)?;
            if maint.new_root_page_id != desc.root_page_id {
                desc.root_page_id = maint.new_root_page_id;
                catalog_changed = true;
            }
        }

        if catalog_changed {
            let new_catalog_root = save_collection_entry(tx, catalog_root, &next_entry)?;
            tx.set_catalog_root_page_id(new_catalog_root);
        }
        Ok(true)
    })();

    match res {
        Ok(ok) => {
            tx.release_savepoint(sp)?;
            *entry = next_entry;
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
    let mut entry = load_collection_entry(tx, tx.catalog_root_page_id(), collection_name)?
        .ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::NotFound,
                MODULE_ID,
                "write::delete_by_id_stmt",
                format!("collection '{collection_name}' does not exist"),
            )
        })?;
    delete_by_id_with_entry(tx, registries, &mut entry, id_value)
}

/// Deletes the document matching `id_value` using a pre-loaded [`CatalogEntry`].
pub(crate) fn delete_by_id_with_entry(
    tx: &mut WriteTxn<'_>,
    registries: &CoreRegistries,
    entry: &mut CatalogEntry,
    id_value: &Value,
) -> Result<bool> {
    let sp = tx.savepoint();
    let mut next_entry = entry.clone();
    let res = (|| -> Result<bool> {
        let catalog_root = tx.catalog_root_page_id();

        let primary_root = next_entry.primary_index()?.root_page_id;
        let Some(record_id) = with_value_view(id_value, |id_view| {
            lookup_primary_id(tx, primary_root, id_view)
        })?
        else {
            return Ok(false);
        };

        let mut data_tree = BTree::open(next_entry.data_root_page_id, TreeKind::Collection);
        let rec_key = encode_record_id(record_id);
        let Some(raw) = data_tree.delete(tx, &rec_key)? else {
            return Ok(false);
        };
        let mut catalog_changed = data_tree.root() != next_entry.data_root_page_id;
        next_entry.data_root_page_id = data_tree.root();

        let existing_doc = decode_collection_record_from_store(tx, &raw)?;
        free_record_overflow_if_any(tx, &raw)?;
        let view = existing_doc.view();
        for desc in &mut next_entry.indexes {
            let provider = registries.indexes.require(&desc.provider_id)?;
            let maint = provider.on_delete(tx, desc, record_id, &view)?;
            if maint.new_root_page_id != desc.root_page_id {
                desc.root_page_id = maint.new_root_page_id;
                catalog_changed = true;
            }
        }

        if catalog_changed {
            let new_catalog_root = save_collection_entry(tx, catalog_root, &next_entry)?;
            tx.set_catalog_root_page_id(new_catalog_root);
        }
        Ok(true)
    })();

    match res {
        Ok(ok) => {
            tx.release_savepoint(sp)?;
            *entry = next_entry;
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
            let doc = decode_collection_record_from_store(tx, &raw_rec)?;
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

        // Recompute required_write_features from remaining indexes so dropping the last
        // optional-feature index clears its write feature bit cleanly.
        let mut req_write = FeatureBitmask::EMPTY;
        for remaining in &entry.indexes {
            if let Some(bit) = remaining.required_feature_bit {
                if bit < 64 {
                    req_write = req_write.union(FeatureBitmask::from_raw(1u64 << bit));
                }
            }
        }
        entry.required_write_features = req_write;

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

fn free_collection_tree_pages(tx: &mut WriteTxn<'_>, root: PageId) -> Result<()> {
    let mut stack = vec![root];
    let mut visited = HashSet::new();
    while let Some(pid) = stack.pop() {
        if !visited.insert(pid) {
            continue;
        }
        let bytes = tx.read_page(pid)?;
        let view = SlottedPageView::parse(&bytes, pid)?;
        let ptype = view.header().page_type;
        if TreeKind::is_internal(ptype) {
            let node = read_internal_node(&bytes, pid)?;
            stack.push(node.leftmost_child);
            for (_, child) in node.separators {
                stack.push(child);
            }
        } else if TreeKind::is_leaf(ptype) {
            for entry in read_leaf_entries(&bytes, pid)? {
                free_record_overflow_if_any(tx, &entry.value)?;
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
            let doc = decode_collection_record_from_store(tx, &raw_rec)?;
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

/// Atomically creates and backfills an optional full-text search (`index.fts`) index
/// inside a statement savepoint (`--features fts`).
#[cfg(feature = "fts")]
pub fn create_fts_index_stmt(
    tx: &mut WriteTxn<'_>,
    registries: &CoreRegistries,
    collection_name: &str,
    name: Option<&str>,
    fields: &[&str],
) -> Result<IndexDescriptor> {
    if fields.is_empty() {
        return Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "write::create_fts_index_stmt",
            "FTS index requires at least one text field path",
        ));
    }
    let mut parsed_paths = Vec::with_capacity(fields.len());
    for &raw_path in fields {
        parsed_paths.push(FieldPath::parse(raw_path)?);
    }
    let idx_name = match name {
        Some(n) if !n.is_empty() => n.to_owned(),
        _ => format!("{}_fts", fields.join("_")),
    };

    let sp = tx.savepoint();
    let res = (|| -> Result<IndexDescriptor> {
        let catalog_root = tx.catalog_root_page_id();
        let mut entry =
            load_collection_entry(tx, catalog_root, collection_name)?.ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::NotFound,
                    MODULE_ID,
                    "write::create_fts_index_stmt",
                    format!("collection '{collection_name}' does not exist"),
                )
            })?;

        if entry.indexes.iter().any(|i| i.name == idx_name) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::DuplicateKey,
                MODULE_ID,
                "write::create_fts_index_stmt",
                format!("index '{idx_name}' already exists on collection '{collection_name}'"),
            ));
        }

        let mut meta = load_catalog_meta(tx, catalog_root)?;
        let index_id = IndexId::new(meta.next_index_id)?;
        meta.next_index_id = meta.next_index_id.checked_add(1).ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "write::create_fts_index_stmt",
                "next_index_id overflow",
            )
        })?;

        let provider_id = CapabilityId::new(oryq_fts::FTS_PROVIDER_ID)?;
        let provider = registries.indexes.require(&provider_id)?;
        let mut desc =
            oryq_fts::fts_index_descriptor(index_id, idx_name, parsed_paths, PageId::new(1));
        desc.root_page_id = provider.create_empty(tx, &desc)?;

        let data_tree = BTree::open(entry.data_root_page_id, TreeKind::Collection);
        let existing = data_tree.scan_all(tx)?;
        for (raw_rid, raw_rec) in existing {
            let rid = decode_record_id(&raw_rid)?;
            let doc = decode_collection_record_from_store(tx, &raw_rec)?;
            let maint = provider.on_insert(tx, &desc, rid, &doc.view())?;
            desc.root_page_id = maint.new_root_page_id;
        }

        entry.indexes.push(desc.clone());
        let fts_bit = FeatureBitmask::from_raw(1u64 << oryq_fts::FTS_FEATURE_BIT);
        entry.required_write_features = entry.required_write_features.union(fts_bit);

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

/// Atomically creates and backfills an optional vector similarity (`index.vec`) index
/// inside a statement savepoint (`--features vec`).
#[cfg(feature = "vec")]
pub fn create_vector_index_stmt(
    tx: &mut WriteTxn<'_>,
    registries: &CoreRegistries,
    collection_name: &str,
    name: Option<&str>,
    raw_field: &str,
    options: oryq_vec::VectorIndexOptions,
) -> Result<IndexDescriptor> {
    options.validate()?;
    let field_path = FieldPath::parse(raw_field)?;
    let idx_name = match name {
        Some(n) if !n.is_empty() => n.to_owned(),
        _ => format!("{}_vec", field_path.as_str().replace('.', "_")),
    };

    let sp = tx.savepoint();
    let res = (|| -> Result<IndexDescriptor> {
        let catalog_root = tx.catalog_root_page_id();
        let mut entry =
            load_collection_entry(tx, catalog_root, collection_name)?.ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::NotFound,
                    MODULE_ID,
                    "write::create_vector_index_stmt",
                    format!("collection '{collection_name}' does not exist"),
                )
            })?;

        if entry.indexes.iter().any(|i| i.name == idx_name) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::DuplicateKey,
                MODULE_ID,
                "write::create_vector_index_stmt",
                format!("index '{idx_name}' already exists on collection '{collection_name}'"),
            ));
        }

        let mut meta = load_catalog_meta(tx, catalog_root)?;
        let index_id = IndexId::new(meta.next_index_id)?;
        meta.next_index_id = meta.next_index_id.checked_add(1).ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "write::create_vector_index_stmt",
                "next_index_id overflow",
            )
        })?;

        let provider_id = CapabilityId::new(oryq_vec::VEC_PROVIDER_ID)?;
        let provider = registries.indexes.require(&provider_id)?;
        let mut desc = oryq_vec::vec_index_descriptor(
            index_id,
            idx_name,
            field_path,
            options,
            PageId::new(1),
        )?;
        desc.root_page_id = provider.create_empty(tx, &desc)?;

        let data_tree = BTree::open(entry.data_root_page_id, TreeKind::Collection);
        let existing = data_tree.scan_all(tx)?;
        for (raw_rid, raw_rec) in existing {
            let rid = decode_record_id(&raw_rid)?;
            let doc = decode_collection_record_from_store(tx, &raw_rec)?;
            let maint = provider.on_insert(tx, &desc, rid, &doc.view())?;
            desc.root_page_id = maint.new_root_page_id;
        }

        entry.indexes.push(desc.clone());
        let vec_bit = FeatureBitmask::from_raw(1u64 << oryq_vec::VEC_FEATURE_BIT);
        entry.required_write_features = entry.required_write_features.union(vec_bit);

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
