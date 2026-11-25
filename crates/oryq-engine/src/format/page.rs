//! Logical page header (32B), slotted page view/mutator (16B slots), overflow page/ref,
//! and freelist trunk layouts (`docs/file-format.md` §4.1).

use oryq_types::{ErrorCategory, ErrorEnvelope, PageId, Result, TxId};
use xxhash_rust::xxh3::Xxh3;

const MODULE_ID: &str = "engine";

pub const PAGE_HEADER_SIZE: usize = 32;
pub const SLOT_ENTRY_SIZE: usize = 16;
pub const OVERFLOW_HEADER_SIZE: usize = 48;
pub const OVERFLOW_REF_SIZE: usize = 16;
pub const FREELIST_HEADER_SIZE: usize = 48;
pub const FREELIST_ENTRY_SIZE: usize = 16;
pub const NO_DELTA_SLOT: u16 = 0xFFFF;

/// Logical page type tag (`docs/file-format.md` §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PageType {
    ColLeaf = 0x01,
    ColInternal = 0x02,
    IdxLeaf = 0x03,
    IdxInternal = 0x04,
    Overflow = 0x05,
    Freelist = 0x06,
    PluginPage = 0x07,
}

impl PageType {
    pub fn from_u8(raw: u8) -> Result<Self> {
        match raw {
            0x01 => Ok(Self::ColLeaf),
            0x02 => Ok(Self::ColInternal),
            0x03 => Ok(Self::IdxLeaf),
            0x04 => Ok(Self::IdxInternal),
            0x05 => Ok(Self::Overflow),
            0x06 => Ok(Self::Freelist),
            0x07 => Ok(Self::PluginPage),
            other => Err(corrupt(
                "PageType::from_u8",
                format!("unknown page_type byte 0x{other:02x}"),
            )),
        }
    }
}

/// Bitmask flags on a logical page header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PageFlags(u8);

impl PageFlags {
    pub const EMPTY: Self = Self(0);
    pub const HAS_PREFIX_RESTARTS: u8 = 0x01;
    pub const IS_ROOT_PAGE: u8 = 0x02;
    const VALID_MASK: u8 = Self::HAS_PREFIX_RESTARTS | Self::IS_ROOT_PAGE;

    pub fn from_bits(bits: u8) -> Result<Self> {
        if (bits & !Self::VALID_MASK) != 0 {
            return Err(corrupt(
                "PageFlags::from_bits",
                format!("reserved page_flags bits set: 0x{bits:02x}"),
            ));
        }
        Ok(Self(bits))
    }

    #[inline]
    pub const fn bits(self) -> u8 {
        self.0
    }

    #[inline]
    pub const fn with(self, flag: u8) -> Self {
        Self(self.0 | (flag & Self::VALID_MASK))
    }

    #[inline]
    pub const fn contains(self, flag: u8) -> bool {
        (self.0 & flag) != 0
    }
}

/// Bitmask flags on a 16-byte slot directory entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct SlotFlags(u8);

impl SlotFlags {
    pub const EMPTY: Self = Self(0);
    pub const BASE_DOC: u8 = 0x01;
    pub const DELTA_PATCH: u8 = 0x02;
    pub const HAS_OVERFLOW: u8 = 0x04;
    pub const TOMBSTONE: u8 = 0x08;
    const VALID_MASK: u8 =
        Self::BASE_DOC | Self::DELTA_PATCH | Self::HAS_OVERFLOW | Self::TOMBSTONE;

    pub fn from_bits(bits: u8) -> Result<Self> {
        if (bits & !Self::VALID_MASK) != 0 {
            return Err(corrupt(
                "SlotFlags::from_bits",
                format!("reserved slot_flags bits set: 0x{bits:02x}"),
            ));
        }
        Ok(Self(bits))
    }

    #[inline]
    pub const fn bits(self) -> u8 {
        self.0
    }

    #[inline]
    pub const fn with(self, flag: u8) -> Self {
        Self(self.0 | (flag & Self::VALID_MASK))
    }

    #[inline]
    pub const fn contains(self, flag: u8) -> bool {
        (self.0 & flag) != 0
    }
}

