//! Stack-based forward and reverse B+ tree range cursor (no on-disk sibling links,
//! matching `docs/file-format.md` §4.1).

use std::collections::HashSet;

use oryq_types::{ErrorCategory, ErrorEnvelope, PageId, Result};

use crate::btree::layout::{
    read_internal_node, read_leaf_entries, InternalNode, LeafEntry, PageStore, TreeKind,
    MAX_BTREE_HEIGHT,
};
use crate::btree::search::choose_child;
use crate::format::SlottedPageView;

const MODULE_ID: &str = "engine";

#[derive(Debug, Clone)]
struct CursorFrame {
    node: InternalNode,
    /// Child index currently descended (`0` = leftmost, `1..=separators.len()` = right child).
    child_idx: usize,
}

impl CursorFrame {
    fn child_count(&self) -> usize {
        1 + self.node.separators.len()
    }

    fn child_pid(&self, idx: usize) -> PageId {
        if idx == 0 {
            self.node.leftmost_child
        } else {
            self.node.separators[idx - 1].1
        }
    }
}

/// Forward and reverse B+ tree cursor retaining an ancestor traversal stack.
#[derive(Debug)]
pub struct BTreeCursor<'a, S: PageStore> {
    store: &'a S,
    root: PageId,
    stack: Vec<CursorFrame>,
    leaf_entries: Vec<LeafEntry>,
    entry_idx: usize,
    valid: bool,
}

impl<'a, S: PageStore> BTreeCursor<'a, S> {
    pub fn new(store: &'a S, root: PageId) -> Self {
        Self {
            store,
            root,
            stack: Vec::new(),
            leaf_entries: Vec::new(),
            entry_idx: 0,
            valid: false,
        }
    }

    /// Positions the cursor at the smallest key in the tree.
    pub fn seek_first(&mut self) -> Result<bool> {
        self.stack.clear();
        self.descend_leftmost(self.root)?;
        if !self.leaf_entries.is_empty() {
            self.entry_idx = 0;
            self.valid = true;
            return Ok(true);
        }
        self.advance_to_next_nonempty_leaf()
    }

    /// Positions the cursor at the largest key in the tree.
    pub fn seek_last(&mut self) -> Result<bool> {
        self.stack.clear();
        self.descend_rightmost(self.root)?;
        if !self.leaf_entries.is_empty() {
            self.entry_idx = self.leaf_entries.len() - 1;
            self.valid = true;
            return Ok(true);
        }
        self.retreat_to_prev_nonempty_leaf()
    }

