//! Page allocator combining horizon-gated freelist reuse with file extension.

use oryq_types::{ErrorCategory, ErrorEnvelope, PageId, Result, TxId};

use crate::pager::freelist::FreelistManager;
use crate::pager::Pager;

const MODULE_ID: &str = "engine";

/// Allocates logical pages by preferring reusable freelist pages before extending the file.
#[derive(Debug, Default)]
pub struct PageAllocator;

impl PageAllocator {
    /// Allocates a [`PageId`], reusing a retired page from the freelist if one satisfies
    /// `retired_tx_id < min_active_snapshot_tx_id`, otherwise extending `total_page_count`.
    pub fn allocate_page(
        pager: &mut Pager,
        min_active_snapshot_tx_id: TxId,
        writer_tx_id: TxId,
    ) -> Result<PageId> {
        if let Some(reused) =
            FreelistManager::reclaim_page(pager, min_active_snapshot_tx_id, writer_tx_id)?
        {
            return Ok(reused);
        }

        let next_raw = pager.total_page_count();
        let pid = PageId::new(next_raw);
        pager.header_mut().total_page_count = next_raw.checked_add(1).ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "PageAllocator::allocate_page",
                "total_page_count overflow",
            )
        })?;
        Ok(pid)
    }

    /// Retires `page_id` into the freelist under `retired_tx_id`.
    pub fn free_page(pager: &mut Pager, page_id: PageId, retired_tx_id: TxId) -> Result<()> {
        FreelistManager::retire_page(pager, page_id, retired_tx_id)
    }
}