/// 32-byte logical page header (`PageId >= 1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageHeader {
    pub page_id: PageId,
    pub page_type: PageType,
    pub page_flags: PageFlags,
    pub slot_count: u16,
    pub free_space_end: u16,
    pub fragmented_bytes: u16,
    pub lsn_tx_id: TxId,
    pub page_checksum: u64,
}

impl PageHeader {
    /// Computes the canonical `XXH3_64` over `page[0..24] || page[32..page_size]`.
    pub fn compute_checksum(page: &[u8]) -> u64 {
        let mut hasher = Xxh3::new();
        hasher.update(&page[0..24]);
        hasher.update(&page[32..]);
        hasher.digest()
    }

    /// Encodes this header (except checksum) into `page[0..24]`, computes `page_checksum`,
    /// and writes it into `page[24..32]`.
    pub fn encode_and_checksum(&mut self, page: &mut [u8]) {
        page[0..8].copy_from_slice(&self.page_id.raw().to_le_bytes());
        page[8] = self.page_type as u8;
        page[9] = self.page_flags.bits();
        page[10..12].copy_from_slice(&self.slot_count.to_le_bytes());
        page[12..14].copy_from_slice(&self.free_space_end.to_le_bytes());
        page[14..16].copy_from_slice(&self.fragmented_bytes.to_le_bytes());
        page[16..24].copy_from_slice(&self.lsn_tx_id.raw().to_le_bytes());
        let checksum = Self::compute_checksum(page);
        self.page_checksum = checksum;
        page[24..32].copy_from_slice(&checksum.to_le_bytes());
    }

    /// Decodes and validates a 32-byte header and full-page `XXH3_64` checksum.
    pub fn decode_and_verify(page: &[u8], expected_page_id: PageId) -> Result<Self> {
        if page.len() < PAGE_HEADER_SIZE {
            return Err(corrupt(
                "PageHeader::decode_and_verify",
                format!("page buffer too small ({} bytes)", page.len()),
            ));
        }
        let stored_checksum = u64::from_le_bytes(page[24..32].try_into().unwrap());
        let computed = Self::compute_checksum(page);
        if stored_checksum != computed {
            return Err(corrupt(
                "PageHeader::decode_and_verify",
                format!(
                    "page {} checksum mismatch: stored=0x{stored_checksum:016x}, computed=0x{computed:016x}",
                    expected_page_id.raw()
                ),
            ));
        }
        Self::decode_without_checksum(page, expected_page_id)
    }

    /// Decodes a 32-byte header from an already I/O-verified page without recomputing `XXH3_64`.
    #[inline]
    pub fn decode_without_checksum(page: &[u8], expected_page_id: PageId) -> Result<Self> {
        if page.len() < PAGE_HEADER_SIZE {
            return Err(corrupt(
                "PageHeader::decode_without_checksum",
                format!("page buffer too small ({} bytes)", page.len()),
            ));
        }
        let stored_checksum = u64::from_le_bytes(page[24..32].try_into().unwrap());
        let raw_id = u64::from_le_bytes(page[0..8].try_into().unwrap());
        if raw_id == 0 {
            return Err(corrupt(
                "PageHeader::decode_and_verify",
                "logical page header has invalid PageId 0",
            ));
        }
        let page_id = PageId::new(raw_id);
        if page_id != expected_page_id {
            return Err(corrupt(
                "PageHeader::decode_and_verify",
                format!(
                    "page_id mismatch: expected {}, found {}",
                    expected_page_id.raw(),
                    page_id.raw()
                ),
            ));
        }

        let page_type = PageType::from_u8(page[8])?;
        let page_flags = PageFlags::from_bits(page[9])?;
        let slot_count = u16::from_le_bytes([page[10], page[11]]);
        let free_space_end = u16::from_le_bytes([page[12], page[13]]);
        let fragmented_bytes = u16::from_le_bytes([page[14], page[15]]);
        let lsn_tx_id = TxId::new(u64::from_le_bytes(page[16..24].try_into().unwrap()));

        if usize::from(free_space_end) > page.len() {
            return Err(corrupt(
                "PageHeader::decode_and_verify",
                format!(
                    "free_space_end {free_space_end} exceeds page_size {}",
                    page.len()
                ),
            ));
        }

        Ok(Self {
            page_id,
            page_type,
            page_flags,
            slot_count,
            free_space_end,
            fragmented_bytes,
            lsn_tx_id,
            page_checksum: stored_checksum,
        })
    }
}

