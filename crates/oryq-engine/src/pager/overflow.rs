//! Whole-record overflow chain writer, bounded reader, and transactional freelist reclaimer
//! (`docs/file-format.md` §4.1).

use std::collections::HashSet;

use oryq_types::{BudgetHandle, ErrorCategory, ErrorEnvelope, Reservation, Result, TxId};

use crate::format::{OverflowHeader, OverflowRef};
use crate::pager::allocator::PageAllocator;
use crate::pager::Pager;

const MODULE_ID: &str = "engine";

/// Manages `PageType::Overflow` page chains for payloads exceeding a single slotted page.
#[derive(Debug, Default)]
pub struct OverflowStore;

impl OverflowStore {
    /// Persists `payload` across one or more `PageType::Overflow` pages and returns an [`OverflowRef`].
    pub fn write_blob(
        pager: &mut Pager,
        payload: &[u8],
        original_tag: u8,
        min_active_snapshot_tx_id: TxId,
        writer_tx_id: TxId,
    ) -> Result<OverflowRef> {
        let total_len = u32::try_from(payload.len()).map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "OverflowStore::write_blob",
                "overflow blob exceeds u32::MAX bytes",
            )
        })?;

        let page_size = pager.page_size().bytes();
        let cap = OverflowHeader::max_chunk_capacity(page_size);
        let chunk_count = if payload.is_empty() {
            1
        } else {
            payload.len().div_ceil(cap)
        };

        let mut page_ids = Vec::with_capacity(chunk_count);
        for _ in 0..chunk_count {
            let pid = PageAllocator::allocate_page(pager, min_active_snapshot_tx_id, writer_tx_id)?;
            page_ids.push(pid);
        }

        let mut page_buf = vec![0u8; page_size];
        for (idx, &pid) in page_ids.iter().enumerate() {
            let start = idx * cap;
            let end = (start + cap).min(payload.len());
            let chunk = if start < payload.len() {
                &payload[start..end]
            } else {
                &[]
            };
            let next_page = page_ids.get(idx + 1).copied();
            OverflowHeader::encode_page(&mut page_buf, pid, writer_tx_id, next_page, chunk)?;
            pager.write_page(pid, &page_buf)?;
        }

        Ok(OverflowRef {
            first_page: page_ids[0],
            total_len,
            original_tag,
        })
    }

    /// Reads and reassembles an overflow chain into an owned `Vec<u8>`, enforcing cycle
    /// detection, out-of-file bounds, chunk capacity, exact `total_len`, and `budget`.
    pub fn read_blob(
        pager: &Pager,
        oref: &OverflowRef,
        budget: Option<&BudgetHandle>,
    ) -> Result<(Vec<u8>, Option<Reservation>)> {
        let expected_len = oref.total_len as usize;
        let reservation = match budget {
            Some(b) => Some(b.reserve(expected_len)?),
            None => None,
        };

        let mut out = Vec::with_capacity(expected_len);
        let mut visited = HashSet::new();
        let max_pages = pager.total_page_count();
        let mut current = Some(oref.first_page);

        while let Some(pid) = current {
            if !visited.insert(pid) {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "OverflowStore::read_blob",
                    format!("cycle detected in overflow chain at page {}", pid.raw()),
                ));
            }
            if (visited.len() as u64) > max_pages {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "OverflowStore::read_blob",
                    "overflow chain length exceeds total_page_count",
                ));
            }

            let guard = pager.read_page(pid, None)?;
            let (_, ohdr, chunk) = OverflowHeader::decode_page(guard.as_slice(), pid)?;
            if out.len().saturating_add(chunk.len()) > expected_len {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "OverflowStore::read_blob",
                    format!("overflow chain exceeds declared total_len {expected_len}"),
                ));
            }
            out.extend_from_slice(chunk);
            current = ohdr.next_page;
        }

        if out.len() != expected_len {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "OverflowStore::read_blob",
                format!(
                    "overflow chain truncated: read {} bytes, expected {expected_len}",
                    out.len()
                ),
            ));
        }

        Ok((out, reservation))
    }

    /// Walks an overflow chain and retires all its pages into the freelist under `retired_tx_id`.
    pub fn free_blob(pager: &mut Pager, oref: &OverflowRef, retired_tx_id: TxId) -> Result<usize> {
        let mut visited = HashSet::new();
        let mut to_free = Vec::new();
        let mut current = Some(oref.first_page);

        while let Some(pid) = current {
            if !visited.insert(pid) {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "OverflowStore::free_blob",
                    format!("cycle detected in overflow chain at page {}", pid.raw()),
                ));
            }
            let guard = pager.read_page(pid, None)?;
            let (_, ohdr, _) = OverflowHeader::decode_page(guard.as_slice(), pid)?;
            to_free.push(pid);
            current = ohdr.next_page;
        }

        let count = to_free.len();
        for pid in to_free {
            PageAllocator::free_page(pager, pid, retired_tx_id)?;
        }
        Ok(count)
    }
}
