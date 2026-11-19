//! Restartable physical WAL replay into [`VersionIndex`] without domain callbacks.

use oryq_types::{PageId, Result, TxId};

use crate::cache::{PageVersionEntry, PageVersionLocation, VersionIndex};
use crate::format::StorageHeader;
use crate::recovery::scan::WalScanResult;

/// Latest committed physical database metadata (either from Page 0 or the latest WAL CommitFrame).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommittedDbMeta {
    pub latest_tx_id: TxId,
    pub total_page_count: u64,
    pub catalog_root_page_id: PageId,
    pub freelist_trunk_page_id: u64,
    pub freelist_page_count: u64,
}

impl CommittedDbMeta {
    pub fn from_header(hdr: &StorageHeader) -> Self {
        Self {
            latest_tx_id: hdr.checkpointed_tx_id,
            total_page_count: hdr.total_page_count,
            catalog_root_page_id: hdr.catalog_root_page_id,
            freelist_trunk_page_id: hdr.freelist_trunk_page_id,
            freelist_page_count: hdr.freelist_page_count,
        }
    }
}

/// Replays all committed transactions from `scan` into `versions` and returns the
/// resulting [`CommittedDbMeta`].
///
/// Only transactions with `tx.tx_id > header.checkpointed_tx_id` are applied, making
/// recovery idempotent and restartable.
pub fn replay_into_version_index(
    header: &StorageHeader,
    scan: &WalScanResult,
    versions: &mut VersionIndex,
) -> Result<CommittedDbMeta> {
    let mut meta = CommittedDbMeta::from_header(header);

    for tx in &scan.committed_txs {
        if tx.tx_id <= header.checkpointed_tx_id {
            continue;
        }
        for pf in &tx.pages {
            versions.record_version(
                pf.page_id,
                PageVersionEntry {
                    tx_id: tx.tx_id,
                    location: PageVersionLocation::WalFrame {
                        frame_seq: pf.frame_seq,
                        file_offset: pf.payload_offset,
                    },
                },
            )?;
        }
        meta.latest_tx_id = tx.tx_id;
        meta.total_page_count = tx.commit.db_total_pages;
        meta.catalog_root_page_id = PageId::new(tx.commit.catalog_root);
        meta.freelist_trunk_page_id = tx.commit.freelist_root;
        meta.freelist_page_count = tx.commit.freelist_page_count;
    }

    Ok(meta)
}