/// 16-byte slot directory entry (`docs/file-format.md` §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlotEntry {
    pub payload_offset: u16,
    pub payload_len: u16,
    pub slot_flags: SlotFlags,
    pub delta_count: u8,
    pub head_delta_slot: u16,
    pub field_bloom_mask: u64,
}

impl SlotEntry {
    pub fn new_live(payload_offset: u16, payload_len: u16, slot_flags: SlotFlags) -> Self {
        Self {
            payload_offset,
            payload_len,
            slot_flags,
            delta_count: 0,
            head_delta_slot: NO_DELTA_SLOT,
            field_bloom_mask: 0,
        }
    }

    pub fn tombstone() -> Self {
        Self {
            payload_offset: 0,
            payload_len: 0,
            slot_flags: SlotFlags::EMPTY.with(SlotFlags::TOMBSTONE),
            delta_count: 0,
            head_delta_slot: NO_DELTA_SLOT,
            field_bloom_mask: 0,
        }
    }

    pub fn is_tombstone(&self) -> bool {
        self.slot_flags.contains(SlotFlags::TOMBSTONE)
    }

    pub fn encode(&self, out: &mut [u8; SLOT_ENTRY_SIZE]) {
        out[0..2].copy_from_slice(&self.payload_offset.to_le_bytes());
        out[2..4].copy_from_slice(&self.payload_len.to_le_bytes());
        out[4] = self.slot_flags.bits();
        out[5] = self.delta_count;
        out[6..8].copy_from_slice(&self.head_delta_slot.to_le_bytes());
        out[8..16].copy_from_slice(&self.field_bloom_mask.to_le_bytes());
    }

    pub fn decode(buf: &[u8; SLOT_ENTRY_SIZE]) -> Result<Self> {
        let slot_flags = SlotFlags::from_bits(buf[4])?;
        let delta_count = buf[5];
        if delta_count > 4 {
            return Err(corrupt(
                "SlotEntry::decode",
                format!("delta_count {delta_count} exceeds max 4"),
            ));
        }
        Ok(Self {
            payload_offset: u16::from_le_bytes([buf[0], buf[1]]),
            payload_len: u16::from_le_bytes([buf[2], buf[3]]),
            slot_flags,
            delta_count,
            head_delta_slot: u16::from_le_bytes([buf[6], buf[7]]),
            field_bloom_mask: u64::from_le_bytes(buf[8..16].try_into().unwrap()),
        })
    }
}

/// Validated read-only view over a slotted logical page.
#[derive(Debug, Clone, Copy)]
pub struct SlottedPageView<'a> {
    bytes: &'a [u8],
    header: PageHeader,
}

impl<'a> SlottedPageView<'a> {
    /// Parses and validates header checksum, directory bounds, and non-overlapping live spans.
    pub fn parse(page_bytes: &'a [u8], expected_page_id: PageId) -> Result<Self> {
        let header = PageHeader::decode_and_verify(page_bytes, expected_page_id)?;
        Self::parse_with_header(page_bytes, header)
    }

