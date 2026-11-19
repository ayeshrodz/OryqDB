//! Versioned transactional catalog stored in the database's catalog B+ tree
//! (`docs/file-format.md` §4.1 & `docs/modules/core.md` §4).

pub mod codec;

pub use codec::{
    decode_catalog_entry, decode_catalog_meta, encode_catalog_entry, encode_catalog_meta,
    CatalogEntry, CatalogMeta, CATALOG_META_KEY, CATALOG_SCHEMA_VERSION,
};

use oryq_engine::{BTree, PageStore, PageStoreMut, TreeKind};
use oryq_types::{ErrorCategory, ErrorEnvelope, PageId, Result};

const MODULE_ID: &str = "core";

/// Loads global monotonic catalog counters (`next_collection_id`, `next_index_id`).
pub fn load_catalog_meta(store: &impl PageStore, catalog_root: PageId) -> Result<CatalogMeta> {
    let tree = BTree::open(catalog_root, TreeKind::Collection);
    match tree.get(store, CATALOG_META_KEY.as_bytes())? {
        Some(bytes) => decode_catalog_meta(&bytes),
        None => Ok(CatalogMeta::initial()),
    }
}

/// Persists global catalog counters into the catalog B+ tree, returning the new catalog root.
pub fn save_catalog_meta(
    store: &mut impl PageStoreMut,
    catalog_root: PageId,
    meta: &CatalogMeta,
) -> Result<PageId> {
    let bytes = encode_catalog_meta(meta)?;
    let mut tree = BTree::open(catalog_root, TreeKind::Collection);
    tree.insert(store, CATALOG_META_KEY.as_bytes(), &bytes)?;
    Ok(tree.root())
}

/// Loads a collection's [`CatalogEntry`] by name, or `Ok(None)` if not found.
pub fn load_collection_entry(
    store: &impl PageStore,
    catalog_root: PageId,
    name: &str,
) -> Result<Option<CatalogEntry>> {
    validate_collection_name(name)?;
    let tree = BTree::open(catalog_root, TreeKind::Collection);
    match tree.get(store, name.as_bytes())? {
        Some(bytes) => Ok(Some(decode_catalog_entry(&bytes)?)),
        None => Ok(None),
    }
}

/// Persists a collection's [`CatalogEntry`] into the catalog B+ tree, returning the new catalog root.
pub fn save_collection_entry(
    store: &mut impl PageStoreMut,
    catalog_root: PageId,
    entry: &CatalogEntry,
) -> Result<PageId> {
    validate_collection_name(&entry.name)?;
    let bytes = encode_catalog_entry(entry)?;
    let mut tree = BTree::open(catalog_root, TreeKind::Collection);
    tree.insert(store, entry.name.as_bytes(), &bytes)?;
    Ok(tree.root())
}

/// Removes a collection entry from the catalog B+ tree, returning `(new_catalog_root, removed_entry)`.
pub fn delete_collection_entry(
    store: &mut impl PageStoreMut,
    catalog_root: PageId,
    name: &str,
) -> Result<(PageId, Option<CatalogEntry>)> {
    validate_collection_name(name)?;
    let mut tree = BTree::open(catalog_root, TreeKind::Collection);
    match tree.delete(store, name.as_bytes())? {
        Some(raw) => Ok((tree.root(), Some(decode_catalog_entry(&raw)?))),
        None => Ok((tree.root(), None)),
    }
}

/// Lists all user collection names in lexicographical order (excluding internal `__oryq_*` keys).
pub fn list_collection_names(store: &impl PageStore, catalog_root: PageId) -> Result<Vec<String>> {
    let tree = BTree::open(catalog_root, TreeKind::Collection);
    let pairs = tree.scan_all(store)?;
    let mut names = Vec::new();
    for (k, _) in pairs {
        let name = core::str::from_utf8(&k).map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "catalog::list_collection_names",
                "non-UTF8 key in catalog B+ tree",
            )
        })?;
        if !name.starts_with("__oryq_") {
            names.push(name.to_owned());
        }
    }
    Ok(names)
}

/// Validates a user collection name (non-empty, no NUL, cannot use reserved `__oryq_` prefix).
pub fn validate_collection_name(name: &str) -> Result<()> {
    if name.is_empty() || name.contains('\0') || name.starts_with("__oryq_") {
        return Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "catalog::validate_collection_name",
            format!("invalid collection name {name:?}"),
        ));
    }
    Ok(())
}
