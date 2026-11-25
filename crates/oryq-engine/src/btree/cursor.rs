//! Zero-copy stack-based forward and reverse B+ tree range cursor (no on-disk sibling links,
//! matching `docs/file-format.md` §4.1).

use std::sync::Arc;

use oryq_types::{ErrorCategory, ErrorEnvelope, PageId, Result};

use crate::btree::layout::{
    decode_internal_slot, decode_leaf_entry, PageStore, TreeKind, MAX_BTREE_HEIGHT,
};
use crate::btree::search::choose_child;
use crate::format::SlottedPageView;

const MODULE_ID: &str = "engine";

#[derive(Debug, Clone)]
struct CursorFrame {
    page_id: PageId,
    page_bytes: Arc<[u8]>,
    slot_count: usize,
    /// Child slot index currently descended (`0` = leftmost sentinel, `1..slot_count` = right child).
    child_idx: usize,
}

impl CursorFrame {
    #[inline]
    fn child_count(&self) -> usize {
        self.slot_count
    }

    fn child_pid(&self, idx: usize) -> Result<PageId> {
        let view = SlottedPageView::parse_verified(&self.page_bytes, self.page_id)?;
        let payload = view.payload(idx as u16).ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "BTreeCursor::child_pid",
                format!("missing internal slot {idx} on page {}", self.page_id.raw()),
            )
        })?;
        let (_, pid) = decode_internal_slot(payload)?;
        Ok(pid)
    }
}

#[derive(Debug)]
struct ActiveLeaf {
    page_id: PageId,
    page_bytes: Arc<[u8]>,
    slot_count: usize,
}

/// Forward and reverse B+ tree cursor retaining pinned `Arc<[u8]>` pages for zero-copy traversal.
#[derive(Debug)]
pub struct BTreeCursor<'a, S: PageStore> {
    store: &'a S,
    root: PageId,
    stack: Vec<CursorFrame>,
    leaf: Option<ActiveLeaf>,
    entry_idx: usize,
    valid: bool,
}

impl<'a, S: PageStore> BTreeCursor<'a, S> {
    pub fn new(store: &'a S, root: PageId) -> Self {
        Self {
            store,
            root,
            stack: Vec::with_capacity(4),
            leaf: None,
            entry_idx: 0,
            valid: false,
        }
    }

    /// Resets cursor position state before starting a fresh seek or on corruption.
    #[inline]
    fn reset_state(&mut self) {
        self.stack.clear();
        self.leaf = None;
        self.entry_idx = 0;
        self.valid = false;
    }

