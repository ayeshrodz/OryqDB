//! B+ tree deletion, sibling borrow/merge rebalancing, and root collapse.

use oryq_types::{ErrorCategory, ErrorEnvelope, PageId, Result};

use crate::btree::layout::{
    build_internal_page, build_leaf_page, internal_serialized_bytes, leaf_serialized_bytes,
    max_page_usable_bytes, read_internal_node, read_leaf_entries, InternalNode, PageStoreMut,
    TreeKind,
};
use crate::btree::search::{descend_to_leaf, DescentFrame};

const MODULE_ID: &str = "engine";

/// Deletes `key` from the B+ tree rooted at `root`.
///
/// Returns `(new_root_page_id, removed_value_if_present)`.
pub fn btree_delete(
    store: &mut impl PageStoreMut,
    root: PageId,
    kind: TreeKind,
    key: &[u8],
) -> Result<(PageId, Option<Vec<u8>>)> {
    let (leaf_pid, leaf_bytes, mut path) = descend_to_leaf(store, root, key)?;
    let mut entries = read_leaf_entries(&leaf_bytes, leaf_pid)?;

    let Ok(idx) = entries.binary_search_by(|e| e.key.as_slice().cmp(key)) else {
        return Ok((root, None));
    };
    let removed = entries.remove(idx).value;

    let page_size = store.page_size();
    let min_bytes = max_page_usable_bytes(page_size) / 3;
    let lsn = store.writer_tx_id();

    // Root leaf: simply rewrite (an empty tree has one empty root leaf).
    if path.is_empty() {
        let raw =
            build_leaf_page(page_size, leaf_pid, kind, true, lsn, &entries)?.ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::Internal,
                    MODULE_ID,
                    "btree::btree_delete",
                    "root leaf unexpectedly failed to serialize after delete",
                )
            })?;
        store.write_page(leaf_pid, &raw)?;
        return Ok((root, Some(removed)));
    }

    // No underflow: rewrite leaf and finish.
    if !entries.is_empty() && leaf_serialized_bytes(&entries) >= min_bytes {
        let raw =
            build_leaf_page(page_size, leaf_pid, kind, false, lsn, &entries)?.ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::Internal,
                    MODULE_ID,
                    "btree::btree_delete",
                    "leaf unexpectedly failed to serialize after delete",
                )
            })?;
        store.write_page(leaf_pid, &raw)?;
        return Ok((root, Some(removed)));
    }

    // Underflow on a non-root leaf: inspect parent to borrow or merge with an adjacent sibling.
    let parent_frame = path.pop().unwrap();
    let parent_bytes = store.read_page(parent_frame.page_id)?;
    let mut parent = read_internal_node(&parent_bytes, parent_frame.page_id)?;
    let child_idx = parent_frame.child_slot_idx;

    // 1. Try left sibling if available (`child_idx > 0`).
    if child_idx > 0 {
        let left_pid = child_at(&parent, child_idx - 1);
        let left_bytes = store.read_page(left_pid)?;
        let mut left_entries = read_leaf_entries(&left_bytes, left_pid)?;

        // Try merging `entries` into `left_entries`.
        let mut merged = left_entries.clone();
        merged.extend(entries.iter().cloned());
        if let Some(merged_raw) = build_leaf_page(page_size, left_pid, kind, false, lsn, &merged)? {
            store.write_page(left_pid, &merged_raw)?;
            store.free_page(leaf_pid)?;
            parent.separators.remove(child_idx - 1);
            let new_root =
                rebalance_internal(store, root, kind, parent_frame.page_id, parent, path)?;
            return Ok((new_root, Some(removed)));
        }

        // Otherwise try borrowing the last entry from `left_entries` (if left has > 1 entry).
        if left_entries.len() > 1 {
            let borrowed = left_entries.pop().unwrap();
            entries.insert(0, borrowed);
            let new_sep = entries[0].key.clone();
            parent.separators[child_idx - 1].0 = new_sep;

            if let (Some(left_raw), Some(right_raw), Some(parent_raw)) = (
                build_leaf_page(page_size, left_pid, kind, false, lsn, &left_entries)?,
                build_leaf_page(page_size, leaf_pid, kind, false, lsn, &entries)?,
                build_internal_page(
                    page_size,
                    parent_frame.page_id,
                    kind,
                    path.is_empty(),
                    lsn,
                    &parent,
                )?,
            ) {
                store.write_page(left_pid, &left_raw)?;
                store.write_page(leaf_pid, &right_raw)?;
                store.write_page(parent_frame.page_id, &parent_raw)?;
                return Ok((root, Some(removed)));
            }
            // Restore if parent separator grew too large to fit.
            let put_back = entries.remove(0);
            left_entries.push(put_back);
        }
    }

    // 2. Try right sibling if available (`child_idx < parent.separators.len()`).
    if child_idx < parent.separators.len() {
        let right_pid = child_at(&parent, child_idx + 1);
        let right_bytes = store.read_page(right_pid)?;
        let mut right_entries = read_leaf_entries(&right_bytes, right_pid)?;

        // Try merging `right_entries` into `leaf_pid`.
        let mut merged = entries.clone();
        merged.extend(right_entries.iter().cloned());
        if let Some(merged_raw) = build_leaf_page(page_size, leaf_pid, kind, false, lsn, &merged)? {
            store.write_page(leaf_pid, &merged_raw)?;
            store.free_page(right_pid)?;
            parent.separators.remove(child_idx);
            let new_root =
                rebalance_internal(store, root, kind, parent_frame.page_id, parent, path)?;
            return Ok((new_root, Some(removed)));
        }

        // Otherwise try borrowing the first entry from `right_entries`.
        if right_entries.len() > 1 {
            let borrowed = right_entries.remove(0);
            entries.push(borrowed);
            let new_sep = right_entries[0].key.clone();
            parent.separators[child_idx].0 = new_sep;

            if let (Some(left_raw), Some(right_raw), Some(parent_raw)) = (
                build_leaf_page(page_size, leaf_pid, kind, false, lsn, &entries)?,
                build_leaf_page(page_size, right_pid, kind, false, lsn, &right_entries)?,
                build_internal_page(
                    page_size,
                    parent_frame.page_id,
                    kind,
                    path.is_empty(),
                    lsn,
                    &parent,
                )?,
            ) {
                store.write_page(leaf_pid, &left_raw)?;
                store.write_page(right_pid, &right_raw)?;
                store.write_page(parent_frame.page_id, &parent_raw)?;
                return Ok((root, Some(removed)));
            }
        }
    }

    // Fallback (only reachable if leaf is still non-empty and neighbors couldn't merge/lend):
    // rewrite leaf in place.
    let raw =
        build_leaf_page(page_size, leaf_pid, kind, false, lsn, &entries)?.ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Internal,
                MODULE_ID,
                "btree::btree_delete",
                "leaf unexpectedly failed to serialize in fallback path",
            )
        })?;
    store.write_page(leaf_pid, &raw)?;
    Ok((root, Some(removed)))
}

