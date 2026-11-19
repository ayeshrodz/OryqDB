//! Ordered WAL append, durable sync, uncertain-sync poisoning, and atomic publication
//! (`docs/transactions.md` §4.1 step 5 & §6).

use std::collections::BTreeMap;
use std::sync::Arc;

use oryq_types::{ErrorCategory, ErrorEnvelope, PageId, Result, TxId};

use crate::cache::{PageCache, PageVersionEntry, PageVersionLocation, VersionIndex};
use crate::config::SyncMode;
use crate::format::{
    CommitPayload, WalFrameHeader, WalFrameKind, WAL_COMMIT_PAYLOAD_SIZE, WAL_FRAME_HEADER_SIZE,
};
use crate::io::FileHandle;
use crate::ports::CommitReceipt;
use crate::recovery::CommittedDbMeta;

const MODULE_ID: &str = "engine";

/// Mutable WAL append state guarded by the engine's writer/WAL mutex.
#[derive(Debug)]
pub(crate) struct WalAppendState {
    pub file: Option<Arc<dyn FileHandle>>,
    pub salt1: u64,
    pub next_offset: u64,
    pub last_frame_seq: u64,
    pub last_checksum: u64,
}

/// Recorded location of one committed page frame inside `.oryq-wal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WalFrameLocation {
    pub page_id: PageId,
    pub frame_seq: u64,
    pub payload_offset: u64,
}

