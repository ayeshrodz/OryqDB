//! Multi-process shared-memory coordination (`.oryq-shm` / `X02`).
//!
//! Coordinates concurrent OS processes sharing an `.oryq` database using a checksummed
//! companion `.oryq-shm` state file plus OS-kernel-backed writer (`.oryq-shm.wlock`) and
//! reader/WAL-reset (`.oryq-shm.rlock`) lock files.
//!
//! # Lock Ordering (Deadlock Freedom)
//! Every operation acquires OS locks in strict total order:
//! `wlock` -> `rlock` -> `shm_file` (brief metadata critical section).
//! Because OS kernel locks are automatically released on process termination (including
//! `SIGKILL` / `TerminateProcess`), peer crashes are detected and recovered deterministically
//! without database corruption.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use fs4::fs_std::FileExt;
use oryq_types::{ErrorCategory, ErrorEnvelope, Result, SnapshotId, TxId};
use xxhash_rust::xxh3::xxh3_64;

const MODULE_ID: &str = "engine";

const SHM_MAGIC: [u8; 8] = *b"ORYQSHM1";
const SHM_VERSION: u32 = 1;
const SHM_HEADER_SIZE: usize = 64;
const SHM_SLOT_SIZE: usize = 32;

/// Maximum number of concurrent reader slots permitted in an `.oryq-shm` file.
pub const MAX_SHM_READERS: usize = 256;

const SLOT_FREE: u32 = 0;
const SLOT_ACTIVE: u32 = 1;

/// Derives the canonical `.oryq-shm` sidecar path for a database path.
pub fn shm_path_for(db_path: &Path) -> PathBuf {
    let mut os = db_path.as_os_str().to_os_string();
    os.push("-shm");
    PathBuf::from(os)
}

fn wlock_path_for(shm_path: &Path) -> PathBuf {
    let mut os = shm_path.as_os_str().to_os_string();
    os.push(".wlock");
    PathBuf::from(os)
}

fn rlock_path_for(shm_path: &Path) -> PathBuf {
    let mut os = shm_path.as_os_str().to_os_string();
    os.push(".rlock");
    PathBuf::from(os)
}

/// Configuration for [`SharedProcessCoordinator`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedProcessConfig {
    /// Maximum concurrent reader slots across all processes (`1..=256`).
    pub max_readers: usize,
    /// Lease duration after which an unrefreshed reader slot from a dead/stalled peer
    /// may be reclaimed when another live reader prevents exclusive `rlock` probing.
    pub peer_lease_timeout: Duration,
    /// Polling interval when waiting on OS file locks.
    pub lock_retry_interval: Duration,
    /// Maximum wait duration before returning `ErrorCategory::Busy` on contended locks.
    pub lock_timeout: Duration,
}

impl Default for SharedProcessConfig {
    fn default() -> Self {
        Self {
            max_readers: 64,
            peer_lease_timeout: Duration::from_secs(30),
            lock_retry_interval: Duration::from_millis(2),
            lock_timeout: Duration::from_secs(5),
        }
    }
}

/// Summary of stale peer entries reclaimed by [`SharedProcessCoordinator::reclaim_stale_peers`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StaleReclaimReport {
    pub reclaimed_readers: usize,
    pub reclaimed_writer: bool,
    pub reclaimed_wal_reset: bool,
}

/// Snapshot of `.oryq-shm` coordination state returned by [`SharedProcessCoordinator::inspect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShmInspectionReport {
    pub shm_path: PathBuf,
    pub max_readers: usize,
    pub latest_committed_tx_id: TxId,
    pub retention_horizon: SnapshotId,
    pub active_readers: usize,
    pub writer_pid: Option<u32>,
    pub wal_reset_pid: Option<u32>,
    pub wal_reset_epoch: u64,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReaderSlot {
    state: u32,
    pid: u32,
    token_id: u64,
    snapshot_tx_id: u64,
    lease_updated_ms: u64,
}

impl ReaderSlot {
    const EMPTY: Self = Self {
        state: SLOT_FREE,
        pid: 0,
        token_id: 0,
        snapshot_tx_id: 0,
        lease_updated_ms: 0,
    };

    fn is_active(&self) -> bool {
        self.state == SLOT_ACTIVE
    }