    /// Validates that the currently landed leaf slot is a live, well-formed B+ tree entry.
    /// Fails closed with `ErrorCategory::Corruption` and invalidates the cursor on corrupt data.
    fn validate_landed_entry(&mut self, op: &'static str) -> Result<()> {
        let Some(leaf) = &self.leaf else {
            self.valid = false;
            return Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                op,
                "cursor has no active leaf page",
            ));
        };
        let res = (|| -> Result<()> {
            let view = SlottedPageView::parse_verified(&leaf.page_bytes, leaf.page_id)?;
            let payload = view.payload(self.entry_idx as u16).ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    op,
                    format!(
                        "unexpected tombstone or out-of-bounds leaf slot {} on page {}",
                        self.entry_idx,
                        leaf.page_id.raw()
                    ),
                )
            })?;
            let _ = decode_leaf_entry(payload)?;
            Ok(())
        })();
        if res.is_err() {
            self.reset_state();
        }
        res
    }

    /// Positions the cursor at the smallest key in the tree.
    pub fn seek_first(&mut self) -> Result<bool> {
        self.reset_state();
        self.descend_leftmost(self.root)?;
        if let Some(leaf) = &self.leaf {
            if leaf.slot_count > 0 {
                self.entry_idx = 0;
                self.validate_landed_entry("BTreeCursor::seek_first")?;
                self.valid = true;
                return Ok(true);
            }
        }
        self.advance_to_next_nonempty_leaf()
    }

    /// Positions the cursor at the largest key in the tree.
    pub fn seek_last(&mut self) -> Result<bool> {
        self.reset_state();
        self.descend_rightmost(self.root)?;
        if let Some(leaf) = &self.leaf {
            if leaf.slot_count > 0 {
                self.entry_idx = leaf.slot_count - 1;
                self.validate_landed_entry("BTreeCursor::seek_last")?;
                self.valid = true;
                return Ok(true);
            }
        }
        self.retreat_to_prev_nonempty_leaf()
    }

    /// Positions the cursor at the first key `>= target`.
    pub fn seek_ge(&mut self, target: &[u8]) -> Result<bool> {
        self.reset_state();
        let mut current = self.root;

        loop {
            if self.stack.len() >= MAX_BTREE_HEIGHT
                || self.stack.iter().any(|f| f.page_id == current)
            {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "BTreeCursor::seek_ge",
                    format!(
                        "cycle or excessive height in B+ tree at page {}",
                        current.raw()
                    ),
                ));
            }

            let page_bytes = self.store.read_page(current)?;
            let view = SlottedPageView::parse_verified(&page_bytes, current)?;
            let ptype = view.header().page_type;
            let slot_count = usize::from(view.slot_count());

            if TreeKind::is_leaf(ptype) {
                // Binary search directly over leaf slots without allocating `Vec<LeafEntry>`.
                let mut lo = 0usize;
                let mut hi = slot_count;
                while lo < hi {
                    let mid = lo + (hi - lo) / 2;
                    let payload = view.payload(mid as u16).ok_or_else(|| {
                        ErrorEnvelope::new(
                            ErrorCategory::Corruption,
                            MODULE_ID,
                            "BTreeCursor::seek_ge",
                            format!("unexpected tombstone in leaf slot {mid}"),
                        )
                    })?;
                    let (k, _) = decode_leaf_entry(payload)?;
                    if k < target {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                self.leaf = Some(ActiveLeaf {
                    page_id: current,
                    page_bytes,
                    slot_count,
                });
                if lo < slot_count {
                    self.entry_idx = lo;
                    self.valid = true;
                    return Ok(true);
                }
                return self.advance_to_next_nonempty_leaf();
            }

            if !TreeKind::is_internal(ptype) || slot_count == 0 {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "BTreeCursor::seek_ge",
                    format!("invalid internal page {} during seek_ge", current.raw()),
                ));
            }

            let (child_idx, next_pid) = choose_child(&view, target)?;
            self.stack.push(CursorFrame {
                page_id: current,
                page_bytes,
                slot_count,
                child_idx,
            });
            current = next_pid;
        }
    }

    /// Returns the current `(key, value)` zero-copy slice if the cursor is positioned on a valid entry.
    pub fn current(&self) -> Option<(&[u8], &[u8])> {
        if !self.valid {
            return None;
        }
        let leaf = self.leaf.as_ref()?;
        if self.entry_idx >= leaf.slot_count {
            return None;
        }
        let view = SlottedPageView::parse_verified(&leaf.page_bytes, leaf.page_id).ok()?;
        let payload = view.payload(self.entry_idx as u16)?;
        decode_leaf_entry(payload).ok()
    }

    /// Advances the cursor forward by one entry, returning `true` if still positioned on an entry.
    pub fn step_next(&mut self) -> Result<bool> {
        if !self.valid {
            return Ok(false);
        }
        if let Some(leaf) = &self.leaf {
            if self.entry_idx + 1 < leaf.slot_count {
                self.entry_idx += 1;
                self.validate_landed_entry("BTreeCursor::step_next")?;
                return Ok(true);
            }
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
            self.validate_landed_entry("BTreeCursor::step_prev")?;
            return Ok(true);
        }
        self.retreat_to_prev_nonempty_leaf()
    }

    fn descend_leftmost(&mut self, start: PageId) -> Result<()> {
        let mut current = start;
        loop {
            if self.stack.len() >= MAX_BTREE_HEIGHT
                || self.stack.iter().any(|f| f.page_id == current)
            {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "BTreeCursor::descend_leftmost",
                    "cycle or excessive height in B+ tree",
                ));
            }
            let page_bytes = self.store.read_page(current)?;
            let view = SlottedPageView::parse_verified(&page_bytes, current)?;
            let ptype = view.header().page_type;
            let slot_count = usize::from(view.slot_count());

            if TreeKind::is_leaf(ptype) {
                self.leaf = Some(ActiveLeaf {
                    page_id: current,
                    page_bytes,
                    slot_count,
                });
                return Ok(());
            }
            if !TreeKind::is_internal(ptype) || slot_count == 0 {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "BTreeCursor::descend_leftmost",
                    format!("invalid internal page {}", current.raw()),
                ));
            }
            let payload = view.payload(0).ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "BTreeCursor::descend_leftmost",
                    "internal sentinel slot 0 missing",
                )
            })?;
            let (_, next_pid) = decode_internal_slot(payload)?;
            self.stack.push(CursorFrame {
                page_id: current,
                page_bytes,
                slot_count,
                child_idx: 0,
            });
            current = next_pid;
        }
    }

    fn descend_rightmost(&mut self, start: PageId) -> Result<()> {
        let mut current = start;
        loop {
            if self.stack.len() >= MAX_BTREE_HEIGHT
                || self.stack.iter().any(|f| f.page_id == current)
            {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "BTreeCursor::descend_rightmost",
                    "cycle or excessive height in B+ tree",
                ));
            }
            let page_bytes = self.store.read_page(current)?;
            let view = SlottedPageView::parse_verified(&page_bytes, current)?;
            let ptype = view.header().page_type;
            let slot_count = usize::from(view.slot_count());

            if TreeKind::is_leaf(ptype) {
                self.leaf = Some(ActiveLeaf {
                    page_id: current,
                    page_bytes,
                    slot_count,
                });
                return Ok(());
            }
            if !TreeKind::is_internal(ptype) || slot_count == 0 {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "BTreeCursor::descend_rightmost",
                    format!("invalid internal page {}", current.raw()),
                ));
            }
            let last_idx = slot_count - 1;
            let payload = view.payload(last_idx as u16).ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "BTreeCursor::descend_rightmost",
                    format!("missing internal slot {last_idx}"),
                )
            })?;
            let (_, next_pid) = decode_internal_slot(payload)?;
            self.stack.push(CursorFrame {
                page_id: current,
                page_bytes,
                slot_count,
                child_idx: last_idx,
            });
            current = next_pid;
        }
    }

    fn advance_to_next_nonempty_leaf(&mut self) -> Result<bool> {
        self.valid = false;
        self.leaf = None;
        loop {
            while let Some(top) = self.stack.last_mut() {
                if top.child_idx + 1 < top.child_count() {
                    top.child_idx += 1;
                    let next_child = top.child_pid(top.child_idx)?;
                    self.descend_leftmost(next_child)?;
                    if self.leaf.as_ref().is_some_and(|l| l.slot_count > 0) {
                        self.entry_idx = 0;
                        self.validate_landed_entry("BTreeCursor::advance_to_next_nonempty_leaf")?;
                        self.valid = true;
                        return Ok(true);
                    }
                    break;
                }
                self.stack.pop();
            }
            if self.stack.is_empty() {
                self.valid = false;
                self.leaf = None;
                return Ok(false);
            }
        }
    }

    fn retreat_to_prev_nonempty_leaf(&mut self) -> Result<bool> {
        self.valid = false;
        self.leaf = None;
        loop {
            while let Some(top) = self.stack.last_mut() {
                if top.child_idx > 0 {
                    top.child_idx -= 1;
                    let prev_child = top.child_pid(top.child_idx)?;
                    self.descend_rightmost(prev_child)?;
                    if let Some(leaf) = &self.leaf {
                        if leaf.slot_count > 0 {
                            self.entry_idx = leaf.slot_count - 1;
                            self.validate_landed_entry(
                                "BTreeCursor::retreat_to_prev_nonempty_leaf",
                            )?;
                            self.valid = true;
                            return Ok(true);
                        }
                    }
                    break;
                }
                self.stack.pop();
            }
            if self.stack.is_empty() {
                self.valid = false;
                self.leaf = None;
                return Ok(false);
            }
        }
    }
}
