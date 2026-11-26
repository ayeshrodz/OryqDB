//! Redundant `reset_intent_seq` WAL reset protocol and interrupted-reset recovery
//! (`docs/transactions.md` §4.1 step 3).

use std::path::Path;
use std::sync::Arc;

use oryq_types::Result;

use crate::checkpoint::seal::write_fresh_wal_header;
use crate::config::SyncMode;
use crate::format::WalHeader;
use crate::io::{FileHandle, OpenMode, StorageIo};
use crate::pager::Pager;

/// Executes the durable WAL reset protocol after a full checkpoint through `latest_committed`
/// while reader admission is blocked and zero active snapshots remain:
///
/// 1. Durably publish `reset_intent_seq = current_seq + 1` in BOTH Page 0 shadow slots (sync each).
/// 2. Truncate `.oryq-wal` to 64 bytes with the new `WalHeader` (`checkpoint_seq = next_seq`,
///    incremented `salt1`, new `salt2`) and sync.
/// 3. Durably clear `reset_intent_seq = 0`, set `checkpointed_wal_seq = next_seq`, and
///    `wal_required = false` (sealed) in BOTH Page 0 shadow slots (sync each).
pub fn reset_wal_after_full_checkpoint(
    pager: &mut Pager,
    wal_file: &dyn FileHandle,
    sync_mode: SyncMode,
    next_salt1: u64,
    next_salt2: u64,
) -> Result<WalHeader> {
    let next_seq = pager.header().checkpointed_wal_seq + 1;

    // Pre-flight data sync BEFORE mutating `reset_intent_seq` in memory so a pre-header sync
    // fault leaves `pager.header()` completely untouched.
    pager.sync_data()?;

    // Step 1: Durably publish reset_intent_seq in both shadow slots.
    pager.header_mut().reset_intent_seq = next_seq;
    pager.commit_header()?;
    pager.commit_header()?;

    // Step 2: Truncate WAL to 64B and write/sync new WalHeader.
    let mut new_wal_hdr = WalHeader::new(
        pager.page_size(),
        next_seq,
        pager.header().database_uuid,
        next_salt1,
        next_salt2,
    );
    write_fresh_wal_header(wal_file, &mut new_wal_hdr, sync_mode)?;

    // Step 3: Clear reset_intent_seq, advance checkpointed_wal_seq, and seal (`wal_required=0`) in both slots.
    let hdr = pager.header_mut();
    hdr.reset_intent_seq = 0;
    hdr.checkpointed_wal_seq = next_seq;
    hdr.wal_required = false;
    pager.commit_header()?;
    pager.commit_header()?;

    Ok(new_wal_hdr)
}

/// Completes a WAL reset that was interrupted by a crash after `reset_intent_seq != 0` was
/// durably published. All committed state is already in `.oryq`, so no WAL replay is performed.
pub fn complete_interrupted_reset(
    pager: &mut Pager,
    io: &Arc<dyn StorageIo>,
    wal_path: &Path,
    sync_mode: SyncMode,
    next_salt1: u64,
    next_salt2: u64,
) -> Result<WalHeader> {
    let target_seq = pager.header().reset_intent_seq;
    let wal_file = io.open(wal_path, OpenMode::CREATE_OR_OPEN)?;

    let mut new_wal_hdr = WalHeader::new(
        pager.page_size(),
        target_seq,
        pager.header().database_uuid,
        next_salt1,
        next_salt2,
    );
    write_fresh_wal_header(wal_file.as_ref(), &mut new_wal_hdr, sync_mode)?;
    if sync_mode != SyncMode::Off {
        io.sync_parent_dir(wal_path)?;
    }

    let hdr = pager.header_mut();
    hdr.reset_intent_seq = 0;
    hdr.checkpointed_wal_seq = target_seq;
    hdr.wal_required = false;
    pager.commit_header()?;
    pager.commit_header()?;

    Ok(new_wal_hdr)
}