    /// Validates directory bounds and non-overlapping live spans using an already-verified header.
    pub fn parse_with_header(page_bytes: &'a [u8], header: PageHeader) -> Result<Self> {
        let dir_end = PAGE_HEADER_SIZE
            .checked_add(usize::from(header.slot_count) * SLOT_ENTRY_SIZE)
            .ok_or_else(|| corrupt("SlottedPageView::parse", "slot directory size overflow"))?;

        if dir_end > usize::from(header.free_space_end) {
            return Err(corrupt(
                "SlottedPageView::parse",
                format!(
                    "slot directory end {dir_end} overlaps free_space_end {}",
                    header.free_space_end
                ),
            ));
        }

        let mut spans: Vec<(usize, usize)> = Vec::with_capacity(usize::from(header.slot_count));
        for idx in 0..usize::from(header.slot_count) {
            let off = PAGE_HEADER_SIZE + idx * SLOT_ENTRY_SIZE;
            let raw: &[u8; SLOT_ENTRY_SIZE] =
                page_bytes[off..off + SLOT_ENTRY_SIZE].try_into().unwrap();
            let entry = SlotEntry::decode(raw)?;
            if entry.is_tombstone() {
                if entry.payload_len != 0 {
                    return Err(corrupt(
                        "SlottedPageView::parse",
                        format!("tombstone slot {idx} has non-zero payload_len"),
                    ));
                }
                continue;
            }
            let start = usize::from(entry.payload_offset);
            let len = usize::from(entry.payload_len);
            let end = start
                .checked_add(len)
                .ok_or_else(|| corrupt("SlottedPageView::parse", "slot span overflow"))?;
            if start < usize::from(header.free_space_end) || end > page_bytes.len() {
                return Err(corrupt(
                    "SlottedPageView::parse",
                    format!(
                        "slot {idx} span [{start}..{end}) outside valid payload region [{}..{})",
                        header.free_space_end,
                        page_bytes.len()
                    ),
                ));
            }
            if len > 0 {
                spans.push((start, end));
            }
        }

        // Verify live payload spans are strictly non-overlapping.
        spans.sort_unstable_by_key(|&(s, _)| s);
        for w in spans.windows(2) {
            if w[0].1 > w[1].0 {
                return Err(corrupt(
                    "SlottedPageView::parse",
                    format!(
                        "overlapping slot payload spans [{}, {}) and [{}, {})",
                        w[0].0, w[0].1, w[1].0, w[1].1
                    ),
                ));
            }
        }

        Ok(Self {
            bytes: page_bytes,
            header,
        })
    }

    /// O(1), allocation-free view over an immutable page already verified at the I/O or write
    /// boundary (skips redundant `XXH3_64` hashing and full-directory span sorting).
    #[inline]
    pub fn parse_verified(page_bytes: &'a [u8], expected_page_id: PageId) -> Result<Self> {
        let header = PageHeader::decode_without_checksum(page_bytes, expected_page_id)?;
        let dir_end = PAGE_HEADER_SIZE
            .checked_add(usize::from(header.slot_count) * SLOT_ENTRY_SIZE)
            .ok_or_else(|| {
                corrupt(
                    "SlottedPageView::parse_verified",
                    "slot directory size overflow",
                )
            })?;
        if dir_end > usize::from(header.free_space_end) {
            return Err(corrupt(
                "SlottedPageView::parse_verified",
                format!(
                    "slot directory end {dir_end} overlaps free_space_end {}",
                    header.free_space_end
                ),
            ));
        }
        Ok(Self {
            bytes: page_bytes,
            header,
        })
    }

    #[inline]
    pub fn header(&self) -> PageHeader {
        self.header
    }

    #[inline]
    pub fn slot_count(&self) -> u16 {
        self.header.slot_count
    }

    /// Contiguous unallocated free bytes between slot directory end and `free_space_end`.
    #[inline]
    pub fn contiguous_free_bytes(&self) -> usize {
        let dir_end = PAGE_HEADER_SIZE + usize::from(self.header.slot_count) * SLOT_ENTRY_SIZE;
        usize::from(self.header.free_space_end).saturating_sub(dir_end)
    }

    pub fn slot(&self, idx: u16) -> Option<SlotEntry> {
        if idx >= self.header.slot_count {
            return None;
        }
        let off = PAGE_HEADER_SIZE + usize::from(idx) * SLOT_ENTRY_SIZE;
        let raw: &[u8; SLOT_ENTRY_SIZE] =
            self.bytes[off..off + SLOT_ENTRY_SIZE].try_into().unwrap();
        SlotEntry::decode(raw).ok()
    }