    /// Positions the cursor at the first key `>= target`.
    pub fn seek_ge(&mut self, target: &[u8]) -> Result<bool> {
        self.stack.clear();
        let mut visited = HashSet::new();
        let mut current = self.root;

        loop {
            if self.stack.len() >= MAX_BTREE_HEIGHT {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "BTreeCursor::seek_ge",
                    format!("B+ tree height exceeded maximum {MAX_BTREE_HEIGHT}"),
                ));
            }
            if !visited.insert(current) {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "BTreeCursor::seek_ge",
                    format!("cycle detected at page {}", current.raw()),
                ));
            }

            let page_bytes = self.store.read_page(current)?;
            let view = SlottedPageView::parse(&page_bytes, current)?;
            let ptype = view.header().page_type;

            if TreeKind::is_leaf(ptype) {
                self.leaf_entries = read_leaf_entries(&page_bytes, current)?;
                let pos = self
                    .leaf_entries
                    .partition_point(|e| e.key.as_slice() < target);
                if pos < self.leaf_entries.len() {
                    self.entry_idx = pos;
                    self.valid = true;
                    return Ok(true);
                }
                return self.advance_to_next_nonempty_leaf();
            }

            let node = read_internal_node(&page_bytes, current)?;
            let (child_idx, next_pid) = choose_child(&view, target)?;
            self.stack.push(CursorFrame { node, child_idx });
            current = next_pid;
        }
    }

    /// Returns the current `(key, value)` slice if the cursor is positioned on a valid entry.
    pub fn current(&self) -> Option<(&[u8], &[u8])> {
        if !self.valid {
            return None;
        }
        let e = self.leaf_entries.get(self.entry_idx)?;
        Some((e.key.as_slice(), e.value.as_slice()))
    }

    /// Advances the cursor forward by one entry, returning `true` if still positioned on an entry.
    pub fn step_next(&mut self) -> Result<bool> {
        if !self.valid {
            return Ok(false);
        }
        if self.entry_idx + 1 < self.leaf_entries.len() {
            self.entry_idx += 1;
            return Ok(true);
        }
        self.advance_to_next_nonempty_leaf()
    }

    /// Moves the cursor backward by one entry, returning `true` if still positioned on an entry.
    pub fn step_prev(&mut self) -> Result<bool> {
        if !self.valid {
            return Ok(false);
        }
        if self.entry_idx > 0 {
            self.entry_idx -= 1;
            return Ok(true);
        }
        self.retreat_to_prev_nonempty_leaf()
    }

    fn descend_leftmost(&mut self, start: PageId) -> Result<()> {
        let mut current = start;
        let mut visited = HashSet::new();
        loop {
            if self.stack.len() >= MAX_BTREE_HEIGHT || !visited.insert(current) {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "BTreeCursor::descend_leftmost",
                    "cycle or excessive height in B+ tree",
                ));
            }
            let page_bytes = self.store.read_page(current)?;
            let view = SlottedPageView::parse(&page_bytes, current)?;
            if TreeKind::is_leaf(view.header().page_type) {
                self.leaf_entries = read_leaf_entries(&page_bytes, current)?;
                return Ok(());
            }
            let node = read_internal_node(&page_bytes, current)?;
            let next_pid = node.leftmost_child;
            self.stack.push(CursorFrame { node, child_idx: 0 });
            current = next_pid;
        }
    }

    fn descend_rightmost(&mut self, start: PageId) -> Result<()> {
        let mut current = start;
        let mut visited = HashSet::new();
        loop {
            if self.stack.len() >= MAX_BTREE_HEIGHT || !visited.insert(current) {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "BTreeCursor::descend_rightmost",
                    "cycle or excessive height in B+ tree",
                ));
            }
            let page_bytes = self.store.read_page(current)?;
            let view = SlottedPageView::parse(&page_bytes, current)?;
            if TreeKind::is_leaf(view.header().page_type) {
                self.leaf_entries = read_leaf_entries(&page_bytes, current)?;
                return Ok(());
            }
            let node = read_internal_node(&page_bytes, current)?;
            let last_idx = node.separators.len();
            let next_pid = if last_idx == 0 {
                node.leftmost_child
            } else {
                node.separators[last_idx - 1].1
            };
            self.stack.push(CursorFrame {
                node,
                child_idx: last_idx,
            });
            current = next_pid;
        }
    }

    fn advance_to_next_nonempty_leaf(&mut self) -> Result<bool> {
        loop {
            while let Some(top) = self.stack.last_mut() {
                if top.child_idx + 1 < top.child_count() {
                    top.child_idx += 1;
                    let next_child = top.child_pid(top.child_idx);
                    self.descend_leftmost(next_child)?;
                    if !self.leaf_entries.is_empty() {
                        self.entry_idx = 0;
                        self.valid = true;
                        return Ok(true);
                    }
                    break;
                }
                self.stack.pop();
            }
            if self.stack.is_empty() {
                self.valid = false;
                self.leaf_entries.clear();
                return Ok(false);
            }
        }
    }

    fn retreat_to_prev_nonempty_leaf(&mut self) -> Result<bool> {
        loop {
            while let Some(top) = self.stack.last_mut() {
                if top.child_idx > 0 {
                    top.child_idx -= 1;
                    let prev_child = top.child_pid(top.child_idx);
                    self.descend_rightmost(prev_child)?;
                    if !self.leaf_entries.is_empty() {
                        self.entry_idx = self.leaf_entries.len() - 1;
                        self.valid = true;
                        return Ok(true);
                    }
                    break;
                }
                self.stack.pop();
            }
            if self.stack.is_empty() {
                self.valid = false;
                self.leaf_entries.clear();
                return Ok(false);
            }
        }
    }
}
