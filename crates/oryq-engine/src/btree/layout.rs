//! Byte-key/value B+ tree page layout, slot codecs, and `PageStore` storage adapters
//! (`docs/file-format.md` §4.1).

use std::sync::Arc;

use oryq_types::{ErrorCategory, ErrorEnvelope, PageId, Result, TxId};

use crate::cache::PageCache;
use crate::config::PageSize;
use crate::format::{
    PageFlags, PageHeader, PageType, SlotFlags, SlottedPageMut, SlottedPageView, PAGE_HEADER_SIZE,
    SLOT_ENTRY_SIZE,
};
use crate::pager::{PageAllocator, Pager};

const MODULE_ID: &str = "engine";

/// Maximum permitted B+ tree height to bound traversal against corrupt cycles.
pub const MAX_BTREE_HEIGHT: usize = 64;

/// Discriminates collection trees (`ColLeaf`/`ColInternal`) from index trees (`IdxLeaf`/`IdxInternal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TreeKind {
    Collection,
    Index,
}

impl TreeKind {
    #[inline]
    pub const fn leaf_page_type(self) -> PageType {
        match self {
            Self::Collection => PageType::ColLeaf,
            Self::Index => PageType::IdxLeaf,
        }
    }

    #[inline]
    pub const fn internal_page_type(self) -> PageType {
        match self {
            Self::Collection => PageType::ColInternal,
            Self::Index => PageType::IdxInternal,
        }
    }

    #[inline]
    pub const fn is_leaf(page_type: PageType) -> bool {
        matches!(page_type, PageType::ColLeaf | PageType::IdxLeaf)
    }

    #[inline]
    pub const fn is_internal(page_type: PageType) -> bool {
        matches!(page_type, PageType::ColInternal | PageType::IdxInternal)
    }
}

/// Read-only page access abstraction used by B+ tree search and cursors.
pub trait PageStore {
    fn page_size(&self) -> PageSize;
    fn read_page(&self, page_id: PageId) -> Result<Arc<[u8]>>;
}

/// Mutable page access abstraction used by B+ tree insert/delete/split/merge.
pub trait PageStoreMut: PageStore {
    fn writer_tx_id(&self) -> TxId;
    fn write_page(&mut self, page_id: PageId, bytes: &[u8]) -> Result<()>;
    fn allocate_page(&mut self) -> Result<PageId>;
    fn free_page(&mut self, page_id: PageId) -> Result<()>;
}

/// Direct [`PageStore`] / [`PageStoreMut`] adapter backed by a [`Pager`] and optional [`PageCache`].
#[derive(Debug)]
pub struct PagerPageStore<'a> {
    pager: &'a mut Pager,
    cache: Option<&'a PageCache>,
    min_active_snapshot_tx_id: TxId,
    writer_tx_id: TxId,
}

impl<'a> PagerPageStore<'a> {
    pub fn new(
        pager: &'a mut Pager,
        cache: Option<&'a PageCache>,
        min_active_snapshot_tx_id: TxId,
        writer_tx_id: TxId,
    ) -> Self {
        Self {
            pager,
            cache,
            min_active_snapshot_tx_id,
            writer_tx_id,
        }
    }

    pub fn pager(&self) -> &Pager {
        self.pager
    }

    pub fn pager_mut(&mut self) -> &mut Pager {
        self.pager
    }
}

impl PageStore for PagerPageStore<'_> {
    fn page_size(&self) -> PageSize {
        self.pager.page_size()
    }

    fn read_page(&self, page_id: PageId) -> Result<Arc<[u8]>> {
        let guard = self.pager.read_page(page_id, None)?;
        let hdr = PageHeader::decode_and_verify(guard.as_slice(), page_id)?;
        if TreeKind::is_leaf(hdr.page_type) || TreeKind::is_internal(hdr.page_type) {
            let _ = SlottedPageView::parse_with_header(guard.as_slice(), hdr)?;
        }
        if let Some(cache) = self.cache {
            if let Some(hit) = cache.get(page_id, hdr.lsn_tx_id) {
                return Ok(hit.as_arc());
            }
            let arc: Arc<[u8]> = Arc::from(guard.as_slice());
            let _ = cache.insert(page_id, hdr.lsn_tx_id, Arc::clone(&arc));
            return Ok(arc);
        }
        Ok(Arc::from(guard.as_slice()))
    }
}

