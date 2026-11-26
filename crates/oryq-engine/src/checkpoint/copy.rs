//! Materializes committed WAL page versions up to a snapshot horizon into `.oryq`.

use std::collections::BTreeMap;

use oryq_types::{PageId, Result, SnapshotId};

use crate::cache::{PageVersionLocation, VersionIndex};
use crate::io::FileHandle;
use crate::pager::Pager;
use crate::recovery::CommittedDbMeta;

/// Copies the newest committed version (with `tx_id <= horizon`) of every page in `page_ids`
/// from `.oryq-wal` into the main `.oryq` file, syncs data, and commits Page 0 shadow header.
///
/// Returns the number of pages copied into `.oryq`.
pub fn copy_committed_pages_to_main(
    pager: &mut Pager,
    wal_file: &dyn FileHandle,
    versions: &VersionIndex,
    dirty_page_ids: &[PageId],
    horizon: SnapshotId,
    meta_at_horizon: CommittedDbMeta,
) -> Result<usize> {
    let page_bytes = pager.page_size().bytes();
    let mut buf = vec![0u8; page_bytes];
    let mut to_copy: BTreeMap<PageId, u64> = BTreeMap::new();

    for &pid in dirty_page_ids {
        if let Some(entry) = versions.resolve_for_snapshot(pid, horizon) {
            if entry.tx_id > pager.header().checkpointed_tx_id {
                if let PageVersionLocation::WalFrame { file_offset, .. } = entry.location {
                    to_copy.insert(pid, file_offset);
                }
            }
        }
    }

    for (&pid, &wal_offset) in &to_copy {
        wal_file.read_exact_at(&mut buf, wal_offset)?;
        pager.write_page(pid, &buf)?;
    }

    // Ensure `.oryq` physical length covers `meta_at_horizon.total_page_count` even if the
    // highest allocated page was retired before commit without being written directly.
    pager.ensure_min_page_count(meta_at_horizon.total_page_count)?;

    // Update header metadata to horizon and commit alternate shadow slot (syncs data first, then Page 0).
    let hdr = pager.header_mut();
    hdr.checkpointed_tx_id = horizon;
    hdr.total_page_count = meta_at_horizon.total_page_count;
    hdr.catalog_root_page_id = meta_at_horizon.catalog_root_page_id;
    hdr.freelist_trunk_page_id = meta_at_horizon.freelist_trunk_page_id;
    hdr.freelist_page_count = meta_at_horizon.freelist_page_count;
    pager.commit_header()?;

    Ok(to_copy.len())
}
