//! Primary `_id -> RecordId` snapshot lookup and ordered scan helpers.

use oryq_document::ValueView;
use oryq_engine::{BTree, PageStore, TreeKind};
use oryq_types::{PageId, RecordId, Result};

use crate::key_codec::{decode_record_id, encode_primary_id_key};

/// Looks up a single user `_id` in the primary index B+ tree rooted at `root_page_id`.
pub fn lookup_primary_id(
    store: &impl PageStore,
    root_page_id: PageId,
    id_value: &ValueView<'_>,
) -> Result<Option<RecordId>> {
    let encoded_key = encode_primary_id_key(id_value, store.page_size())?;
    let tree = BTree::open(root_page_id, TreeKind::Index);
    match tree.get(store, &encoded_key)? {
        Some(raw) => Ok(Some(decode_record_id(&raw)?)),
        None => Ok(None),
    }
}

/// Scans all `RecordId`s in canonical `_id` ascending order.
pub fn scan_primary_record_ids(
    store: &impl PageStore,
    root_page_id: PageId,
) -> Result<Vec<RecordId>> {
    let tree = BTree::open(root_page_id, TreeKind::Index);
    let pairs = tree.scan_all(store)?;
    let mut out = Vec::with_capacity(pairs.len());
    for (_, val_bytes) in pairs {
        out.push(decode_record_id(&val_bytes)?);
    }
    Ok(out)
}