impl PageStoreMut for PagerPageStore<'_> {
    fn writer_tx_id(&self) -> TxId {
        self.writer_tx_id
    }

    fn write_page(&mut self, page_id: PageId, bytes: &[u8]) -> Result<()> {
        self.pager.write_page(page_id, bytes)?;
        if let Some(cache) = self.cache {
            let hdr = PageHeader::decode_and_verify(bytes, page_id)?;
            let arc: Arc<[u8]> = Arc::from(bytes);
            let _ = cache.put_replace(page_id, hdr.lsn_tx_id, arc);
        }
        Ok(())
    }

    fn allocate_page(&mut self) -> Result<PageId> {
        PageAllocator::allocate_page(
            self.pager,
            self.min_active_snapshot_tx_id,
            self.writer_tx_id,
        )
    }

    fn free_page(&mut self, page_id: PageId) -> Result<()> {
        PageAllocator::free_page(self.pager, page_id, self.writer_tx_id)
    }
}

/// Decoded leaf key-value pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeafEntry {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

/// Decoded internal node representation:
/// `leftmost_child` (slot 0 sentinel) followed by strictly increasing `(separator, right_child)` pairs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternalNode {
    pub leftmost_child: PageId,
    pub separators: Vec<(Vec<u8>, PageId)>,
}

/// Encodes a leaf entry `[key_len: u16 LE][key_bytes][val_len: u32 LE][val_bytes]`.
pub fn encode_leaf_entry(key: &[u8], value: &[u8]) -> Result<Vec<u8>> {
    let key_len = u16::try_from(key.len()).map_err(|_| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "btree::encode_leaf_entry",
            "B+ tree key exceeds u16::MAX bytes",
        )
    })?;
    let val_len = u32::try_from(value.len()).map_err(|_| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "btree::encode_leaf_entry",
            "B+ tree value exceeds u32::MAX bytes",
        )
    })?;
    let mut buf = Vec::with_capacity(2 + key.len() + 4 + value.len());
    buf.extend_from_slice(&key_len.to_le_bytes());
    buf.extend_from_slice(key);
    buf.extend_from_slice(&val_len.to_le_bytes());
    buf.extend_from_slice(value);
    Ok(buf)
}

/// Decodes a leaf entry slice into `(key, value)` borrows.
pub fn decode_leaf_entry(payload: &[u8]) -> Result<(&[u8], &[u8])> {
    if payload.len() < 6 {
        return Err(corrupt(
            "btree::decode_leaf_entry",
            "leaf slot payload shorter than 6-byte envelope",
        ));
    }
    let key_len = u16::from_le_bytes([payload[0], payload[1]]) as usize;
    let val_len_off = 2 + key_len;
    if val_len_off + 4 > payload.len() {
        return Err(corrupt(
            "btree::decode_leaf_entry",
            "leaf slot key_len exceeds payload bounds",
        ));
    }
    let key = &payload[2..val_len_off];
    let val_len =
        u32::from_le_bytes(payload[val_len_off..val_len_off + 4].try_into().unwrap()) as usize;
    let val_start = val_len_off + 4;
    if val_start + val_len != payload.len() {
        return Err(corrupt(
            "btree::decode_leaf_entry",
            "leaf slot val_len does not match payload bounds",
        ));
    }
    Ok((key, &payload[val_start..val_start + val_len]))
}

/// Encodes an internal slot `[separator_len: u16 LE][separator_bytes][right_child: u64 LE]`.
pub fn encode_internal_slot(separator: &[u8], child: PageId) -> Result<Vec<u8>> {
    let sep_len = u16::try_from(separator.len()).map_err(|_| {
        ErrorEnvelope::new(
            ErrorCategory::InvalidArgument,
            MODULE_ID,
            "btree::encode_internal_slot",
            "internal separator key exceeds u16::MAX bytes",
        )
    })?;
    let mut buf = Vec::with_capacity(2 + separator.len() + 8);
    buf.extend_from_slice(&sep_len.to_le_bytes());
    buf.extend_from_slice(separator);
    buf.extend_from_slice(&child.raw().to_le_bytes());
    Ok(buf)
}