    fn encode(&self, out: &mut [u8]) {
        out[0..4].copy_from_slice(&self.state.to_le_bytes());
        out[4..8].copy_from_slice(&self.pid.to_le_bytes());
        out[8..16].copy_from_slice(&self.token_id.to_le_bytes());
        out[16..24].copy_from_slice(&self.snapshot_tx_id.to_le_bytes());
        out[24..32].copy_from_slice(&self.lease_updated_ms.to_le_bytes());
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        let state = u32::from_le_bytes(bytes[0..4].try_into().unwrap());
        if state != SLOT_FREE && state != SLOT_ACTIVE {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "ReaderSlot::decode",
                format!("invalid reader slot state {state}"),
            ));
        }
        Ok(Self {
            state,
            pid: u32::from_le_bytes(bytes[4..8].try_into().unwrap()),
            token_id: u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
            snapshot_tx_id: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            lease_updated_ms: u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
        })
    }
}

#[derive(Debug, Clone)]
struct ShmState {
    max_readers: u32,
    latest_committed_tx_id: u64,
    writer_pid: u32,
    wal_reset_pid: u32,
    writer_token: u64,
    wal_reset_epoch: u64,
    generation: u64,
    slots: Vec<ReaderSlot>,
}

impl ShmState {
    fn new(initial_committed_tx_id: TxId, max_readers: usize) -> Self {
        Self {
            max_readers: max_readers as u32,
            latest_committed_tx_id: initial_committed_tx_id.raw(),
            writer_pid: 0,
            wal_reset_pid: 0,
            writer_token: 0,
            wal_reset_epoch: 0,
            generation: 1,
            slots: vec![ReaderSlot::EMPTY; max_readers],
        }
    }

    fn encode(&self) -> Vec<u8> {
        let total = SHM_HEADER_SIZE + self.slots.len() * SHM_SLOT_SIZE;
        let mut buf = vec![0u8; total];
        buf[0..8].copy_from_slice(&SHM_MAGIC);
        buf[8..12].copy_from_slice(&SHM_VERSION.to_le_bytes());
        buf[12..16].copy_from_slice(&self.max_readers.to_le_bytes());
        buf[16..24].copy_from_slice(&self.latest_committed_tx_id.to_le_bytes());
        buf[24..28].copy_from_slice(&self.writer_pid.to_le_bytes());
        buf[28..32].copy_from_slice(&self.wal_reset_pid.to_le_bytes());
        buf[32..40].copy_from_slice(&self.writer_token.to_le_bytes());
        buf[40..48].copy_from_slice(&self.wal_reset_epoch.to_le_bytes());
        buf[48..56].copy_from_slice(&self.generation.to_le_bytes());
        for (i, slot) in self.slots.iter().enumerate() {
            let off = SHM_HEADER_SIZE + i * SHM_SLOT_SIZE;
            slot.encode(&mut buf[off..off + SHM_SLOT_SIZE]);
        }
        // Checksum covers header[0..56] + all slot bytes.
        let mut hasher_input = Vec::with_capacity(total - 8);
        hasher_input.extend_from_slice(&buf[0..56]);
        hasher_input.extend_from_slice(&buf[SHM_HEADER_SIZE..]);
        let checksum = xxh3_64(&hasher_input);
        buf[56..64].copy_from_slice(&checksum.to_le_bytes());
        buf
    }

    fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < SHM_HEADER_SIZE {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "ShmState::decode",
                format!("truncated .oryq-shm header ({} bytes)", bytes.len()),
            ));
        }
        if bytes[0..8] != SHM_MAGIC {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "ShmState::decode",
                "invalid .oryq-shm magic bytes",
            ));
        }
        let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
        if version != SHM_VERSION {
            return Err(ErrorEnvelope::new(
                ErrorCategory::UnsupportedFeature,
                MODULE_ID,
                "ShmState::decode",
                format!("unsupported .oryq-shm version {version}"),
            ));
        }
        let max_readers = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
        if max_readers == 0 || (max_readers as usize) > MAX_SHM_READERS {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "ShmState::decode",
                format!("invalid .oryq-shm max_readers {max_readers}"),
            ));
        }
        let expected_len = SHM_HEADER_SIZE + (max_readers as usize) * SHM_SLOT_SIZE;
        if bytes.len() != expected_len {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "ShmState::decode",
                format!(
                    ".oryq-shm length {} != expected {expected_len}",
                    bytes.len()
                ),
            ));
        }
        let stored_checksum = u64::from_le_bytes(bytes[56..64].try_into().unwrap());
        let mut hasher_input = Vec::with_capacity(expected_len - 8);
        hasher_input.extend_from_slice(&bytes[0..56]);
        hasher_input.extend_from_slice(&bytes[SHM_HEADER_SIZE..]);
        let computed = xxh3_64(&hasher_input);
        if stored_checksum != computed {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "ShmState::decode",
                "checksum mismatch in .oryq-shm coordination file",
            ));
        }

        let mut slots = Vec::with_capacity(max_readers as usize);
        for i in 0..(max_readers as usize) {
            let off = SHM_HEADER_SIZE + i * SHM_SLOT_SIZE;
            slots.push(ReaderSlot::decode(&bytes[off..off + SHM_SLOT_SIZE])?);
        }

        Ok(Self {
            max_readers,
            latest_committed_tx_id: u64::from_le_bytes(bytes[16..24].try_into().unwrap()),
            writer_pid: u32::from_le_bytes(bytes[24..28].try_into().unwrap()),
            wal_reset_pid: u32::from_le_bytes(bytes[28..32].try_into().unwrap()),
            writer_token: u64::from_le_bytes(bytes[32..40].try_into().unwrap()),
            wal_reset_epoch: u64::from_le_bytes(bytes[40..48].try_into().unwrap()),
            generation: u64::from_le_bytes(bytes[48..56].try_into().unwrap()),
            slots,
        })
    }

    fn next_generation(&mut self) -> u64 {
        let next = self.generation.wrapping_add(1);
        self.generation = if next == 0 { 1 } else { next };
        self.generation
    }

    fn retention_horizon(&self) -> SnapshotId {
        let latest = TxId::new(self.latest_committed_tx_id);
        let min_reader = self
            .slots
            .iter()
            .filter(|s| s.is_active())
            .map(|s| TxId::new(s.snapshot_tx_id))
            .min();
        match min_reader {
            Some(r) => r.min(latest),
            None => latest,
        }
    }

    fn active_reader_count(&self) -> usize {
        self.slots.iter().filter(|s| s.is_active()).count()
    }
}