    pub fn payload(&self, idx: u16) -> Option<&'a [u8]> {
        let entry = self.slot(idx)?;
        if entry.is_tombstone() {
            return None;
        }
        let start = usize::from(entry.payload_offset);
        let end = start + usize::from(entry.payload_len);
        Some(&self.bytes[start..end])
    }
}

/// Mutable slotted page editor operating directly on a page-sized byte slice.
#[derive(Debug)]
pub struct SlottedPageMut<'a> {
    bytes: &'a mut [u8],
    header: PageHeader,
}

impl<'a> SlottedPageMut<'a> {
    /// Initializes a blank slotted page with zero slots and computes its initial checksum.
    pub fn init(
        bytes: &'a mut [u8],
        page_id: PageId,
        page_type: PageType,
        page_flags: PageFlags,
        lsn_tx_id: TxId,
    ) -> Result<Self> {
        let page_len = u16::try_from(bytes.len()).map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "SlottedPageMut::init",
                "page buffer exceeds u16::MAX",
            )
        })?;
        bytes.fill(0);
        let mut header = PageHeader {
            page_id,
            page_type,
            page_flags,
            slot_count: 0,
            free_space_end: page_len,
            fragmented_bytes: 0,
            lsn_tx_id,
            page_checksum: 0,
        };
        header.encode_and_checksum(bytes);
        Ok(Self { bytes, header })
    }

    /// Opens an existing validated slotted page for mutation under `lsn_tx_id`.
    pub fn from_bytes(
        bytes: &'a mut [u8],
        expected_page_id: PageId,
        lsn_tx_id: TxId,
    ) -> Result<Self> {
        let view = SlottedPageView::parse(bytes, expected_page_id)?;
        let mut header = view.header();
        header.lsn_tx_id = lsn_tx_id;
        Ok(Self { bytes, header })
    }

    #[inline]
    pub fn header(&self) -> PageHeader {
        self.header
    }

    #[inline]
    pub fn contiguous_free_bytes(&self) -> usize {
        let dir_end = PAGE_HEADER_SIZE + usize::from(self.header.slot_count) * SLOT_ENTRY_SIZE;
        usize::from(self.header.free_space_end).saturating_sub(dir_end)
    }

    /// Inserts a new payload, returning its assigned slot index, or `None` if insufficient space.
    pub fn insert_slot(&mut self, payload: &[u8], flags: SlotFlags) -> Option<u16> {
        let needed = SLOT_ENTRY_SIZE + payload.len();
        if self.contiguous_free_bytes() < needed {
            if self.contiguous_free_bytes() + usize::from(self.header.fragmented_bytes) >= needed {
                self.compact_payloads();
            } else {
                return None;
            }
        }
        if self.contiguous_free_bytes() < needed {
            return None;
        }

        let slot_idx = self.header.slot_count;
        let new_free_end = usize::from(self.header.free_space_end) - payload.len();
        self.bytes[new_free_end..new_free_end + payload.len()].copy_from_slice(payload);

        let entry = SlotEntry::new_live(new_free_end as u16, payload.len() as u16, flags);
        let dir_off = PAGE_HEADER_SIZE + usize::from(slot_idx) * SLOT_ENTRY_SIZE;
        let mut raw = [0u8; SLOT_ENTRY_SIZE];
        entry.encode(&mut raw);
        self.bytes[dir_off..dir_off + SLOT_ENTRY_SIZE].copy_from_slice(&raw);

        self.header.slot_count += 1;
        self.header.free_space_end = new_free_end as u16;
        Some(slot_idx)
    }

    /// Marks `slot_idx` as a tombstone and zeroes its dead payload bytes.
    pub fn tombstone_slot(&mut self, slot_idx: u16) -> Result<()> {
        if slot_idx >= self.header.slot_count {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "SlottedPageMut::tombstone_slot",
                format!("slot index {slot_idx} out of bounds"),
            ));
        }
        let dir_off = PAGE_HEADER_SIZE + usize::from(slot_idx) * SLOT_ENTRY_SIZE;
        let raw: &[u8; SLOT_ENTRY_SIZE] = self.bytes[dir_off..dir_off + SLOT_ENTRY_SIZE]
            .try_into()
            .unwrap();
        let entry = SlotEntry::decode(raw)?;
        if !entry.is_tombstone() {
            let start = usize::from(entry.payload_offset);
            let end = start + usize::from(entry.payload_len);
            self.bytes[start..end].fill(0);
            self.header.fragmented_bytes = self
                .header
                .fragmented_bytes
                .saturating_add(entry.payload_len);
            let mut tomb = [0u8; SLOT_ENTRY_SIZE];
            SlotEntry::tombstone().encode(&mut tomb);
            self.bytes[dir_off..dir_off + SLOT_ENTRY_SIZE].copy_from_slice(&tomb);
        }
        Ok(())
    }

    /// Defragments live payloads to the top of the page while preserving slot indices.
    pub fn compact_payloads(&mut self) {
        let mut live: Vec<(u16, SlotEntry, Vec<u8>)> = Vec::new();
        for idx in 0..self.header.slot_count {
            let dir_off = PAGE_HEADER_SIZE + usize::from(idx) * SLOT_ENTRY_SIZE;
            let raw: &[u8; SLOT_ENTRY_SIZE] = self.bytes[dir_off..dir_off + SLOT_ENTRY_SIZE]
                .try_into()
                .unwrap();
            if let Ok(entry) = SlotEntry::decode(raw) {
                if !entry.is_tombstone() {
                    let start = usize::from(entry.payload_offset);
                    let end = start + usize::from(entry.payload_len);
                    live.push((idx, entry, self.bytes[start..end].to_vec()));
                }
            }
        }

        let dir_end = PAGE_HEADER_SIZE + usize::from(self.header.slot_count) * SLOT_ENTRY_SIZE;
        self.bytes[dir_end..].fill(0);

        let mut cursor = self.bytes.len();
        for (idx, mut entry, data) in live {
            cursor -= data.len();
            self.bytes[cursor..cursor + data.len()].copy_from_slice(&data);
            entry.payload_offset = cursor as u16;
            let dir_off = PAGE_HEADER_SIZE + usize::from(idx) * SLOT_ENTRY_SIZE;
            let mut raw = [0u8; SLOT_ENTRY_SIZE];
            entry.encode(&mut raw);
            self.bytes[dir_off..dir_off + SLOT_ENTRY_SIZE].copy_from_slice(&raw);
        }
        self.header.free_space_end = cursor as u16;
        self.header.fragmented_bytes = 0;
    }

    /// Writes the updated header and recomputes the full-page `XXH3_64` checksum.
    pub fn finalize(mut self) -> PageHeader {
        self.header.encode_and_checksum(self.bytes);
        self.header
    }
}

