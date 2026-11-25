//! B+ tree insertion, upsert, leaf split, internal split, and root growth.

use oryq_types::{ErrorCategory, ErrorEnvelope, PageId, Result};

use crate::btree::layout::{
    build_internal_page, build_leaf_page, leaf_serialized_bytes, max_page_usable_bytes,
    read_internal_node, read_leaf_entries, InternalNode, LeafEntry, PageStoreMut, TreeKind,
};
use crate::btree::search::descend_to_leaf;

const MODULE_ID: &str = "engine";

/// Inserts or replaces `(key, value)` in the B+ tree rooted at `root`.
///
/// Returns `(new_root_page_id, previous_value_if_replaced)`.
pub fn btree_insert(
    store: &mut impl PageStoreMut,
    root: PageId,
    kind: TreeKind,
    key: &[u8],
    value: &[u8],
) -> Result<(PageId, Option<Vec<u8>>)> {
    let page_size = store.page_size();
    let single_entry = [LeafEntry {
        key: key.to_vec(),
        value: value.to_vec(),
    }];
    if leaf_serialized_bytes(&single_entry) > max_page_usable_bytes(page_size) {
        return Err(ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "btree::btree_insert",
            format!(
                "single entry ({} bytes) exceeds maximum B+ tree leaf capacity ({} bytes); use overflow storage",
                leaf_serialized_bytes(&single_entry),
                max_page_usable_bytes(page_size)
            ),
        ));
    }

    let (leaf_pid, leaf_bytes, mut path) = descend_to_leaf(store, root, key)?;
    let mut entries = read_leaf_entries(&leaf_bytes, leaf_pid)?;

    let (previous, inserted_at_right_edge) =
        match entries.binary_search_by(|e| e.key.as_slice().cmp(key)) {
            Ok(idx) => {
                let prev = core::mem::replace(&mut entries[idx].value, value.to_vec());
                (Some(prev), false)
            }
            Err(idx) => {
                let at_right_edge = idx == entries.len();
                entries.insert(
                    idx,
                    LeafEntry {
                        key: key.to_vec(),
                        value: value.to_vec(),
                    },
                );
                (None, at_right_edge)
            }
        };

    let lsn = store.writer_tx_id();
    let is_root = path.is_empty();

    // Fast path: updated leaf fits in a single page.
    if let Some(raw) = build_leaf_page(page_size, leaf_pid, kind, is_root, lsn, &entries)? {
        store.write_page(leaf_pid, &raw)?;
        return Ok((root, previous));
    }

    // Leaf split: split entries cleanly into two non-empty halves that each fit in one page.
    let split_idx = choose_leaf_split_index(
        &entries,
        max_page_usable_bytes(page_size),
        inserted_at_right_edge,
    )?;
    let right_entries = entries.split_off(split_idx);
    let left_entries = entries;
    let promoted_key = right_entries[0].key.clone();

    let right_pid = store.allocate_page()?;
    let left_raw = build_leaf_page(page_size, leaf_pid, kind, false, lsn, &left_entries)?
        .ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Internal,
                MODULE_ID,
                "btree::btree_insert",
                "left leaf half unexpectedly exceeded page capacity after split",
            )
        })?;
    let right_raw = build_leaf_page(page_size, right_pid, kind, false, lsn, &right_entries)?
        .ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Internal,
                MODULE_ID,
                "btree::btree_insert",
                "right leaf half unexpectedly exceeded page capacity after split",
            )
        })?;

    store.write_page(leaf_pid, &left_raw)?;
    store.write_page(right_pid, &right_raw)?;

    // Propagate `(promoted_key, right_pid)` upward through ancestor internal nodes.
    let mut sep_key = promoted_key;
    let mut left_child = leaf_pid;
    let mut right_child = right_pid;

    while let Some(frame) = path.pop() {
        let parent_bytes = store.read_page(frame.page_id)?;
        let mut node = read_internal_node(&parent_bytes, frame.page_id)?;

        // Insert `(sep_key, right_child)` right after `frame.child_slot_idx`.
        // `frame.child_slot_idx == 0` -> insert at separator index 0.
        // `frame.child_slot_idx == k` -> insert at separator index k.
        let insert_pos = frame.child_slot_idx;
        node.separators.insert(insert_pos, (sep_key, right_child));

        let parent_is_root = path.is_empty();
        if let Some(raw) =
            build_internal_page(page_size, frame.page_id, kind, parent_is_root, lsn, &node)?
        {
            store.write_page(frame.page_id, &raw)?;
            return Ok((root, previous));
        }

        // Split internal node: median separator is promoted upward (not duplicated in children).
        let mid = node.separators.len() / 2;
        let mut right_seps = node.separators.split_off(mid);
        let (mid_key, mid_right_child) = right_seps.remove(0);

        let right_node = InternalNode {
            leftmost_child: mid_right_child,
            separators: right_seps,
        };
        let new_right_pid = store.allocate_page()?;

        let left_raw = build_internal_page(page_size, frame.page_id, kind, false, lsn, &node)?
            .ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::Internal,
                    MODULE_ID,
                    "btree::btree_insert",
                    "left internal half unexpectedly exceeded page capacity",
                )
            })?;
        let right_raw =
            build_internal_page(page_size, new_right_pid, kind, false, lsn, &right_node)?
                .ok_or_else(|| {
                    ErrorEnvelope::new(
                        ErrorCategory::Internal,
                        MODULE_ID,
                        "btree::btree_insert",
                        "right internal half unexpectedly exceeded page capacity",
                    )
                })?;

        store.write_page(frame.page_id, &left_raw)?;
        store.write_page(new_right_pid, &right_raw)?;

        sep_key = mid_key;
        left_child = frame.page_id;
        right_child = new_right_pid;
    }

    // Root split: allocate a new root internal page pointing to `(left_child, sep_key, right_child)`.
    let new_root_pid = store.allocate_page()?;
    let new_root_node = InternalNode {
        leftmost_child: left_child,
        separators: vec![(sep_key, right_child)],
    };
    let root_raw = build_internal_page(page_size, new_root_pid, kind, true, lsn, &new_root_node)?
        .ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::Internal,
            MODULE_ID,
            "btree::btree_insert",
            "new root internal page failed to fit single separator",
        )
    })?;
    store.write_page(new_root_pid, &root_raw)?;

    Ok((new_root_pid, previous))
}