/// Multi-process MVCC snapshot, writer-lock, and WAL-reset coordinator backed by `.oryq-shm`.
#[derive(Debug, Clone)]
pub struct SharedProcessCoordinator {
    shm_path: PathBuf,
    wlock_path: PathBuf,
    rlock_path: PathBuf,
    config: SharedProcessConfig,
}

impl SharedProcessCoordinator {
    /// Opens or initializes the `.oryq-shm` coordination files for `db_path`.
    ///
    /// If `.oryq-shm` does not yet exist (or was left truncated by a crash when no live peers
    /// hold locks), it is initialized deterministically with `initial_committed_tx_id`.
    pub fn open(
        db_path: &Path,
        initial_committed_tx_id: TxId,
        config: SharedProcessConfig,
    ) -> Result<Self> {
        if config.max_readers == 0 || config.max_readers > MAX_SHM_READERS {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "SharedProcessCoordinator::open",
                format!(
                    "max_readers {} out of range 1..={MAX_SHM_READERS}",
                    config.max_readers
                ),
            ));
        }

        let shm_path = shm_path_for(db_path);
        let wlock_path = wlock_path_for(&shm_path);
        let rlock_path = rlock_path_for(&shm_path);

        if let Some(parent) = shm_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|err| {
                    io_err("SharedProcessCoordinator::open", "create parent dir", err)
                })?;
            }
        }

        // Ensure lock files exist.
        let _ = open_rw_create(&wlock_path, "SharedProcessCoordinator::open")?;
        let _ = open_rw_create(&rlock_path, "SharedProcessCoordinator::open")?;

        let coord = Self {
            shm_path,
            wlock_path,
            rlock_path,
            config,
        };

        // Initialize or validate `.oryq-shm` under short exclusive metadata lock.
        let mut shm_file = open_rw_create(&coord.shm_path, "SharedProcessCoordinator::open")?;
        lock_exclusive_with_timeout(
            &shm_file,
            coord.config.lock_timeout,
            coord.config.lock_retry_interval,
            "SharedProcessCoordinator::open",
        )?;

        let init_res = (|| -> Result<()> {
            let len = shm_file
                .metadata()
                .map_err(|e| io_err("SharedProcessCoordinator::open", "stat shm", e))?
                .len();
            if len == 0 {
                let st = ShmState::new(initial_committed_tx_id, coord.config.max_readers);
                write_shm_state(&mut shm_file, &st)?;
                return Ok(());
            }
            let raw = read_all_file(&mut shm_file, "SharedProcessCoordinator::open")?;
            match ShmState::decode(&raw) {
                Ok(mut st) => {
                    if initial_committed_tx_id.raw() > st.latest_committed_tx_id {
                        st.latest_committed_tx_id = initial_committed_tx_id.raw();
                        st.next_generation();
                        write_shm_state(&mut shm_file, &st)?;
                    }
                    Ok(())
                }
                Err(decode_err) => {
                    // Only reinitialize corrupt/truncated ephemeral `.oryq-shm` when zero peers
                    // hold locks; future schema versions (`UnsupportedFeature`) always fail closed.
                    if decode_err.category() == ErrorCategory::Corruption {
                        let wl =
                            open_rw_create(&coord.wlock_path, "SharedProcessCoordinator::open")?;
                        let rl =
                            open_rw_create(&coord.rlock_path, "SharedProcessCoordinator::open")?;
                        if FileExt::try_lock_exclusive(&wl).is_ok() {
                            let r_ok = FileExt::try_lock_exclusive(&rl).is_ok();
                            if r_ok {
                                let st = ShmState::new(
                                    initial_committed_tx_id,
                                    coord.config.max_readers,
                                );
                                write_shm_state(&mut shm_file, &st)?;
                                let _ = FileExt::unlock(&rl);
                                let _ = FileExt::unlock(&wl);
                                return Ok(());
                            }
                            let _ = FileExt::unlock(&wl);
                        }
                    }
                    Err(decode_err)
                }
            }
        })();

        let _ = FileExt::unlock(&shm_file);
        init_res?;
        Ok(coord)
    }

    pub fn shm_path(&self) -> &Path {
        &self.shm_path
    }

    /// Acquires a multi-process MVCC read snapshot pin.
    ///
    /// Acquires a shared OS lock on `.oryq-shm.rlock` (blocked while a WAL reset is active)
    /// and registers the reader's `SnapshotId` in the `.oryq-shm` reader slot table.
    pub fn acquire_read_snapshot(&self) -> Result<SharedSnapshotGuard> {
        let rlock_file = open_rw_create(
            &self.rlock_path,
            "SharedProcessCoordinator::acquire_read_snapshot",
        )?;
        lock_shared_with_timeout(
            &rlock_file,
            self.config.lock_timeout,
            self.config.lock_retry_interval,
            "SharedProcessCoordinator::acquire_read_snapshot",
        )?;

        let pid = std::process::id();
        let now = now_epoch_ms();
        let lease_ms = lease_timeout_ms(self.config.peer_lease_timeout);

        let reg = self.with_shm_mut("SharedProcessCoordinator::acquire_read_snapshot", |st| {
            // Reclaim any provably dead or lease-expired slots first so crashed readers
            // cannot exhaust the reader slot table.
            reclaim_dead_or_expired_slots(&mut st.slots, now, lease_ms);

            let Some(slot_idx) = st.slots.iter().position(|s| !s.is_active()) else {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Busy,
                    MODULE_ID,
                    "SharedProcessCoordinator::acquire_read_snapshot",
                    format!(
                        "shared reader slot table full (max_readers={})",
                        st.max_readers
                    ),
                ));
            };

            let token_id = st.next_generation();
            let snapshot_id = TxId::new(st.latest_committed_tx_id);
            st.slots[slot_idx] = ReaderSlot {
                state: SLOT_ACTIVE,
                pid,
                token_id,
                snapshot_tx_id: snapshot_id.raw(),
                lease_updated_ms: now,
            };
            Ok((slot_idx, token_id, snapshot_id))
        });

        match reg {
            Ok((slot_idx, token_id, snapshot_id)) => Ok(SharedSnapshotGuard {
                coord: self.clone(),
                _rlock_file: rlock_file,
                slot_idx,
                token_id,
                snapshot_id,
            }),
            Err(err) => {
                let _ = FileExt::unlock(&rlock_file);
                Err(err)
            }
        }
    }

    /// Acquires exclusive multi-process writer ownership, waiting up to `config.lock_timeout`.
    pub fn acquire_writer(&self) -> Result<SharedWriterPermit> {
        let wlock_file =
            open_rw_create(&self.wlock_path, "SharedProcessCoordinator::acquire_writer")?;
        lock_exclusive_with_timeout(
            &wlock_file,
            self.config.lock_timeout,
            self.config.lock_retry_interval,
            "SharedProcessCoordinator::acquire_writer",
        )?;
        self.register_writer_permit(wlock_file, "SharedProcessCoordinator::acquire_writer")
    }

    /// Attempts to acquire exclusive multi-process writer ownership without blocking.
    pub fn try_acquire_writer(&self) -> Result<SharedWriterPermit> {
        let wlock_file = open_rw_create(
            &self.wlock_path,
            "SharedProcessCoordinator::try_acquire_writer",
        )?;
        match FileExt::try_lock_exclusive(&wlock_file) {
            Ok(()) => {}
            Err(err) if is_lock_contended(&err) => {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Busy,
                    MODULE_ID,
                    "SharedProcessCoordinator::try_acquire_writer",
                    "another process holds the exclusive writer lock",
                ));
            }
            Err(err) => {
                return Err(io_err(
                    "SharedProcessCoordinator::try_acquire_writer",
                    "try_lock_exclusive wlock",
                    err,
                ));
            }
        }
        self.register_writer_permit(wlock_file, "SharedProcessCoordinator::try_acquire_writer")
    }

    fn register_writer_permit(
        &self,
        wlock_file: File,
        op: &'static str,
    ) -> Result<SharedWriterPermit> {
        let pid = std::process::id();
        let reg = self.with_shm_mut(op, |st| {
            let token = st.next_generation();
            st.writer_pid = pid;
            st.writer_token = token;
            // Holding exclusive wlock guarantees no peer is mid-WAL-reset either.
            st.wal_reset_pid = 0;
            Ok((token, TxId::new(st.latest_committed_tx_id)))
        });

        match reg {
            Ok((writer_token, base_tx_id)) => Ok(SharedWriterPermit {
                coord: self.clone(),
                _wlock_file: wlock_file,
                writer_token,
                base_tx_id,
            }),
            Err(err) => {
                let _ = FileExt::unlock(&wlock_file);
                Err(err)
            }
        }
    }

    /// Publishes a newly committed transaction ID to `.oryq-shm` while holding `permit`.
    pub fn publish_commit(&self, permit: &SharedWriterPermit, new_tx_id: TxId) -> Result<()> {
        if permit.coord.shm_path != self.shm_path {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "SharedProcessCoordinator::publish_commit",
                "writer permit belongs to a different database .oryq-shm coordinator",
            ));
        }
        self.with_shm_mut("SharedProcessCoordinator::publish_commit", |st| {
            if st.writer_token != permit.writer_token {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "SharedProcessCoordinator::publish_commit",
                    "writer permit token does not match active writer in .oryq-shm",
                ));
            }
            if new_tx_id.raw() < st.latest_committed_tx_id {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "SharedProcessCoordinator::publish_commit",
                    format!(
                        "published tx_id {} regresses below latest_committed {}",
                        new_tx_id.raw(),
                        st.latest_committed_tx_id
                    ),
                ));
            }
            st.latest_committed_tx_id = new_tx_id.raw();
            st.next_generation();
            Ok(())
        })
    }

    /// Attempts to acquire exclusive WAL-reset exclusion while holding `permit`.
    ///
    /// The returned [`WalResetGuard`] borrows `permit`, guaranteeing at compile time that
    /// exclusive writer ownership cannot be released while a WAL reset is in progress.
    ///
    /// Returns `Ok(None)` if any live reader in any process currently holds a snapshot.
    /// Returns `Ok(Some(WalResetGuard))` when zero live readers exist (reclaiming any stale
    /// crashed-reader slots deterministically and blocking new readers until dropped).
    pub fn try_acquire_wal_reset<'a>(
        &self,
        permit: &'a SharedWriterPermit,
    ) -> Result<Option<WalResetGuard<'a>>> {
        if permit.coord.shm_path != self.shm_path {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "SharedProcessCoordinator::try_acquire_wal_reset",
                "writer permit belongs to a different database .oryq-shm coordinator",
            ));
        }
        let rlock_file = open_rw_create(
            &self.rlock_path,
            "SharedProcessCoordinator::try_acquire_wal_reset",
        )?;
        match FileExt::try_lock_exclusive(&rlock_file) {
            Ok(()) => {}
            Err(err) if is_lock_contended(&err) => return Ok(None),
            Err(err) => {
                return Err(io_err(
                    "SharedProcessCoordinator::try_acquire_wal_reset",
                    "try_lock_exclusive rlock",
                    err,
                ));
            }
        }

        let pid = std::process::id();
        let res = self.with_shm_mut("SharedProcessCoordinator::try_acquire_wal_reset", |st| {
            if st.writer_token != permit.writer_token {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "SharedProcessCoordinator::try_acquire_wal_reset",
                    "writer permit token does not match active writer in .oryq-shm",
                ));
            }
            // Exclusive rlock guarantees ZERO live readers exist anywhere in the OS:
            // any remaining active slot belongs to a crashed reader and is reclaimed immediately.
            st.slots.fill(ReaderSlot::EMPTY);
            st.wal_reset_pid = pid;
            st.wal_reset_epoch = st.wal_reset_epoch.wrapping_add(1);
            st.next_generation();
            Ok(st.wal_reset_epoch)
        });

        match res {
            Ok(epoch) => Ok(Some(WalResetGuard {
                coord: self.clone(),
                _permit: permit,
                _rlock_file: rlock_file,
                wal_reset_epoch: epoch,
            })),
            Err(err) => {
                let _ = FileExt::unlock(&rlock_file);
                Err(err)
            }
        }
    }

    /// Probes OS kernel locks and peer liveness to deterministically reclaim crashed
    /// writer, WAL-reset, and reader entries from `.oryq-shm`.
    pub fn reclaim_stale_peers(&self) -> Result<StaleReclaimReport> {
        let wlock = open_rw_create(
            &self.wlock_path,
            "SharedProcessCoordinator::reclaim_stale_peers",
        )?;
        let rlock = open_rw_create(
            &self.rlock_path,
            "SharedProcessCoordinator::reclaim_stale_peers",
        )?;

        // Follow strict lock order: wlock -> rlock -> shm_file.
        let got_wlock = match FileExt::try_lock_exclusive(&wlock) {
            Ok(()) => true,
            Err(err) if is_lock_contended(&err) => false,
            Err(err) => {
                return Err(io_err(
                    "SharedProcessCoordinator::reclaim_stale_peers",
                    "try_lock_exclusive wlock",
                    err,
                ));
            }
        };
        let got_rlock = match FileExt::try_lock_exclusive(&rlock) {
            Ok(()) => true,
            Err(err) if is_lock_contended(&err) => false,
            Err(err) => {
                if got_wlock {
                    let _ = FileExt::unlock(&wlock);
                }
                return Err(io_err(
                    "SharedProcessCoordinator::reclaim_stale_peers",
                    "try_lock_exclusive rlock",
                    err,
                ));
            }
        };

        let now = now_epoch_ms();
        let lease_ms = lease_timeout_ms(self.config.peer_lease_timeout);

        let res = self.with_shm_mut("SharedProcessCoordinator::reclaim_stale_peers", |st| {
            let mut report = StaleReclaimReport::default();
            if got_wlock {
                if st.writer_pid != 0 {
                    st.writer_pid = 0;
                    st.writer_token = 0;
                    report.reclaimed_writer = true;
                }
                // Because `WalResetGuard` requires holding `SharedWriterPermit` (`wlock`),
                // acquiring `wlock` proves any recorded `wal_reset_pid` is dead even if new
                // readers have since acquired shared `rlock`.
                if st.wal_reset_pid != 0 {
                    st.wal_reset_pid = 0;
                    report.reclaimed_wal_reset = true;
                }
            }
            if got_rlock {
                // Zero live readers exist anywhere in the OS -> reclaim every stale slot.
                for slot in &mut st.slots {
                    if slot.is_active() {
                        *slot = ReaderSlot::EMPTY;
                        report.reclaimed_readers += 1;
                    }
                }
            } else {
                report.reclaimed_readers =
                    reclaim_dead_or_expired_slots(&mut st.slots, now, lease_ms);
            }
            if report.reclaimed_writer || report.reclaimed_wal_reset || report.reclaimed_readers > 0
            {
                st.next_generation();
            }
            Ok(report)
        });

        if got_rlock {
            let _ = FileExt::unlock(&rlock);
        }
        if got_wlock {
            let _ = FileExt::unlock(&wlock);
        }
        res
    }

    /// Returns the minimum of `latest_committed_tx_id` and all active reader snapshot pins.
    pub fn retention_horizon(&self) -> Result<SnapshotId> {
        let st = self.read_shm("SharedProcessCoordinator::retention_horizon")?;
        Ok(st.retention_horizon())
    }

    pub fn latest_committed(&self) -> Result<TxId> {
        let st = self.read_shm("SharedProcessCoordinator::latest_committed")?;
        Ok(TxId::new(st.latest_committed_tx_id))
    }

    pub fn active_reader_count(&self) -> Result<usize> {
        let st = self.read_shm("SharedProcessCoordinator::active_reader_count")?;
        Ok(st.active_reader_count())
    }

    /// Returns a point-in-time inspection snapshot of the `.oryq-shm` coordination state.
    pub fn inspect(&self) -> Result<ShmInspectionReport> {
        let st = self.read_shm("SharedProcessCoordinator::inspect")?;
        Ok(ShmInspectionReport {
            shm_path: self.shm_path.clone(),
            max_readers: st.max_readers as usize,
            latest_committed_tx_id: TxId::new(st.latest_committed_tx_id),
            retention_horizon: st.retention_horizon(),
            active_readers: st.active_reader_count(),
            writer_pid: (st.writer_pid != 0).then_some(st.writer_pid),
            wal_reset_pid: (st.wal_reset_pid != 0).then_some(st.wal_reset_pid),
            wal_reset_epoch: st.wal_reset_epoch,
            generation: st.generation,
        })
    }

    fn read_shm(&self, op: &'static str) -> Result<ShmState> {
        let mut shm_file = open_rw_create(&self.shm_path, op)?;
        lock_shared_with_timeout(
            &shm_file,
            self.config.lock_timeout,
            self.config.lock_retry_interval,
            op,
        )?;
        let res = read_all_file(&mut shm_file, op).and_then(|raw| ShmState::decode(&raw));
        let _ = FileExt::unlock(&shm_file);
        res
    }

    fn with_shm_mut<T>(
        &self,
        op: &'static str,
        f: impl FnOnce(&mut ShmState) -> Result<T>,
    ) -> Result<T> {
        let mut shm_file = open_rw_create(&self.shm_path, op)?;
        lock_exclusive_with_timeout(
            &shm_file,
            self.config.lock_timeout,
            self.config.lock_retry_interval,
            op,
        )?;
        let res = (|| -> Result<T> {
            let raw = read_all_file(&mut shm_file, op)?;
            let mut st = ShmState::decode(&raw)?;
            let out = f(&mut st)?;
            write_shm_state(&mut shm_file, &st)?;
            Ok(out)
        })();
        let _ = FileExt::unlock(&shm_file);
        res
    }
}

