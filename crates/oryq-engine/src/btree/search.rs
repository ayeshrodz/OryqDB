//! Point lookup and cycle-checked descent through logical B+ tree pages.

use std::collections::HashSet;
use std::sync::Arc;

use oryq_types::{ErrorCategory, ErrorEnvelope, PageId, Result};

use crate::btree::layout::{
    decode_internal_slot, decode_leaf_entry, PageStore, TreeKind, MAX_BTREE_HEIGHT,
};
use crate::format::SlottedPageView;

const MODULE_ID: &str = "engine";

/// One ancestor step recorded while descending from root to leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DescentFrame {
    pub page_id: PageId,
    /// Child slot index followed at this internal node (`0` = leftmost sentinel, `1..` = right child of separator).
    pub child_slot_idx: usize,
}

/// Descends from `root` to the leaf page that owns `key`, returning the leaf page ID,
/// raw validated leaf page bytes, and the ancestor path stack.
pub fn descend_to_leaf(
    store: &impl PageStore,
    root: PageId,
    key: &[u8],
) -> Result<(PageId, Arc<[u8]>, Vec<DescentFrame>)> {
    let mut visited = HashSet::new();
    let mut path = Vec::new();
    let mut current = root;

    loop {
        if path.len() >= MAX_BTREE_HEIGHT {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "btree::descend_to_leaf",
                format!("B+ tree height exceeded maximum {MAX_BTREE_HEIGHT}"),
            ));
        }
        if !visited.insert(current) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "btree::descend_to_leaf",
                format!("cycle detected in B+ tree at page {}", current.raw()),
            ));
        }

        let page_bytes = store.read_page(current)?;
        let view = SlottedPageView::parse(&page_bytes, current)?;
        let ptype = view.header().page_type;

        if TreeKind::is_leaf(ptype) {
            return Ok((current, page_bytes, path));
        }
        if !TreeKind::is_internal(ptype) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "btree::descend_to_leaf",
                format!(
                    "unexpected page type {:?} at page {} during B+ tree traversal",
                    ptype,
                    current.raw()
                ),
            ));
        }

        let (child_slot_idx, next_pid) = choose_child(&view, key)?;
        path.push(DescentFrame {
            page_id: current,
            child_slot_idx,
        });
        current = next_pid;
    }
}

/// Binary searches an internal page's separators (`1..slot_count`) to select the child
/// subtree for `key` (equality descends right, matching `docs/file-format.md` §4.1).
pub fn choose_child(view: &SlottedPageView<'_>, key: &[u8]) -> Result<(usize, PageId)> {
    let count = usize::from(view.slot_count());
    if count == 0 {
        return Err(ErrorEnvelope::new(
            ErrorCategory::Corruption,
            MODULE_ID,
            "btree::choose_child",
            "internal page has zero slots",
        ));
    }

    // Binary search over separator slots 1..count for the first separator > key.
    let mut lo = 1usize;
    let mut hi = count;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let payload = view.payload(mid as u16).ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "btree::choose_child",
                format!("tombstone in internal page slot {mid}"),
            )
        })?;
        let (sep, _) = decode_internal_slot(payload)?;
        if key < sep {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }

    // Follow slot `lo - 1` (slot 0 sentinel if key < all separators, otherwise right child of last sep <= key).
    let chosen_slot = lo - 1;
    let payload = view.payload(chosen_slot as u16).ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::Corruption,
            MODULE_ID,
            "btree::choose_child",
            format!("missing internal slot {chosen_slot}"),
        )
    })?;
    let (_, child_pid) = decode_internal_slot(payload)?;
    Ok((chosen_slot, child_pid))
}

/// Point lookup of `key` in the B+ tree rooted at `root`.
pub fn btree_get(store: &impl PageStore, root: PageId, key: &[u8]) -> Result<Option<Vec<u8>>> {
    let (leaf_pid, leaf_bytes, _) = descend_to_leaf(store, root, key)?;
    let view = SlottedPageView::parse(&leaf_bytes, leaf_pid)?;
    let count = usize::from(view.slot_count());

    let mut lo = 0usize;
    let mut hi = count;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let payload = view.payload(mid as u16).ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "btree::btree_get",
                format!("unexpected tombstone in compact leaf slot {mid}"),
            )
        })?;
        let (entry_key, entry_val) = decode_leaf_entry(payload)?;
        match entry_key.cmp(key) {
            core::cmp::Ordering::Less => lo = mid + 1,
            core::cmp::Ordering::Greater => hi = mid,
            core::cmp::Ordering::Equal => return Ok(Some(entry_val.to_vec())),
        }
    }

    Ok(None)
}
