//! Freelist trunk-chain manager with MVCC snapshot-horizon reuse gating
//! (`docs/file-format.md` §4.1).

use std::collections::HashSet;

use oryq_types::{ErrorCategory, ErrorEnvelope, PageId, Result, TxId};

use crate::format::{FreelistEntry, FreelistTrunkHeader};
use crate::pager::Pager;

const MODULE_ID: &str = "engine";

/// Manages `PageType::Freelist` trunk pages for retiring and reclaiming pages.
#[derive(Debug, Default)]
pub struct FreelistManager;

impl FreelistManager {
    /// Retires `page_id` as freed by `retired_tx_id`, appending to the current freelist trunk
    /// or allocating a new head trunk if the current trunk is absent or full.
    pub fn retire_page(pager: &mut Pager, page_id: PageId, retired_tx_id: TxId) -> Result<()> {
        if page_id == PageId::HEADER {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "FreelistManager::retire_page",
                "cannot retire Page 0",
            ));
        }
        if page_id == pager.header().catalog_root_page_id {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "FreelistManager::retire_page",
                "cannot retire the live catalog_root_page_id",
            ));
        }
        if page_id.raw() >= pager.total_page_count() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "FreelistManager::retire_page",
                format!(
                    "page_id {} out of file bounds (total_page_count={})",
                    page_id.raw(),
                    pager.total_page_count()
                ),
            ));
        }

        let page_size = pager.page_size().bytes();
        let max_entries = FreelistTrunkHeader::max_entries(page_size);
        let head_raw = pager.header().freelist_trunk_page_id;

        if head_raw != 0 {
            let head_pid = PageId::new(head_raw);
            let guard = pager.read_page(head_pid, None)?;
            let (_, trunk_hdr, mut entries) =
                FreelistTrunkHeader::decode_page(guard.as_slice(), head_pid)?;

            if entries.iter().any(|e| e.page_id == page_id) || head_pid == page_id {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "FreelistManager::retire_page",
                    format!("duplicate retirement of page {}", page_id.raw()),
                ));
            }

            if entries.len() < max_entries {
                entries.push(FreelistEntry {
                    page_id,
                    retired_tx_id,
                });
                let mut buf = vec![0u8; page_size];
                FreelistTrunkHeader::encode_page(
                    &mut buf,
                    head_pid,
                    retired_tx_id,
                    trunk_hdr.next_trunk,
                    &entries,
                )?;
                pager.write_page(head_pid, &buf)?;
                pager.header_mut().freelist_page_count += 1;
                return Ok(());
            }
        }

        // No trunk yet or head trunk is full: allocate a new trunk page at the end of the file
        // and store `page_id` as its first entry (never listing a trunk page inside itself).
        let new_trunk_pid = PageId::new(pager.total_page_count());
        let next_trunk = if head_raw == 0 {
            None
        } else {
            Some(PageId::new(head_raw))
        };
        let entries = [FreelistEntry {
            page_id,
            retired_tx_id,
        }];
        let mut buf = vec![0u8; page_size];
        FreelistTrunkHeader::encode_page(
            &mut buf,
            new_trunk_pid,
            retired_tx_id,
            next_trunk,
            &entries,
        )?;
        pager.write_page(new_trunk_pid, &buf)?;
        pager.header_mut().freelist_trunk_page_id = new_trunk_pid.raw();
        pager.header_mut().freelist_page_count += 1;
        Ok(())
    }

    /// Reclaims one retired page whose `retired_tx_id < min_active_snapshot_tx_id`,
    /// or returns `Ok(None)` if no retired page is safe to reuse yet.
    pub fn reclaim_page(
        pager: &mut Pager,
        min_active_snapshot_tx_id: TxId,
        writer_tx_id: TxId,
    ) -> Result<Option<PageId>> {
        let mut visited = HashSet::new();
        let mut current_raw = pager.header().freelist_trunk_page_id;
        let page_size = pager.page_size().bytes();

        while current_raw != 0 {
            let trunk_pid = PageId::new(current_raw);
            if !visited.insert(trunk_pid) {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "FreelistManager::reclaim_page",
                    format!(
                        "cycle detected in freelist trunk chain at page {}",
                        trunk_pid.raw()
                    ),
                ));
            }
            let guard = pager.read_page(trunk_pid, None)?;
            let (_, trunk_hdr, mut entries) =
                FreelistTrunkHeader::decode_page(guard.as_slice(), trunk_pid)?;

            if let Some(pos) = entries
                .iter()
                .position(|e| e.retired_tx_id < min_active_snapshot_tx_id)
            {
                let reclaimed = entries.swap_remove(pos);
                let mut buf = vec![0u8; page_size];
                FreelistTrunkHeader::encode_page(
                    &mut buf,
                    trunk_pid,
                    writer_tx_id,
                    trunk_hdr.next_trunk,
                    &entries,
                )?;
                pager.write_page(trunk_pid, &buf)?;
                pager.header_mut().freelist_page_count =
                    pager.header().freelist_page_count.saturating_sub(1);
                return Ok(Some(reclaimed.page_id));
            }

            current_raw = trunk_hdr.next_trunk.map_or(0, PageId::raw);
        }

        Ok(None)
    }

    /// Validates the entire freelist trunk chain for cycles, out-of-range page IDs,
    /// duplicate entries, and count consistency.
    pub fn validate_chain(pager: &Pager) -> Result<u64> {
        let mut visited_trunks = HashSet::new();
        let mut seen_pages = HashSet::new();
        let mut total_entries = 0u64;
        let mut current_raw = pager.header().freelist_trunk_page_id;

        while current_raw != 0 {
            let trunk_pid = PageId::new(current_raw);
            if !visited_trunks.insert(trunk_pid) {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "FreelistManager::validate_chain",
                    format!("cycle in freelist trunk chain at page {}", trunk_pid.raw()),
                ));
            }
            let guard = pager.read_page(trunk_pid, None)?;
            let (_, trunk_hdr, entries) =
                FreelistTrunkHeader::decode_page(guard.as_slice(), trunk_pid)?;

            for entry in entries {
                if entry.page_id.raw() >= pager.total_page_count() {
                    return Err(ErrorEnvelope::new(
                        ErrorCategory::Corruption,
                        MODULE_ID,
                        "FreelistManager::validate_chain",
                        format!(
                            "freelist entry page {} exceeds total_page_count {}",
                            entry.page_id.raw(),
                            pager.total_page_count()
                        ),
                    ));
                }
                if entry.page_id == trunk_pid || visited_trunks.contains(&entry.page_id) {
                    return Err(ErrorEnvelope::new(
                        ErrorCategory::Corruption,
                        MODULE_ID,
                        "FreelistManager::validate_chain",
                        format!("freelist lists trunk page {}", entry.page_id.raw()),
                    ));
                }
                if !seen_pages.insert(entry.page_id) {
                    return Err(ErrorEnvelope::new(
                        ErrorCategory::Corruption,
                        MODULE_ID,
                        "FreelistManager::validate_chain",
                        format!("duplicate page {} in freelist", entry.page_id.raw()),
                    ));
                }
                total_entries += 1;
            }
            current_raw = trunk_hdr.next_trunk.map_or(0, PageId::raw);
        }

        if total_entries != pager.header().freelist_page_count {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "FreelistManager::validate_chain",
                format!(
                    "header freelist_page_count {} != actual entry count {total_entries}",
                    pager.header().freelist_page_count
                ),
            ));
        }

        Ok(total_entries)
    }
}