/// 16-byte overflow chain reference stored in a collection record or descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverflowRef {
    pub first_page: PageId,
    pub total_len: u32,
    pub original_tag: u8,
}

impl OverflowRef {
    pub fn encode(&self, out: &mut [u8; OVERFLOW_REF_SIZE]) {
        out.fill(0);
        out[0..8].copy_from_slice(&self.first_page.raw().to_le_bytes());
        out[8..12].copy_from_slice(&self.total_len.to_le_bytes());
        out[12] = self.original_tag;
        // Bytes 13..16 reserved zero.
    }

    pub fn decode(buf: &[u8; OVERFLOW_REF_SIZE]) -> Result<Self> {
        if buf[13..16] != [0, 0, 0] {
            return Err(corrupt(
                "OverflowRef::decode",
                "non-zero reserved bytes in OverflowRef",
            ));
        }
        let raw_page = u64::from_le_bytes(buf[0..8].try_into().unwrap());
        if raw_page == 0 {
            return Err(corrupt(
                "OverflowRef::decode",
                "OverflowRef first_page cannot be Page 0",
            ));
        }
        let first_page = PageId::new(raw_page);
        let total_len = u32::from_le_bytes(buf[8..12].try_into().unwrap());
        Ok(Self {
            first_page,
            total_len,
            original_tag: buf[12],
        })
    }
}

/// Overflow page (`PageType::Overflow`) fixed 16-byte body header at bytes `32..48`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverflowHeader {
    pub next_page: Option<PageId>,
    pub chunk_len: u32,
}