/// Appends all dirty pages as `FullPageFrame`s followed by a `CommitFrame`, syncs `.oryq-wal`,
/// and returns the frame locations for publication.
///
/// Any write or sync error returns [`ErrorCategory::CommitOutcomeUnknown`] so the caller
/// poisons the engine and requires reopen recovery.
pub(crate) fn append_and_sync_wal_commit(
    wal: &mut WalAppendState,
    dirty_pages: &BTreeMap<PageId, Vec<u8>>,
    tx_id: TxId,
    meta: CommittedDbMeta,
    sync_mode: SyncMode,
    max_wal_hard_bytes: u64,
) -> Result<(CommitReceipt, Vec<WalFrameLocation>)> {
    let wal_file = wal.file.as_ref().ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::Internal,
            MODULE_ID,
            "publish::append_and_sync_wal_commit",
            "WAL file must be opened and unsealed before committing",
        )
    })?;

    // Check hard WAL byte cap BEFORE writing any frames.
    let mut projected_bytes: u64 = 0;
    for bytes in dirty_pages.values() {
        projected_bytes =
            projected_bytes.saturating_add(WAL_FRAME_HEADER_SIZE as u64 + bytes.len() as u64);
    }
    projected_bytes = projected_bytes
        .saturating_add(WAL_FRAME_HEADER_SIZE as u64 + WAL_COMMIT_PAYLOAD_SIZE as u64);

    if wal.next_offset.saturating_add(projected_bytes) > max_wal_hard_bytes {
        return Err(ErrorEnvelope::new(
            ErrorCategory::QuotaExceeded,
            MODULE_ID,
            "publish::append_and_sync_wal_commit",
            format!(
                "projected WAL size {} exceeds max_wal_hard_bytes ({max_wal_hard_bytes})",
                wal.next_offset.saturating_add(projected_bytes)
            ),
        ));
    }

    let first_frame_seq = wal.last_frame_seq + 1;
    let mut cur_seq = wal.last_frame_seq;
    let mut cur_checksum = wal.last_checksum;
    let mut cur_offset = wal.next_offset;

    let mut batch = Vec::with_capacity(projected_bytes as usize);
    let mut frame_locations = Vec::with_capacity(dirty_pages.len());

    for (&pid, page_bytes) in dirty_pages {
        cur_seq += 1;
        let mut fhdr = WalFrameHeader {
            frame_seq: cur_seq,
            tx_id,
            logical_page_id: pid.raw(),
            frame_kind: WalFrameKind::FullPage,
            payload_byte_len: page_bytes.len() as u32,
            salt1: wal.salt1,
            cumulative_checksum: 0,
        };
        let mut fhdr_bytes = [0u8; WAL_FRAME_HEADER_SIZE];
        fhdr.encode(page_bytes, cur_checksum, &mut fhdr_bytes)?;
        cur_checksum = fhdr.cumulative_checksum;

        let payload_offset = cur_offset + WAL_FRAME_HEADER_SIZE as u64;
        batch.extend_from_slice(&fhdr_bytes);
        batch.extend_from_slice(page_bytes);
        frame_locations.push(WalFrameLocation {
            page_id: pid,
            frame_seq: cur_seq,
            payload_offset,
        });
        cur_offset += (WAL_FRAME_HEADER_SIZE + page_bytes.len()) as u64;
    }

    // Emit CommitFrame.
    cur_seq += 1;
    let commit_payload = CommitPayload {
        first_frame_seq: if dirty_pages.is_empty() {
            cur_seq
        } else {
            first_frame_seq
        },
        mutation_frame_count: dirty_pages.len() as u64,
        db_total_pages: meta.total_page_count,
        catalog_root: meta.catalog_root_page_id.raw(),
        freelist_root: meta.freelist_trunk_page_id,
        freelist_page_count: meta.freelist_page_count,
    };
    let mut cbuf = [0u8; WAL_COMMIT_PAYLOAD_SIZE];
    commit_payload.encode(&mut cbuf);

    let mut cfhdr = WalFrameHeader {
        frame_seq: cur_seq,
        tx_id,
        logical_page_id: 0,
        frame_kind: WalFrameKind::Commit,
        payload_byte_len: WAL_COMMIT_PAYLOAD_SIZE as u32,
        salt1: wal.salt1,
        cumulative_checksum: 0,
    };
    let mut cfhdr_bytes = [0u8; WAL_FRAME_HEADER_SIZE];
    cfhdr.encode(&cbuf, cur_checksum, &mut cfhdr_bytes)?;
    cur_checksum = cfhdr.cumulative_checksum;

    batch.extend_from_slice(&cfhdr_bytes);
    batch.extend_from_slice(&cbuf);
    cur_offset += (WAL_FRAME_HEADER_SIZE + WAL_COMMIT_PAYLOAD_SIZE) as u64;

    // Write + sync: any failure here leaves commit durability uncertain -> CommitOutcomeUnknown.
    if let Err(err) = wal_file.write_all_at(&batch, wal.next_offset) {
        return Err(ErrorEnvelope::new(
            ErrorCategory::CommitOutcomeUnknown,
            MODULE_ID,
            "publish::append_and_sync_wal_commit",
            format!("WAL commit write failed; outcome uncertain: {err}"),
        ));
    }

    let sync_res = match sync_mode {
        SyncMode::Full => wal_file.sync_all(),
        SyncMode::Normal => wal_file.sync_data(),
        SyncMode::Off => Ok(()),
    };
    if let Err(err) = sync_res {
        return Err(ErrorEnvelope::new(
            ErrorCategory::CommitOutcomeUnknown,
            MODULE_ID,
            "publish::append_and_sync_wal_commit",
            format!("WAL commit sync failed; outcome uncertain: {err}"),
        ));
    }

    wal.next_offset = cur_offset;
    wal.last_frame_seq = cur_seq;
    wal.last_checksum = cur_checksum;

    Ok((
        CommitReceipt {
            tx_id,
            wal_end_frame_seq: cur_seq,
        },
        frame_locations,
    ))
}

/// Installs newly synced WAL page versions into [`VersionIndex`] and [`PageCache`].
pub(crate) fn install_committed_versions(
    versions: &mut VersionIndex,
    cache: &PageCache,
    dirty_pages: &BTreeMap<PageId, Vec<u8>>,
    frame_locations: &[WalFrameLocation],
    tx_id: TxId,
) -> Result<()> {
    for loc in frame_locations {
        versions.record_version(
            loc.page_id,
            PageVersionEntry {
                tx_id,
                location: PageVersionLocation::WalFrame {
                    frame_seq: loc.frame_seq,
                    file_offset: loc.payload_offset,
                },
            },
        )?;
        if let Some(bytes) = dirty_pages.get(&loc.page_id) {
            let arc: Arc<[u8]> = Arc::from(bytes.as_slice());
            let _ = cache.insert(loc.page_id, tx_id, arc);
        }
    }
    Ok(())
}
