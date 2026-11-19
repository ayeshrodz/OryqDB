//! Bounded `.oryq-wal` scanner enforcing chained `XXH3_64` integrity, incomplete-tail
//! truncation rules, and fail-closed corruption detection (`docs/transactions.md` §4.1).

use oryq_types::{ErrorCategory, ErrorEnvelope, PageId, Result, TxId};

use crate::config::PageSize;
use crate::format::{
    CommitPayload, PageHeader, WalFrameHeader, WalFrameKind, WalHeader, WAL_COMMIT_PAYLOAD_SIZE,
    WAL_FRAME_HEADER_SIZE, WAL_HEADER_SIZE,
};
use crate::io::FileHandle;

const MODULE_ID: &str = "engine";

/// One validated committed page frame discovered in `.oryq-wal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveredPageFrame {
    pub page_id: PageId,
    pub frame_seq: u64,
    pub payload_offset: u64,
}

/// One complete committed transaction discovered in `.oryq-wal`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredTx {
    pub tx_id: TxId,
    pub commit: CommitPayload,
    pub pages: Vec<RecoveredPageFrame>,
}

/// Summary of scanning `.oryq-wal` from offset 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalScanResult {
    pub wal_header: WalHeader,
    pub committed_txs: Vec<RecoveredTx>,
    /// Byte offset immediately after the last valid committed `CommitFrame` (or 64 if none).
    pub last_valid_offset: u64,
    /// Sequence number of the last committed frame (0 if none).
    pub last_frame_seq: u64,
    /// Cumulative checksum at `last_valid_offset` (`wal_header.header_checksum` if none).
    pub last_checksum: u64,
    /// True if trailing uncommitted or incomplete bytes were observed after `last_valid_offset`.
    pub has_uncommitted_tail: bool,
}