impl OverflowHeader {
    pub fn max_chunk_capacity(page_size: usize) -> usize {
        page_size.saturating_sub(OVERFLOW_HEADER_SIZE)
    }

    pub fn encode_page(
        page: &mut [u8],
        page_id: PageId,
        lsn_tx_id: TxId,
        next_page: Option<PageId>,
        chunk: &[u8],
    ) -> Result<PageHeader> {
        let cap = Self::max_chunk_capacity(page.len());
        if chunk.len() > cap {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "OverflowHeader::encode_page",
                format!(
                    "chunk len {} exceeds overflow page capacity {cap}",
                    chunk.len()
                ),
            ));
        }
        page.fill(0);
        let next_raw = next_page.map_or(0, PageId::raw);
        page[32..40].copy_from_slice(&next_raw.to_le_bytes());
        page[40..44].copy_from_slice(&(chunk.len() as u32).to_le_bytes());
        // bytes 44..48 remain 0
        page[OVERFLOW_HEADER_SIZE..OVERFLOW_HEADER_SIZE + chunk.len()].copy_from_slice(chunk);

        let mut hdr = PageHeader {
            page_id,
            page_type: PageType::Overflow,
            page_flags: PageFlags::EMPTY,
            slot_count: 0,
            free_space_end: page.len() as u16,
            fragmented_bytes: 0,
            lsn_tx_id,
            page_checksum: 0,
        };
        hdr.encode_and_checksum(page);
        Ok(hdr)
    }

    pub fn decode_page(page: &[u8], expected_page_id: PageId) -> Result<(PageHeader, Self, &[u8])> {
        let hdr = PageHeader::decode_and_verify(page, expected_page_id)?;
        if hdr.page_type != PageType::Overflow {
            return Err(corrupt(
                "OverflowHeader::decode_page",
                format!("expected Overflow page, got {:?}", hdr.page_type),
            ));
        }
        if hdr.slot_count != 0 {
            return Err(corrupt(
                "OverflowHeader::decode_page",
                "overflow page must have slot_count == 0",
            ));
        }
        if page[44..48] != [0, 0, 0, 0] {
            return Err(corrupt(
                "OverflowHeader::decode_page",
                "non-zero reserved bytes [44..48] in overflow page",
            ));
        }
        let next_raw = u64::from_le_bytes(page[32..40].try_into().unwrap());
        let next_page = if next_raw == 0 {
            None
        } else {
            Some(PageId::new(next_raw))
        };
        let chunk_len = u32::from_le_bytes(page[40..44].try_into().unwrap());
        let cap = Self::max_chunk_capacity(page.len());
        if (chunk_len as usize) > cap {
            return Err(corrupt(
                "OverflowHeader::decode_page",
                format!("overflow chunk_len {chunk_len} exceeds page capacity {cap}"),
            ));
        }
        let end = OVERFLOW_HEADER_SIZE + chunk_len as usize;
        if page[end..].iter().any(|&b| b != 0) {
            return Err(corrupt(
                "OverflowHeader::decode_page",
                "non-zero trailing bytes after overflow chunk",
            ));
        }
        Ok((
            hdr,
            Self {
                next_page,
                chunk_len,
            },
            &page[OVERFLOW_HEADER_SIZE..end],
        ))
    }
}

/// Freelist trunk page (`PageType::Freelist`) header at bytes `32..48`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreelistTrunkHeader {
    pub next_trunk: Option<PageId>,
    pub entry_count: u32,
}

/// Single retired-page entry in a freelist trunk page (`16` bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreelistEntry {
    pub page_id: PageId,
    pub retired_tx_id: TxId,
}

impl FreelistTrunkHeader {
    pub fn max_entries(page_size: usize) -> usize {
        page_size.saturating_sub(FREELIST_HEADER_SIZE) / FREELIST_ENTRY_SIZE
    }

