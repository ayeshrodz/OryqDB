//! Transactional MVCC [`StorageEngine`] coordinating `.oryq`, `.oryq-wal`,
//! `PageCache`, `VersionIndex`, `SingleProcessCoordinator`, recovery, and checkpointing.

pub mod batch;
pub mod publish;
pub mod read;
pub mod savepoint;
pub mod write;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use oryq_types::{ErrorCategory, ErrorEnvelope, PageId, Result, SnapshotId, TxId};

use crate::cache::{PageCache, PageVersionLocation, VersionIndex};
use crate::checkpoint::{
    complete_interrupted_reset, copy_committed_pages_to_main, reset_wal_after_full_checkpoint,
    unseal_for_first_write,
};
use crate::config::{EngineConfig, PageSize};
use crate::coordinator::SingleProcessCoordinator;
use crate::format::{PageHeader, WAL_HEADER_SIZE};
use crate::io::{OpenMode, StorageIo};
use crate::pager::Pager;
use crate::ports::{CheckpointOutcome, CommitReceipt};
use crate::recovery::{replay_into_version_index, scan_wal, CommittedDbMeta};
use crate::transaction::publish::{
    append_and_sync_wal_commit, install_committed_versions, WalAppendState,
};

pub use batch::{
    clone_error_envelope, commit_batch, BatchCommitReport, BatchItem, BatchTx, EngineWriteBatchTx,
    GroupCommitConfig, GroupCommitQueue,
};
pub use read::ReadTxn;
pub use savepoint::SavepointId;
pub use write::WriteTxn;

const MODULE_ID: &str = "engine";

#[derive(Debug)]
struct EngineState {
    pager: Pager,
    wal: WalAppendState,
    versions: VersionIndex,
    /// Pages that have at least one committed WAL version in the current cycle.
    wal_dirty_pages: HashSet<PageId>,
    /// Committed metadata history ordered by `TxId` for snapshot-consistent reads and checkpoints.
    meta_history: BTreeMap<TxId, CommittedDbMeta>,
    salt_counter: u64,
}

/// Physical transactional storage engine supporting single-process MVCC snapshots,
/// serialized durable `.oryq-wal` commits, nested savepoints, checkpoint/reset, and
/// restartable crash recovery.
#[derive(Debug)]
pub struct StorageEngine {
    io: Arc<dyn StorageIo>,
    wal_path: PathBuf,
    config: EngineConfig,
    page_size: PageSize,
    cache: PageCache,
    coordinator: SingleProcessCoordinator,
    state: Mutex<EngineState>,
}

/// Derives the canonical `.oryq-wal` sidecar path for a database path.
pub fn wal_path_for(db_path: &Path) -> PathBuf {
    let mut os = db_path.as_os_str().to_os_string();
    os.push("-wal");
    PathBuf::from(os)
}