/// RAII multi-process reader snapshot pin. Dropping unregisters the slot and releases `.oryq-shm.rlock`.
#[derive(Debug)]
pub struct SharedSnapshotGuard {
    coord: SharedProcessCoordinator,
    _rlock_file: File,
    slot_idx: usize,
    token_id: u64,
    snapshot_id: SnapshotId,
}

impl SharedSnapshotGuard {
    #[inline]
    pub fn snapshot_id(&self) -> SnapshotId {
        self.snapshot_id
    }

    #[inline]
    pub fn slot_index(&self) -> usize {
        self.slot_idx
    }

    /// Refreshes this reader slot's heartbeat lease timestamp for long-running read snapshots.
    ///
    /// Returns [`ErrorCategory::SnapshotTooOld`] if the slot was already evicted due to lease
    /// expiration or peer reclamation.
    pub fn refresh_lease(&self) -> Result<()> {
        let now = now_epoch_ms();
        self.coord
            .with_shm_mut("SharedSnapshotGuard::refresh_lease", |st| {
                match st.slots.get_mut(self.slot_idx) {
                    Some(slot) if slot.is_active() && slot.token_id == self.token_id => {
                        slot.lease_updated_ms = now;
                        st.next_generation();
                        Ok(())
                    }
                    _ => Err(ErrorEnvelope::new(
                        ErrorCategory::SnapshotTooOld,
                        MODULE_ID,
                        "SharedSnapshotGuard::refresh_lease",
                        "reader snapshot slot was evicted from .oryq-shm",
                    )),
                }
            })
    }
}