fn choose_leaf_split_index(
    entries: &[LeafEntry],
    max_bytes: usize,
    inserted_at_right_edge: bool,
) -> Result<usize> {
    if entries.len() < 2 {
        return Err(ErrorEnvelope::new(
            ErrorCategory::Internal,
            MODULE_ID,
            "btree::choose_leaf_split_index",
            "cannot split leaf with fewer than 2 entries",
        ));
    }
    // Right-edge append optimization: on strictly increasing / sequential inserts, keep the
    // left leaf ~95-99% full (like SQLite) and move only the new rightmost entry into the new
    // right leaf, avoiding 50% page fill bloat and halving sequential page splits.
    if inserted_at_right_edge {
        let candidate = entries.len() - 1;
        if leaf_serialized_bytes(&entries[..candidate]) <= max_bytes
            && leaf_serialized_bytes(&entries[candidate..]) <= max_bytes
        {
            return Ok(candidate);
        }
    }
    let total = leaf_serialized_bytes(entries);
    let target = total / 2;
    let mut acc = 0usize;
    let mut best_idx = 1usize;

    for (i, e) in entries.iter().enumerate() {
        let entry_size = 16 + 2 + e.key.len() + 4 + e.value.len();
        if i > 0 && acc + entry_size > max_bytes {
            break;
        }
        acc += entry_size;
        if i + 1 < entries.len() {
            best_idx = i + 1;
            if acc >= target {
                break;
            }
        }
    }

    // Verify both sides fit; if right side overflows, advance split_idx while left still fits.
    while best_idx + 1 < entries.len() && leaf_serialized_bytes(&entries[best_idx..]) > max_bytes {
        best_idx += 1;
    }
    Ok(best_idx)
}