impl StorageEngine {
    /// Opens an existing database (running deterministic crash recovery if needed) or
    /// creates a new sealed database if `config.create_if_missing` is true.
    pub fn open(
        io: Arc<dyn StorageIo>,
        db_path: &Path,
        config: EngineConfig,
        database_uuid: [u8; 16],
    ) -> Result<Self> {
        let wal_path = wal_path_for(db_path);
        let mut pager = Pager::open_or_create(io.clone(), db_path, &config, database_uuid)?;
        let page_size = pager.page_size();
        let mut versions = VersionIndex::new(config.max_version_entries);
        let mut wal_dirty_pages = HashSet::new();
        let mut meta_history = BTreeMap::new();

        let base_meta = CommittedDbMeta::from_header(pager.header());
        meta_history.insert(base_meta.latest_tx_id, base_meta);

        let mut salt_counter = 1u64;
        let wal_state;
        let latest_committed_tx_id;

        if pager.header().reset_intent_seq != 0 {
            // Interrupted WAL reset: all committed data is already durably in `.oryq`.
            let new_hdr = complete_interrupted_reset(
                &mut pager,
                &io,
                &wal_path,
                config.sync_mode,
                salt_counter,
                0x51A7_0000_0000_0001,
            )?;
            salt_counter += 1;
            wal_state = WalAppendState {
                file: None,
                salt1: new_hdr.salt1,
                next_offset: WAL_HEADER_SIZE as u64,
                last_frame_seq: 0,
                last_checksum: new_hdr.header_checksum,
            };
            latest_committed_tx_id = base_meta.latest_tx_id;
        } else if !pager.header().wal_required {
            // Sealed database: missing WAL is permitted; present WAL must still validate if non-empty.
            if io.exists(&wal_path) {
                let wf = io.open(&wal_path, OpenMode::READ_WRITE)?;
                let scan = scan_wal(
                    wf.as_ref(),
                    pager.header().database_uuid,
                    page_size,
                    pager.header().checkpointed_wal_seq,
                )?;
                if scan.has_uncommitted_tail && !config.read_only {
                    wf.set_len(scan.last_valid_offset)?;
                    if config.sync_mode != crate::config::SyncMode::Off {
                        wf.sync_all()?;
                    }
                }
                let final_meta = replay_into_version_index(pager.header(), &scan, &mut versions)?;
                for tx in &scan.committed_txs {
                    if tx.tx_id > pager.header().checkpointed_tx_id {
                        for pf in &tx.pages {
                            wal_dirty_pages.insert(pf.page_id);
                        }
                        meta_history.insert(
                            tx.tx_id,
                            CommittedDbMeta {
                                latest_tx_id: tx.tx_id,
                                total_page_count: tx.commit.db_total_pages,
                                catalog_root_page_id: PageId::new(tx.commit.catalog_root),
                                freelist_trunk_page_id: tx.commit.freelist_root,
                                freelist_page_count: tx.commit.freelist_page_count,
                            },
                        );
                    }
                }
                salt_counter = scan.wal_header.salt1.saturating_add(1);
                latest_committed_tx_id = final_meta.latest_tx_id;
                wal_state = WalAppendState {
                    file: Some(wf),
                    salt1: scan.wal_header.salt1,
                    next_offset: scan.last_valid_offset,
                    last_frame_seq: scan.last_frame_seq,
                    last_checksum: scan.last_checksum,
                };
            } else {
                latest_committed_tx_id = base_meta.latest_tx_id;
                wal_state = WalAppendState {
                    file: None,
                    salt1: 0,
                    next_offset: WAL_HEADER_SIZE as u64,
                    last_frame_seq: 0,
                    last_checksum: 0,
                };
            }
        } else {
            // Unsealed (`wal_required = true`): WAL MUST exist and validate.
            if !io.exists(&wal_path) {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "StorageEngine::open",
                    "database header requires .oryq-wal (wal_required=1) but WAL sidecar is missing",
                ));
            }
            let mode = if config.read_only {
                OpenMode::READ_ONLY
            } else {
                OpenMode::READ_WRITE
            };
            let wf = io.open(&wal_path, mode)?;
            let scan = scan_wal(
                wf.as_ref(),
                pager.header().database_uuid,
                page_size,
                pager.header().checkpointed_wal_seq,
            )?;
            if scan.has_uncommitted_tail && !config.read_only {
                wf.set_len(scan.last_valid_offset)?;
                if config.sync_mode != crate::config::SyncMode::Off {
                    wf.sync_all()?;
                }
            }
            let final_meta = replay_into_version_index(pager.header(), &scan, &mut versions)?;
            for tx in &scan.committed_txs {
                if tx.tx_id > pager.header().checkpointed_tx_id {
                    for pf in &tx.pages {
                        wal_dirty_pages.insert(pf.page_id);
                    }
                    meta_history.insert(
                        tx.tx_id,
                        CommittedDbMeta {
                            latest_tx_id: tx.tx_id,
                            total_page_count: tx.commit.db_total_pages,
                            catalog_root_page_id: PageId::new(tx.commit.catalog_root),
                            freelist_trunk_page_id: tx.commit.freelist_root,
                            freelist_page_count: tx.commit.freelist_page_count,
                        },
                    );
                }
            }
            salt_counter = scan.wal_header.salt1.saturating_add(1);
            latest_committed_tx_id = final_meta.latest_tx_id;
            wal_state = WalAppendState {
                file: Some(wf),
                salt1: scan.wal_header.salt1,
                next_offset: scan.last_valid_offset,
                last_frame_seq: scan.last_frame_seq,
                last_checksum: scan.last_checksum,
            };
        }

        let cache = PageCache::with_limit(config.max_cache_bytes);
        let coordinator = SingleProcessCoordinator::new(latest_committed_tx_id, config.max_readers);

        Ok(Self {
            io,
            wal_path,
            config,
            page_size,
            cache,
            coordinator,
            state: Mutex::new(EngineState {
                pager,
                wal: wal_state,
                versions,
                wal_dirty_pages,
                meta_history,
                salt_counter,
            }),
        })
    }

    #[inline]
    pub fn page_size(&self) -> PageSize {
        self.page_size
    }

    pub fn latest_committed_tx_id(&self) -> Result<TxId> {
        self.coordinator.latest_committed()
    }

    pub fn is_poisoned(&self) -> bool {
        self.coordinator.is_poisoned()
    }

    pub fn is_read_only(&self) -> bool {
        self.config.read_only
    }

    /// Executes an ordered batch of write closures inside a single [`WriteTxn`] and a single
    /// WAL fsync, rolling back only failing items via statement savepoints (`X03`).
    pub fn commit_batch<T, F>(&self, items: Vec<F>) -> Result<BatchCommitReport<T>>
    where
        F: FnOnce(&mut WriteTxn<'_>) -> Result<T>,
    {
        commit_batch(self, items)
    }

    pub fn is_sealed(&self) -> Result<bool> {
        let st = self.lock_state()?;
        Ok(!st.pager.header().wal_required)
    }

    /// Starts a snapshot-isolated [`ReadTxn`].
    pub fn begin_read(&self) -> Result<ReadTxn<'_>> {
        let guard = self.coordinator.acquire_read_snapshot()?;
        let meta = {
            let st = self.lock_state()?;
            meta_at_snapshot(&st.meta_history, guard.snapshot_id())
        };
        Ok(ReadTxn::new(self, guard, meta))
    }

    /// Starts an exclusive [`WriteTxn`], blocking if another write transaction is active.
    pub fn begin_write(&self) -> Result<WriteTxn<'_>> {
        if self.config.read_only {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "StorageEngine::begin_write",
                "cannot start WriteTxn on a read-only StorageEngine",
            ));
        }
        let permit = self.coordinator.acquire_writer()?;
        let base_meta = {
            let st = self.lock_state()?;
            meta_at_snapshot(&st.meta_history, permit.base_tx_id())
        };
        WriteTxn::new(self, permit, base_meta)
    }

    /// Attempts to start an exclusive [`WriteTxn`] without blocking (returns `Busy` if contended).
    pub fn try_begin_write(&self) -> Result<WriteTxn<'_>> {
        if self.config.read_only {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "StorageEngine::try_begin_write",
                "cannot start WriteTxn on a read-only StorageEngine",
            ));
        }
        let permit = self.coordinator.try_acquire_writer()?;
        let base_meta = {
            let st = self.lock_state()?;
            meta_at_snapshot(&st.meta_history, permit.base_tx_id())
        };
        WriteTxn::new(self, permit, base_meta)
    }

    /// Checkpoints committed WAL pages up to the active snapshot horizon into `.oryq`.
    ///
    /// If `allow_reset` is true and zero active readers remain, performs the full
    /// `reset_intent_seq` WAL reset and seals the database (`wal_required = false`).
    pub fn checkpoint(&self, allow_reset: bool) -> Result<CheckpointOutcome> {
        if self.config.read_only {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "StorageEngine::checkpoint",
                "cannot checkpoint a read-only StorageEngine",
            ));
        }
        let _writer_permit = self.coordinator.acquire_writer()?;
        let horizon = self.coordinator.retention_horizon()?;
        let latest = self.coordinator.latest_committed()?;

        let mut st_guard = self.lock_state()?;
        let st = &mut *st_guard;
        let dirty_list: Vec<PageId> = st.wal_dirty_pages.iter().copied().collect();
        let meta_at_horizon = meta_at_snapshot(&st.meta_history, horizon);

        let pages_copied = if let Some(wf) = st.wal.file.clone() {
            if horizon > st.pager.header().checkpointed_tx_id {
                copy_committed_pages_to_main(
                    &mut st.pager,
                    wf.as_ref(),
                    &st.versions,
                    &dirty_list,
                    horizon,
                    meta_at_horizon,
                )?
            } else {
                0
            }
        } else {
            0
        };

        // Prune superseded versions below horizon.
        st.versions.prune_below(horizon);

        // Attempt full WAL reset if requested, checkpointed through latest, and zero active readers.
        let mut wal_reset = false;
        if allow_reset
            && horizon == latest
            && st.pager.header().wal_required
            && self.coordinator.try_block_admission_when_idle()?
        {
            let reset_res = (|| -> Result<()> {
                let wf = st.wal.file.clone().ok_or_else(|| {
                    ErrorEnvelope::new(
                        ErrorCategory::Internal,
                        MODULE_ID,
                        "StorageEngine::checkpoint",
                        "WAL file missing during reset",
                    )
                })?;
                let next_salt1 = st.salt_counter;
                st.salt_counter += 1;
                let next_salt2 = 0x51A7_0000_0000_0000u64 ^ next_salt1.wrapping_mul(0x9E37_79B9);
                let new_hdr = reset_wal_after_full_checkpoint(
                    &mut st.pager,
                    wf.as_ref(),
                    self.config.sync_mode,
                    next_salt1,
                    next_salt2,
                )?;
                st.wal.salt1 = new_hdr.salt1;
                st.wal.next_offset = WAL_HEADER_SIZE as u64;
                st.wal.last_frame_seq = 0;
                st.wal.last_checksum = new_hdr.header_checksum;
                st.versions = VersionIndex::new(self.config.max_version_entries);
                st.wal_dirty_pages.clear();
                st.meta_history.retain(|&tx, _| tx == latest);
                Ok(())
            })();
            self.coordinator.unblock_admission();
            reset_res?;
            wal_reset = true;
        }

        Ok(CheckpointOutcome {
            checkpointed_tx_id: horizon,
            pages_copied,
            wal_reset,
        })
    }

    pub(crate) fn read_page_at_snapshot(
        &self,
        page_id: PageId,
        snapshot_id: SnapshotId,
    ) -> Result<Arc<[u8]>> {
        let st = self.lock_state()?;
        if let Some(entry) = st.versions.resolve_for_snapshot(page_id, snapshot_id) {
            if let Some(hit) = self.cache.get(page_id, entry.tx_id) {
                return Ok(hit.as_arc());
            }
            if let PageVersionLocation::WalFrame { file_offset, .. } = entry.location {
                let wf = st.wal.file.as_ref().ok_or_else(|| {
                    ErrorEnvelope::new(
                        ErrorCategory::Corruption,
                        MODULE_ID,
                        "StorageEngine::read_page_at_snapshot",
                        "WAL handle missing for WAL-resident page version",
                    )
                })?;
                let mut buf = vec![0u8; self.page_size.bytes()];
                wf.read_exact_at(&mut buf, file_offset)?;
                let _ = PageHeader::decode_and_verify(&buf, page_id)?;
                let arc: Arc<[u8]> = Arc::from(buf.into_boxed_slice());
                let _ = self.cache.insert(page_id, entry.tx_id, Arc::clone(&arc));
                return Ok(arc);
            }
        }

        // Fall back to main `.oryq` file.
        let guard = st.pager.read_page(page_id, None)?;
        let hdr = PageHeader::decode_and_verify(guard.as_slice(), page_id)?;
        if let Some(hit) = self.cache.get(page_id, hdr.lsn_tx_id) {
            return Ok(hit.as_arc());
        }
        let arc: Arc<[u8]> = Arc::from(guard.as_slice());
        let _ = self.cache.insert(page_id, hdr.lsn_tx_id, Arc::clone(&arc));
        Ok(arc)
    }

    pub(crate) fn commit_write_txn(
        &self,
        tx_id: TxId,
        dirty_pages: BTreeMap<PageId, Vec<u8>>,
        meta: CommittedDbMeta,
    ) -> Result<CommitReceipt> {
        let mut st = self.lock_state()?;

        // Unseal first if currently sealed (`wal_required == false`).
        if !st.pager.header().wal_required {
            let salt1 = st.salt_counter;
            st.salt_counter += 1;
            let salt2 = 0x51A7_0000_0000_0000u64 ^ salt1.wrapping_mul(0x9E37_79B9);
            let (wf, wal_hdr) = unseal_for_first_write(
                &mut st.pager,
                &self.io,
                &self.wal_path,
                self.config.sync_mode,
                salt1,
                salt2,
            )?;
            st.wal = WalAppendState {
                file: Some(wf),
                salt1: wal_hdr.salt1,
                next_offset: WAL_HEADER_SIZE as u64,
                last_frame_seq: 0,
                last_checksum: wal_hdr.header_checksum,
            };
        }

        let (receipt, frame_locations) = match append_and_sync_wal_commit(
            &mut st.wal,
            &dirty_pages,
            tx_id,
            meta,
            self.config.sync_mode,
            self.config.max_wal_hard_bytes,
        ) {
            Ok(ok) => ok,
            Err(err) => {
                if err.category() == ErrorCategory::CommitOutcomeUnknown {
                    self.coordinator.poison();
                }
                return Err(err);
            }
        };

        // Install all committed versions BEFORE advancing latest_committed_tx_id.
        if let Err(err) = install_committed_versions(
            &mut st.versions,
            &self.cache,
            &dirty_pages,
            &frame_locations,
            tx_id,
        ) {
            self.coordinator.poison();
            return Err(ErrorEnvelope::new(
                ErrorCategory::CommitOutcomeUnknown,
                MODULE_ID,
                "StorageEngine::commit_write_txn",
                format!("version installation failed after WAL sync; engine poisoned: {err}"),
            ));
        }

        for loc in &frame_locations {
            st.wal_dirty_pages.insert(loc.page_id);
        }
        st.meta_history.insert(tx_id, meta);

        if let Err(err) = self.coordinator.publish_commit(tx_id) {
            self.coordinator.poison();
            return Err(err);
        }

        Ok(receipt)
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, EngineState>> {
        self.state.lock().map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Internal,
                MODULE_ID,
                "StorageEngine::lock_state",
                "engine state mutex poisoned",
            )
        })
    }
}

fn meta_at_snapshot(
    history: &BTreeMap<TxId, CommittedDbMeta>,
    snapshot_id: SnapshotId,
) -> CommittedDbMeta {
    history
        .range(..=snapshot_id)
        .next_back()
        .map(|(_, &m)| m)
        .expect("meta_history always contains initial header baseline")
}