impl Drop for SharedSnapshotGuard {
    fn drop(&mut self) {
        let _ = self.coord.with_shm_mut("SharedSnapshotGuard::drop", |st| {
            if let Some(slot) = st.slots.get_mut(self.slot_idx) {
                if slot.is_active() && slot.token_id == self.token_id {
                    *slot = ReaderSlot::EMPTY;
                    st.next_generation();
                }
            }
            Ok(())
        });
        let _ = FileExt::unlock(&self._rlock_file);
    }
}

/// RAII exclusive multi-process writer permit holding `.oryq-shm.wlock`.
#[derive(Debug)]
pub struct SharedWriterPermit {
    coord: SharedProcessCoordinator,
    _wlock_file: File,
    writer_token: u64,
    base_tx_id: TxId,
}

impl SharedWriterPermit {
    #[inline]
    pub fn base_tx_id(&self) -> TxId {
        self.base_tx_id
    }

    #[inline]
    pub fn writer_token(&self) -> u64 {
        self.writer_token
    }
}

impl Drop for SharedWriterPermit {
    fn drop(&mut self) {
        let _ = self.coord.with_shm_mut("SharedWriterPermit::drop", |st| {
            if st.writer_token == self.writer_token {
                st.writer_pid = 0;
                st.writer_token = 0;
                st.next_generation();
            }
            Ok(())
        });
        let _ = FileExt::unlock(&self._wlock_file);
    }
}

