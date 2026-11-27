//! Logical-page byte-key/value B+ tree (`docs/file-format.md` §4.1).
//!
//! Operates strictly on opaque byte slices (`&[u8]`) with zero document, index,
//! or query-layer interpretation.

pub mod cursor;
pub mod delete;
pub mod insert;
pub mod layout;
pub mod search;

use oryq_types::{PageId, Result};

pub use cursor::BTreeCursor;
pub use delete::btree_delete;
pub use insert::btree_insert;
pub use layout::{
    build_internal_page, build_leaf_page, decode_internal_slot, decode_leaf_entry,
    encode_internal_slot, encode_leaf_entry, read_internal_node, read_leaf_entries, InternalNode,
    LeafEntry, PageStore, PageStoreMut, PagerPageStore, TreeKind, MAX_BTREE_HEIGHT,
};
pub use search::{btree_get, descend_to_leaf, DescentFrame};

/// High-level byte-key/value B+ tree operations over any [`PageStore`] / [`PageStoreMut`].
#[derive(Debug, Clone, Copy)]
pub struct BTree {
    root: PageId,
    kind: TreeKind,
}

impl BTree {
    pub fn open(root: PageId, kind: TreeKind) -> Self {
        Self { root, kind }
    }

    /// Allocates and initializes an empty root leaf page for a new B+ tree.
    pub fn create_empty(store: &mut impl PageStoreMut, kind: TreeKind) -> Result<Self> {
        let root = store.allocate_page()?;
        let page_size = store.page_size();
        let lsn = store.writer_tx_id();
        let raw = build_leaf_page(page_size, root, kind, true, lsn, &[])?
            .expect("empty leaf always fits");
        store.write_page(root, &raw)?;
        Ok(Self { root, kind })
    }

    #[inline]
    pub fn root(&self) -> PageId {
        self.root
    }

    #[inline]
    pub fn kind(&self) -> TreeKind {
        self.kind
    }

    pub fn get(&self, store: &impl PageStore, key: &[u8]) -> Result<Option<Vec<u8>>> {
        btree_get(store, self.root, key)
    }

    pub fn insert(
        &mut self,
        store: &mut impl PageStoreMut,
        key: &[u8],
        value: &[u8],
    ) -> Result<Option<Vec<u8>>> {
        let (new_root, prev) = btree_insert(store, self.root, self.kind, key, value)?;
        self.root = new_root;
        Ok(prev)
    }

    pub fn delete(&mut self, store: &mut impl PageStoreMut, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let (new_root, removed) = btree_delete(store, self.root, self.kind, key)?;
        self.root = new_root;
        Ok(removed)
    }

    pub fn cursor<'a, S: PageStore>(&self, store: &'a S) -> BTreeCursor<'a, S> {
        BTreeCursor::new(store, self.root)
    }

    /// Collects all `(key, value)` pairs in ascending key order.
    pub fn scan_all(&self, store: &impl PageStore) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let mut cursor = self.cursor(store);
        let mut out = Vec::new();
        if cursor.seek_first()? {
            while let Some((k, v)) = cursor.current() {
                out.push((k.to_vec(), v.to_vec()));
                if !cursor.step_next()? {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Collects all `(key, value)` pairs in `[start_inclusive, end_exclusive)`.
    pub fn scan_range(
        &self,
        store: &impl PageStore,
        start_inclusive: &[u8],
        end_exclusive: &[u8],
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let mut cursor = self.cursor(store);
        let mut out = Vec::new();
        if cursor.seek_ge(start_inclusive)? {
            while let Some((k, v)) = cursor.current() {
                if k >= end_exclusive {
                    break;
                }
                out.push((k.to_vec(), v.to_vec()));
                if !cursor.step_next()? {
                    break;
                }
            }
        }
        Ok(out)
    }

    /// Walks this B+ tree from its root and frees every reachable internal and leaf page.
    pub fn free_all_pages(&self, store: &mut impl PageStoreMut) -> Result<()> {
        let mut stack = vec![self.root];
        let mut visited = std::collections::HashSet::new();
        while let Some(pid) = stack.pop() {
            if !visited.insert(pid) {
                continue;
            }
            let bytes = store.read_page(pid)?;
            let view = crate::format::SlottedPageView::parse(&bytes, pid)?;
            if TreeKind::is_internal(view.header().page_type) {
                let node = read_internal_node(&bytes, pid)?;
                stack.push(node.leftmost_child);
                for (_, child) in node.separators {
                    stack.push(child);
                }
            }
            store.free_page(pid)?;
        }
        Ok(())
    }
}