fn rebalance_internal(
    store: &mut impl PageStoreMut,
    root: PageId,
    kind: TreeKind,
    node_pid: PageId,
    node: InternalNode,
    mut path: Vec<DescentFrame>,
) -> Result<PageId> {
    let page_size = store.page_size();
    let min_bytes = max_page_usable_bytes(page_size) / 3;
    let lsn = store.writer_tx_id();

    // If this internal node is the root:
    if path.is_empty() {
        if node.separators.is_empty() {
            // Root collapse: promote `leftmost_child` to new root and free old root page.
            let new_root = node.leftmost_child;
            let child_bytes = store.read_page(new_root)?;
            if let Ok(entries) = read_leaf_entries(&child_bytes, new_root) {
                let raw = build_leaf_page(page_size, new_root, kind, true, lsn, &entries)?
                    .ok_or_else(|| {
                        ErrorEnvelope::new(
                            ErrorCategory::Internal,
                            MODULE_ID,
                            "btree::rebalance_internal",
                            "promoted leaf root failed to serialize",
                        )
                    })?;
                store.write_page(new_root, &raw)?;
            } else {
                let child_node = read_internal_node(&child_bytes, new_root)?;
                let raw = build_internal_page(page_size, new_root, kind, true, lsn, &child_node)?
                    .ok_or_else(|| {
                    ErrorEnvelope::new(
                        ErrorCategory::Internal,
                        MODULE_ID,
                        "btree::rebalance_internal",
                        "promoted internal root failed to serialize",
                    )
                })?;
                store.write_page(new_root, &raw)?;
            }
            store.free_page(node_pid)?;
            return Ok(new_root);
        }

        let raw =
            build_internal_page(page_size, node_pid, kind, true, lsn, &node)?.ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::Internal,
                    MODULE_ID,
                    "btree::rebalance_internal",
                    "root internal node failed to serialize",
                )
            })?;
        store.write_page(node_pid, &raw)?;
        return Ok(root);
    }

    // Non-root internal node above underflow threshold: just rewrite.
    if !node.separators.is_empty() && internal_serialized_bytes(&node) >= min_bytes {
        let raw = build_internal_page(page_size, node_pid, kind, false, lsn, &node)?.ok_or_else(
            || {
                ErrorEnvelope::new(
                    ErrorCategory::Internal,
                    MODULE_ID,
                    "btree::rebalance_internal",
                    "internal node failed to serialize",
                )
            },
        )?;
        store.write_page(node_pid, &raw)?;
        return Ok(root);
    }

    // Non-root internal node underflow: try merging with left or right sibling.
    let parent_frame = path.pop().unwrap();
    let parent_bytes = store.read_page(parent_frame.page_id)?;
    let mut parent = read_internal_node(&parent_bytes, parent_frame.page_id)?;
    let child_idx = parent_frame.child_slot_idx;

    if child_idx > 0 {
        let left_pid = child_at(&parent, child_idx - 1);
        let left_bytes = store.read_page(left_pid)?;
        let left_node = read_internal_node(&left_bytes, left_pid)?;

        let mut merged = left_node;
        let pull_down_key = parent.separators[child_idx - 1].0.clone();
        merged.separators.push((pull_down_key, node.leftmost_child));
        merged.separators.extend(node.separators.iter().cloned());

        if let Some(merged_raw) =
            build_internal_page(page_size, left_pid, kind, false, lsn, &merged)?
        {
            store.write_page(left_pid, &merged_raw)?;
            store.free_page(node_pid)?;
            parent.separators.remove(child_idx - 1);
            return rebalance_internal(store, root, kind, parent_frame.page_id, parent, path);
        }
    }

    if child_idx < parent.separators.len() {
        let right_pid = child_at(&parent, child_idx + 1);
        let right_bytes = store.read_page(right_pid)?;
        let right_node = read_internal_node(&right_bytes, right_pid)?;

        let mut merged = node.clone();
        let pull_down_key = parent.separators[child_idx].0.clone();
        merged
            .separators
            .push((pull_down_key, right_node.leftmost_child));
        merged.separators.extend(right_node.separators);

        if let Some(merged_raw) =
            build_internal_page(page_size, node_pid, kind, false, lsn, &merged)?
        {
            store.write_page(node_pid, &merged_raw)?;
            store.free_page(right_pid)?;
            parent.separators.remove(child_idx);
            return rebalance_internal(store, root, kind, parent_frame.page_id, parent, path);
        }
    }

    // Fallback: write node as-is.
    let raw =
        build_internal_page(page_size, node_pid, kind, false, lsn, &node)?.ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Internal,
                MODULE_ID,
                "btree::rebalance_internal",
                "internal node failed to serialize in fallback path",
            )
        })?;
    store.write_page(node_pid, &raw)?;
    Ok(root)
}

#[inline]
fn child_at(node: &InternalNode, slot_idx: usize) -> PageId {
    if slot_idx == 0 {
        node.leftmost_child
    } else {
        node.separators[slot_idx - 1].1
    }
}