/// RAII WAL-reset exclusion guard holding exclusive `.oryq-shm.rlock` and borrowing
/// [`SharedWriterPermit`] so no reader or concurrent writer in any process can enter
/// while the WAL is being truncated/reset.
#[derive(Debug)]
pub struct WalResetGuard<'a> {
    coord: SharedProcessCoordinator,
    _permit: &'a SharedWriterPermit,
    _rlock_file: File,
    wal_reset_epoch: u64,
}

impl WalResetGuard<'_> {
    #[inline]
    pub fn wal_reset_epoch(&self) -> u64 {
        self.wal_reset_epoch
    }
}

impl Drop for WalResetGuard<'_> {
    fn drop(&mut self) {
        let _ = self.coord.with_shm_mut("WalResetGuard::drop", |st| {
            if st.wal_reset_epoch == self.wal_reset_epoch {
                st.wal_reset_pid = 0;
                st.next_generation();
            }
            Ok(())
        });
        let _ = FileExt::unlock(&self._rlock_file);
    }
}

fn reclaim_dead_or_expired_slots(slots: &mut [ReaderSlot], now_ms: u64, lease_ms: u64) -> usize {
    let mut reclaimed = 0usize;
    for slot in slots.iter_mut() {
        if !slot.is_active() {
            continue;
        }
        let pid_dead = is_pid_definitely_dead(slot.pid);
        let expired = now_ms.saturating_sub(slot.lease_updated_ms) > lease_ms;
        if pid_dead || expired {
            *slot = ReaderSlot::EMPTY;
            reclaimed += 1;
        }
    }
    reclaimed
}