/// Decodes an internal slot payload into `(separator, child_page_id)`.
pub fn decode_internal_slot(payload: &[u8]) -> Result<(&[u8], PageId)> {
    if payload.len() < 10 {
        return Err(corrupt(
            "btree::decode_internal_slot",
            "internal slot payload shorter than 10 bytes",
        ));
    }
    let sep_len = u16::from_le_bytes([payload[0], payload[1]]) as usize;
    if 2 + sep_len + 8 != payload.len() {
        return Err(corrupt(
            "btree::decode_internal_slot",
            "internal slot separator_len does not match payload bounds",
        ));
    }
    let child_raw = u64::from_le_bytes(payload[2 + sep_len..2 + sep_len + 8].try_into().unwrap());
    if child_raw == 0 {
        return Err(corrupt(
            "btree::decode_internal_slot",
            "internal child PageId cannot be Page 0",
        ));
    }
    Ok((&payload[2..2 + sep_len], PageId::new(child_raw)))
}

/// Reads all live sorted `(key, value)` entries from a validated leaf page.
pub fn read_leaf_entries(page: &[u8], expected_page_id: PageId) -> Result<Vec<LeafEntry>> {
    let view = SlottedPageView::parse_verified(page, expected_page_id)?;
    if !TreeKind::is_leaf(view.header().page_type) {
        return Err(corrupt(
            "btree::read_leaf_entries",
            format!("expected leaf page, got {:?}", view.header().page_type),
        ));
    }
    let mut entries: Vec<LeafEntry> = Vec::with_capacity(usize::from(view.slot_count()));
    for idx in 0..view.slot_count() {
        if let Some(payload) = view.payload(idx) {
            let (k, v) = decode_leaf_entry(payload)?;
            if let Some(prev) = entries.last() {
                if k <= prev.key.as_slice() {
                    return Err(corrupt(
                        "btree::read_leaf_entries",
                        "leaf entries are not strictly sorted by key",
                    ));
                }
            }
            entries.push(LeafEntry {
                key: k.to_vec(),
                value: v.to_vec(),
            });
        }
    }
    Ok(entries)
}

/// Reads an internal node from a validated internal page.
pub fn read_internal_node(page: &[u8], expected_page_id: PageId) -> Result<InternalNode> {
    let view = SlottedPageView::parse_verified(page, expected_page_id)?;
    if !TreeKind::is_internal(view.header().page_type) {
        return Err(corrupt(
            "btree::read_internal_node",
            format!("expected internal page, got {:?}", view.header().page_type),
        ));
    }
    if view.slot_count() == 0 {
        return Err(corrupt(
            "btree::read_internal_node",
            "internal page missing sentinel slot 0",
        ));
    }
    let sentinel_payload = view.payload(0).ok_or_else(|| {
        corrupt(
            "btree::read_internal_node",
            "internal sentinel slot 0 cannot be a tombstone",
        )
    })?;
    let (sentinel_sep, leftmost_child) = decode_internal_slot(sentinel_payload)?;
    if !sentinel_sep.is_empty() {
        return Err(corrupt(
            "btree::read_internal_node",
            "internal sentinel slot 0 must have empty separator",
        ));
    }

    let mut separators: Vec<(Vec<u8>, PageId)> =
        Vec::with_capacity(usize::from(view.slot_count().saturating_sub(1)));
    for idx in 1..view.slot_count() {
        if let Some(payload) = view.payload(idx) {
            let (sep, child) = decode_internal_slot(payload)?;
            if sep.is_empty() {
                return Err(corrupt(
                    "btree::read_internal_node",
                    "non-sentinel internal slot has empty separator",
                ));
            }
            if let Some((prev_sep, _)) = separators.last() {
                if sep <= prev_sep.as_slice() {
                    return Err(corrupt(
                        "btree::read_internal_node",
                        "internal separators are not strictly increasing",
                    ));
                }
            }
            separators.push((sep.to_vec(), child));
        }
    }

    Ok(InternalNode {
        leftmost_child,
        separators,
    })
}