/// Scans `.oryq-wal`, validating header UUID/sequence/page_size and every chained frame.
///
/// - Incomplete trailing frame (EOF mid-header or mid-payload) or uncommitted frames without
///   a terminating `CommitFrame` are discarded (`has_uncommitted_tail = true`).
/// - A complete frame with an invalid checksum, salt mismatch, or bad page checksum fails
///   closed with [`ErrorCategory::Corruption`].
pub fn scan_wal(
    wal_file: &dyn FileHandle,
    expected_uuid: [u8; 16],
    expected_page_size: PageSize,
    expected_wal_seq: u64,
) -> Result<WalScanResult> {
    let file_len = wal_file.len()?;
    if file_len < WAL_HEADER_SIZE as u64 {
        return Err(corrupt(
            "recovery::scan_wal",
            format!("WAL file truncated before 64-byte header ({file_len} bytes)"),
        ));
    }

    let mut hdr_buf = [0u8; WAL_HEADER_SIZE];
    wal_file.read_exact_at(&mut hdr_buf, 0)?;
    let wal_header = WalHeader::decode(&hdr_buf)?;

    if wal_header.database_uuid != expected_uuid {
        return Err(corrupt(
            "recovery::scan_wal",
            "WAL database_uuid does not match .oryq Page 0 database_uuid",
        ));
    }
    if wal_header.page_size != expected_page_size {
        return Err(corrupt(
            "recovery::scan_wal",
            format!(
                "WAL page_size {:?} does not match .oryq page_size {:?}",
                wal_header.page_size, expected_page_size
            ),
        ));
    }
    if wal_header.checkpoint_seq != expected_wal_seq {
        return Err(corrupt(
            "recovery::scan_wal",
            format!(
                "WAL checkpoint_seq {} does not match expected {}",
                wal_header.checkpoint_seq, expected_wal_seq
            ),
        ));
    }

    let page_bytes = expected_page_size.bytes();
    let max_frame_payload = page_bytes.max(WAL_COMMIT_PAYLOAD_SIZE);

    let mut offset = WAL_HEADER_SIZE as u64;
    let mut prev_checksum = wal_header.header_checksum;
    let mut expected_frame_seq = 1u64;
    let mut last_committed_tx_id: Option<TxId> = None;

    let mut committed_txs = Vec::new();
    let mut last_valid_offset = WAL_HEADER_SIZE as u64;
    let mut last_frame_seq = 0u64;
    let mut last_checksum = wal_header.header_checksum;

    let mut pending_tx_id: Option<TxId> = None;
    let mut pending_first_seq: u64 = 0;
    let mut pending_pages: Vec<RecoveredPageFrame> = Vec::new();
    let mut has_uncommitted_tail = false;

    while offset < file_len {
        let remaining = file_len - offset;
        if remaining < WAL_FRAME_HEADER_SIZE as u64 {
            // Incomplete final frame header -> uncommitted tail.
            has_uncommitted_tail = true;
            break;
        }

        let mut fhdr_buf = [0u8; WAL_FRAME_HEADER_SIZE];
        wal_file.read_exact_at(&mut fhdr_buf, offset)?;

        let declared_len = u32::from_le_bytes(fhdr_buf[28..32].try_into().unwrap()) as usize;
        if declared_len > max_frame_payload {
            // Complete header declaring an impossible payload length -> corruption.
            return Err(corrupt(
                "recovery::scan_wal",
                format!(
                    "WAL frame at offset {offset} declares oversized payload {declared_len} > {max_frame_payload}"
                ),
            ));
        }

        let payload_offset = offset + WAL_FRAME_HEADER_SIZE as u64;
        let frame_end = payload_offset + declared_len as u64;
        if frame_end > file_len {
            // Incomplete final frame payload -> uncommitted tail.
            has_uncommitted_tail = true;
            break;
        }

        let mut payload = vec![0u8; declared_len];
        wal_file.read_exact_at(&mut payload, payload_offset)?;

        let fhdr = WalFrameHeader::decode_and_verify(&fhdr_buf, &payload, prev_checksum)?;

        if fhdr.salt1 != wal_header.salt1 {
            return Err(corrupt(
                "recovery::scan_wal",
                format!(
                    "WAL frame {} salt1 mismatch: got {}, expected {}",
                    fhdr.frame_seq, fhdr.salt1, wal_header.salt1
                ),
            ));
        }
        if fhdr.frame_seq != expected_frame_seq {
            return Err(corrupt(
                "recovery::scan_wal",
                format!(
                    "non-contiguous WAL frame_seq: got {}, expected {expected_frame_seq}",
                    fhdr.frame_seq
                ),
            ));
        }

        match fhdr.frame_kind {
            WalFrameKind::FullPage => {
                if declared_len != page_bytes {
                    return Err(corrupt(
                        "recovery::scan_wal",
                        format!(
                            "FullPageFrame payload len {declared_len} != page_size {page_bytes}"
                        ),
                    ));
                }
                if fhdr.logical_page_id == 0 {
                    return Err(corrupt(
                        "recovery::scan_wal",
                        "FullPageFrame cannot reference Page 0",
                    ));
                }
                let pid = PageId::new(fhdr.logical_page_id);
                let phdr = PageHeader::decode_and_verify(&payload, pid)?;
                if phdr.lsn_tx_id != fhdr.tx_id {
                    return Err(corrupt(
                        "recovery::scan_wal",
                        format!(
                            "FullPageFrame page lsn_tx_id {} != frame tx_id {}",
                            phdr.lsn_tx_id.raw(),
                            fhdr.tx_id.raw()
                        ),
                    ));
                }

                match pending_tx_id {
                    None => {
                        if let Some(prev_tx) = last_committed_tx_id {
                            if fhdr.tx_id <= prev_tx {
                                return Err(corrupt(
                                    "recovery::scan_wal",
                                    "WAL transaction IDs must strictly increase",
                                ));
                            }
                        }
                        pending_tx_id = Some(fhdr.tx_id);
                        pending_first_seq = fhdr.frame_seq;
                    }
                    Some(cur) if cur != fhdr.tx_id => {
                        return Err(corrupt(
                            "recovery::scan_wal",
                            "interleaved WAL frames from different transactions before CommitFrame",
                        ));
                    }
                    Some(_) => {}
                }

                pending_pages.push(RecoveredPageFrame {
                    page_id: pid,
                    frame_seq: fhdr.frame_seq,
                    payload_offset,
                });
            }
            WalFrameKind::SlotDelta => {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::UnsupportedFeature,
                    MODULE_ID,
                    "recovery::scan_wal",
                    "SlotDeltaFrame replay is scheduled for W12",
                ));
            }
            WalFrameKind::Commit => {
                if declared_len != WAL_COMMIT_PAYLOAD_SIZE {
                    return Err(corrupt(
                        "recovery::scan_wal",
                        format!(
                            "CommitFrame payload len {declared_len} != {WAL_COMMIT_PAYLOAD_SIZE}"
                        ),
                    ));
                }
                let commit = CommitPayload::decode(payload.as_slice().try_into().unwrap());
                if let Some(cur) = pending_tx_id {
                    if cur != fhdr.tx_id {
                        return Err(corrupt(
                            "recovery::scan_wal",
                            "CommitFrame tx_id does not match pending mutation frames",
                        ));
                    }
                    if commit.first_frame_seq != pending_first_seq
                        || commit.mutation_frame_count != pending_pages.len() as u64
                    {
                        return Err(corrupt(
                            "recovery::scan_wal",
                            "CommitFrame sequence/count metadata mismatch",
                        ));
                    }
                } else if commit.mutation_frame_count != 0 {
                    return Err(corrupt(
                        "recovery::scan_wal",
                        "CommitFrame declares mutations with no preceding mutation frames",
                    ));
                } else if let Some(prev_tx) = last_committed_tx_id {
                    if fhdr.tx_id <= prev_tx {
                        return Err(corrupt(
                            "recovery::scan_wal",
                            "WAL transaction IDs must strictly increase",
                        ));
                    }
                }

                committed_txs.push(RecoveredTx {
                    tx_id: fhdr.tx_id,
                    commit,
                    pages: core::mem::take(&mut pending_pages),
                });
                pending_tx_id = None;
                last_committed_tx_id = Some(fhdr.tx_id);
                last_valid_offset = frame_end;
                last_frame_seq = fhdr.frame_seq;
                last_checksum = fhdr.cumulative_checksum;
            }
        }

        prev_checksum = fhdr.cumulative_checksum;
        expected_frame_seq += 1;
        offset = frame_end;
    }

    if !pending_pages.is_empty() || pending_tx_id.is_some() {
        has_uncommitted_tail = true;
    }

    Ok(WalScanResult {
        wal_header,
        committed_txs,
        last_valid_offset,
        last_frame_seq,
        last_checksum,
        has_uncommitted_tail,
    })
}

fn corrupt(op: &'static str, msg: impl Into<String>) -> ErrorEnvelope {
    ErrorEnvelope::new(ErrorCategory::Corruption, MODULE_ID, op, msg)
}