fn is_pid_definitely_dead(pid: u32) -> bool {
    if pid == 0 {
        return true;
    }
    #[cfg(target_os = "linux")]
    {
        if Path::new("/proc/self").exists() {
            !Path::new(&format!("/proc/{pid}")).exists()
        } else {
            false
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        false
    }
}

fn lease_timeout_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

fn is_lock_contended(err: &std::io::Error) -> bool {
    let contended = fs4::lock_contended_error();
    err.kind() == std::io::ErrorKind::WouldBlock
        || err.kind() == contended.kind()
        || (contended.raw_os_error().is_some() && err.raw_os_error() == contended.raw_os_error())
        || err.raw_os_error() == Some(33) // Windows ERROR_LOCK_VIOLATION
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn open_rw_create(path: &Path, op: &'static str) -> Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|err| io_err(op, &format!("open '{}'", path.display()), err))
}

fn read_all_file(file: &mut File, op: &'static str) -> Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))
        .map_err(|err| io_err(op, "seek shm start", err))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|err| io_err(op, "read shm", err))?;
    Ok(buf)
}

fn write_shm_state(file: &mut File, st: &ShmState) -> Result<()> {
    let encoded = st.encode();
    file.seek(SeekFrom::Start(0))
        .map_err(|err| io_err("write_shm_state", "seek start", err))?;
    file.write_all(&encoded)
        .map_err(|err| io_err("write_shm_state", "write shm", err))?;
    file.set_len(encoded.len() as u64)
        .map_err(|err| io_err("write_shm_state", "set_len shm", err))?;
    file.flush()
        .map_err(|err| io_err("write_shm_state", "flush shm", err))?;
    Ok(())
}

fn lock_exclusive_with_timeout(
    file: &File,
    timeout: Duration,
    retry_interval: Duration,
    op: &'static str,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        match FileExt::try_lock_exclusive(file) {
            Ok(()) => return Ok(()),
            Err(err) if is_lock_contended(&err) => {
                if Instant::now() >= deadline {
                    return Err(ErrorEnvelope::new(
                        ErrorCategory::Busy,
                        MODULE_ID,
                        op,
                        "timed out waiting for exclusive .oryq-shm coordination lock",
                    ));
                }
                std::thread::sleep(retry_interval);
            }
            Err(err) => return Err(io_err(op, "try_lock_exclusive", err)),
        }
    }
}

fn lock_shared_with_timeout(
    file: &File,
    timeout: Duration,
    retry_interval: Duration,
    op: &'static str,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        match FileExt::try_lock_shared(file) {
            Ok(()) => return Ok(()),
            Err(err) if is_lock_contended(&err) => {
                if Instant::now() >= deadline {
                    return Err(ErrorEnvelope::new(
                        ErrorCategory::Busy,
                        MODULE_ID,
                        op,
                        "timed out waiting for shared .oryq-shm reader lock (WAL reset in progress)",
                    ));
                }
                std::thread::sleep(retry_interval);
            }
            Err(err) => return Err(io_err(op, "try_lock_shared", err)),
        }
    }
}

fn io_err(op: &'static str, context: &str, err: std::io::Error) -> ErrorEnvelope {
    ErrorEnvelope::new(
        ErrorCategory::Io,
        MODULE_ID,
        op,
        format!("{context}: {err}"),
    )
    .with_source(err)
}