/// Computes the serialized byte footprint (slot directory + payload) of a set of leaf entries.
pub fn leaf_serialized_bytes(entries: &[LeafEntry]) -> usize {
    entries
        .iter()
        .map(|e| SLOT_ENTRY_SIZE + 2 + e.key.len() + 4 + e.value.len())
        .sum()
}

/// Computes the serialized byte footprint (slot directory + payload) of an internal node.
pub fn internal_serialized_bytes(node: &InternalNode) -> usize {
    // Sentinel slot 0: SLOT_ENTRY_SIZE + 2 + 0 + 8
    let sentinel = SLOT_ENTRY_SIZE + 10;
    let seps: usize = node
        .separators
        .iter()
        .map(|(s, _)| SLOT_ENTRY_SIZE + 2 + s.len() + 8)
        .sum();
    sentinel + seps
}

/// Maximum usable payload+slot bytes inside a single page (`page_size - 32`).
#[inline]
pub const fn max_page_usable_bytes(page_size: PageSize) -> usize {
    page_size.bytes() - PAGE_HEADER_SIZE
}

/// Encodes `entries` into a fresh checksummed leaf page image.
///
/// Returns `Ok(None)` if `entries` do not fit into `page_size`.
pub fn build_leaf_page(
    page_size: PageSize,
    page_id: PageId,
    kind: TreeKind,
    is_root: bool,
    lsn_tx_id: TxId,
    entries: &[LeafEntry],
) -> Result<Option<Vec<u8>>> {
    if leaf_serialized_bytes(entries) > max_page_usable_bytes(page_size) {
        return Ok(None);
    }
    let mut raw = vec![0u8; page_size.bytes()];
    let flags = if is_root {
        PageFlags::EMPTY.with(PageFlags::IS_ROOT_PAGE)
    } else {
        PageFlags::EMPTY
    };
    let mut page =
        SlottedPageMut::init(&mut raw, page_id, kind.leaf_page_type(), flags, lsn_tx_id)?;
    for entry in entries {
        let encoded = encode_leaf_entry(&entry.key, &entry.value)?;
        if page
            .insert_slot(&encoded, SlotFlags::EMPTY.with(SlotFlags::BASE_DOC))
            .is_none()
        {
            return Ok(None);
        }
    }
    page.finalize();
    Ok(Some(raw))
}

/// Encodes `node` into a fresh checksummed internal page image.
///
/// Returns `Ok(None)` if `node` does not fit into `page_size`.
pub fn build_internal_page(
    page_size: PageSize,
    page_id: PageId,
    kind: TreeKind,
    is_root: bool,
    lsn_tx_id: TxId,
    node: &InternalNode,
) -> Result<Option<Vec<u8>>> {
    if internal_serialized_bytes(node) > max_page_usable_bytes(page_size) {
        return Ok(None);
    }
    let mut raw = vec![0u8; page_size.bytes()];
    let flags = if is_root {
        PageFlags::EMPTY.with(PageFlags::IS_ROOT_PAGE)
    } else {
        PageFlags::EMPTY
    };
    let mut page = SlottedPageMut::init(
        &mut raw,
        page_id,
        kind.internal_page_type(),
        flags,
        lsn_tx_id,
    )?;
    let sentinel = encode_internal_slot(&[], node.leftmost_child)?;
    if page.insert_slot(&sentinel, SlotFlags::EMPTY).is_none() {
        return Ok(None);
    }
    for (sep, child) in &node.separators {
        let encoded = encode_internal_slot(sep, *child)?;
        if page.insert_slot(&encoded, SlotFlags::EMPTY).is_none() {
            return Ok(None);
        }
    }
    page.finalize();
    Ok(Some(raw))
}

fn corrupt(op: &'static str, msg: impl Into<String>) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCategory::Corruption, MODULE_ID, op, msg)
}