    pub fn encode_page(
        page: &mut [u8],
        page_id: PageId,
        lsn_tx_id: TxId,
        next_trunk: Option<PageId>,
        entries: &[FreelistEntry],
    ) -> Result<PageHeader> {
        let cap = Self::max_entries(page.len());
        if entries.len() > cap {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "FreelistTrunkHeader::encode_page",
                format!(
                    "freelist entry count {} exceeds capacity {cap}",
                    entries.len()
                ),
            ));
        }
        page.fill(0);
        let next_raw = next_trunk.map_or(0, PageId::raw);
        page[32..40].copy_from_slice(&next_raw.to_le_bytes());
        page[40..44].copy_from_slice(&(entries.len() as u32).to_le_bytes());
        // bytes 44..48 remain 0

        for (i, entry) in entries.iter().enumerate() {
            let off = FREELIST_HEADER_SIZE + i * FREELIST_ENTRY_SIZE;
            page[off..off + 8].copy_from_slice(&entry.page_id.raw().to_le_bytes());
            page[off + 8..off + 16].copy_from_slice(&entry.retired_tx_id.raw().to_le_bytes());
        }

        let mut hdr = PageHeader {
            page_id,
            page_type: PageType::Freelist,
            page_flags: PageFlags::EMPTY,
            slot_count: 0,
            free_space_end: page.len() as u16,
            fragmented_bytes: 0,
            lsn_tx_id,
            page_checksum: 0,
        };
        hdr.encode_and_checksum(page);
        Ok(hdr)
    }

    pub fn decode_page(
        page: &[u8],
        expected_page_id: PageId,
    ) -> Result<(PageHeader, Self, Vec<FreelistEntry>)> {
        let hdr = PageHeader::decode_and_verify(page, expected_page_id)?;
        if hdr.page_type != PageType::Freelist {
            return Err(corrupt(
                "FreelistTrunkHeader::decode_page",
                format!("expected Freelist page, got {:?}", hdr.page_type),
            ));
        }
        if hdr.slot_count != 0 {
            return Err(corrupt(
                "FreelistTrunkHeader::decode_page",
                "freelist trunk page must have slot_count == 0",
            ));
        }
        if page[44..48] != [0, 0, 0, 0] {
            return Err(corrupt(
                "FreelistTrunkHeader::decode_page",
                "non-zero reserved bytes [44..48] in freelist trunk page",
            ));
        }
        let next_raw = u64::from_le_bytes(page[32..40].try_into().unwrap());
        let next_trunk = if next_raw == 0 {
            None
        } else {
            Some(PageId::new(next_raw))
        };
        let entry_count = u32::from_le_bytes(page[40..44].try_into().unwrap());
        let cap = Self::max_entries(page.len());
        if (entry_count as usize) > cap {
            return Err(corrupt(
                "FreelistTrunkHeader::decode_page",
                format!("freelist entry_count {entry_count} exceeds capacity {cap}"),
            ));
        }
        let mut entries = Vec::with_capacity(entry_count as usize);
        for i in 0..(entry_count as usize) {
            let off = FREELIST_HEADER_SIZE + i * FREELIST_ENTRY_SIZE;
            let raw_pid = u64::from_le_bytes(page[off..off + 8].try_into().unwrap());
            if raw_pid == 0 {
                return Err(corrupt(
                    "FreelistTrunkHeader::decode_page",
                    "freelist entry cannot reference Page 0",
                ));
            }
            let pid = PageId::new(raw_pid);
            let retired_tx_id = TxId::new(u64::from_le_bytes(
                page[off + 8..off + 16].try_into().unwrap(),
            ));
            entries.push(FreelistEntry {
                page_id: pid,
                retired_tx_id,
            });
        }
        let used_end = FREELIST_HEADER_SIZE + (entry_count as usize) * FREELIST_ENTRY_SIZE;
        if page[used_end..].iter().any(|&b| b != 0) {
            return Err(corrupt(
                "FreelistTrunkHeader::decode_page",
                "non-zero unused bytes in freelist trunk page",
            ));
        }
        Ok((
            hdr,
            Self {
                next_trunk,
                entry_count,
            },
            entries,
        ))
    }
}

fn corrupt(op: &'static str, msg: impl Into<String>) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCategory::Corruption, MODULE_ID, op, msg)
}
