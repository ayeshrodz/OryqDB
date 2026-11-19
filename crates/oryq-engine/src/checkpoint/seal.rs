//! Durable sealed <-> unsealed transitions (`wal_required = 0 <-> 1`) and WAL header creation
//! (`docs/transactions.md` §4.1.1).

use std::path::Path;
use std::sync::Arc;

use oryq_types::Result;

use crate::config::SyncMode;
use crate::format::{WalHeader, WAL_HEADER_SIZE};
use crate::io::{FileHandle, OpenMode, StorageIo};
use crate::pager::Pager;

/// Writes a fresh 64-byte [`WalHeader`] at offset 0 of `wal_file`, truncating any old suffix
/// to 64 bytes, and syncs the WAL file.
pub fn write_fresh_wal_header(
    wal_file: &dyn FileHandle,
    wal_header: &mut WalHeader,
    sync_mode: SyncMode,
) -> Result<()> {
    let mut buf = [0u8; WAL_HEADER_SIZE];
    wal_header.encode(&mut buf);
    wal_file.set_len(WAL_HEADER_SIZE as u64)?;
    wal_file.write_all_at(&buf, 0)?;
    if sync_mode != SyncMode::Off {
        wal_file.sync_all()?;
    }
    Ok(())
}

/// Performs the normative unseal protocol before the first post-seal write transaction:
/// 1. Ensures `.oryq-wal` exists with a synced valid [`WalHeader`] matching Page 0's
///    `checkpointed_wal_seq` and `database_uuid` (+ parent dir fsync).
/// 2. Durably publishes `wal_required = true` in **both** Page 0 shadow header slots
///    (two separate increasing generations, syncing each) BEFORE any mutation frame is appended.
pub fn unseal_for_first_write(
    pager: &mut Pager,
    io: &Arc<dyn StorageIo>,
    wal_path: &Path,
    sync_mode: SyncMode,
    salt1: u64,
    salt2: u64,
) -> Result<(Arc<dyn FileHandle>, WalHeader)> {
    let wal_file = io.open(wal_path, OpenMode::CREATE_OR_OPEN)?;
    let mut wal_header = WalHeader::new(
        pager.page_size(),
        pager.header().checkpointed_wal_seq,
        pager.header().database_uuid,
        salt1,
        salt2,
    );
    write_fresh_wal_header(wal_file.as_ref(), &mut wal_header, sync_mode)?;
    if sync_mode != SyncMode::Off {
        io.sync_parent_dir(wal_path)?;
    }

    // Publish `wal_required = true` in BOTH Page 0 shadow slots (separate generations, sync each).
    pager.header_mut().wal_required = true;
    pager.commit_header()?;
    pager.commit_header()?;

    Ok((wal_file, wal_header))
}
