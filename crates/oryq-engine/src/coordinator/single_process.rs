//! Single-process MVCC snapshot coordinator (`docs/transactions.md` §4.1).
//!
//! Atomically registers reader snapshot pins under a short admission mutex so
//! checkpointing and page reclamation never race with snapshot acquisition.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};

use oryq_types::{ErrorCategory, ErrorEnvelope, Result, SnapshotId, TxId};

const MODULE_ID: &str = "engine";

#[derive(Debug)]
struct CoordState {
    latest_committed: TxId,
    next_token_id: u64,
    active_snapshots: HashMap<u64, SnapshotId>,
    max_readers: usize,
    writer_active: bool,
    admission_blocked: bool,
    poisoned: bool,
}

/// Default single-process snapshot admission and serialized writer coordinator.
#[derive(Debug)]
pub struct SingleProcessCoordinator {
    state: Arc<Mutex<CoordState>>,
    writer_cv: Arc<Condvar>,
    admission_cv: Arc<Condvar>,
}

impl SingleProcessCoordinator {
    pub fn new(initial_committed_tx_id: TxId, max_readers: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(CoordState {
                latest_committed: initial_committed_tx_id,
                next_token_id: 1,
                active_snapshots: HashMap::new(),
                max_readers: max_readers.max(1),
                writer_active: false,
                admission_blocked: false,
                poisoned: false,
            })),
            writer_cv: Arc::new(Condvar::new()),
            admission_cv: Arc::new(Condvar::new()),
        }
    }

    /// Atomically reads `latest_committed` and registers a retention pin under the
    /// coordinator mutex. Multiple nested readers on the same thread receive distinct tokens.
    pub fn acquire_read_snapshot(&self) -> Result<SnapshotGuard> {
        let mut st = self.state.lock().map_err(|_| poisoned_lock())?;
        while st.admission_blocked && !st.poisoned {
            st = self.admission_cv.wait(st).map_err(|_| poisoned_lock())?;
        }
        if st.poisoned {
            return Err(engine_poisoned(
                "SingleProcessCoordinator::acquire_read_snapshot",
            ));
        }
        if st.active_snapshots.len() >= st.max_readers {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Busy,
                MODULE_ID,
                "SingleProcessCoordinator::acquire_read_snapshot",
                format!(
                    "reader snapshot table full (max_readers={})",
                    st.max_readers
                ),
            ));
        }
        let token_id = st.next_token_id;
        st.next_token_id = st.next_token_id.wrapping_add(1);
        let snapshot_id = st.latest_committed;
        st.active_snapshots.insert(token_id, snapshot_id);
        Ok(SnapshotGuard {
            state: Arc::clone(&self.state),
            token_id,
            snapshot_id,
        })
    }

    /// Acquires exclusive logical writer ownership, blocking until any in-flight writer completes.
    pub fn acquire_writer(&self) -> Result<WriterPermit> {
        let mut st = self.state.lock().map_err(|_| poisoned_lock())?;
        while st.writer_active && !st.poisoned {
            st = self.writer_cv.wait(st).map_err(|_| poisoned_lock())?;
        }
        if st.poisoned {
            return Err(engine_poisoned("SingleProcessCoordinator::acquire_writer"));
        }
        st.writer_active = true;
        let base_tx_id = st.latest_committed;
        Ok(WriterPermit {
            state: Arc::clone(&self.state),
            writer_cv: Arc::clone(&self.writer_cv),
            base_tx_id,
        })
    }

    /// Attempts to acquire exclusive logical writer ownership without blocking.
    pub fn try_acquire_writer(&self) -> Result<WriterPermit> {
        let mut st = self.state.lock().map_err(|_| poisoned_lock())?;
        if st.poisoned {
            return Err(engine_poisoned(
                "SingleProcessCoordinator::try_acquire_writer",
            ));
        }
        if st.writer_active {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Busy,
                MODULE_ID,
                "SingleProcessCoordinator::try_acquire_writer",
                "another write transaction is currently active",
            ));
        }
        st.writer_active = true;
        let base_tx_id = st.latest_committed;
        Ok(WriterPermit {
            state: Arc::clone(&self.state),
            writer_cv: Arc::clone(&self.writer_cv),
            base_tx_id,
        })
    }

    /// Publishes a newly committed transaction ID after WAL sync and version installation.
    pub fn publish_commit(&self, new_tx_id: TxId) -> Result<()> {
        let mut st = self.state.lock().map_err(|_| poisoned_lock())?;
        if st.poisoned {
            return Err(engine_poisoned("SingleProcessCoordinator::publish_commit"));
        }
        st.latest_committed = new_tx_id;
        Ok(())
    }

    /// Returns the minimum of `latest_committed` and all active reader snapshot pins.
    pub fn retention_horizon(&self) -> Result<SnapshotId> {
        let st = self.state.lock().map_err(|_| poisoned_lock())?;
        let min_reader = st.active_snapshots.values().copied().min();
        Ok(match min_reader {
            Some(r) => r.min(st.latest_committed),
            None => st.latest_committed,
        })
    }

    pub fn latest_committed(&self) -> Result<TxId> {
        let st = self.state.lock().map_err(|_| poisoned_lock())?;
        Ok(st.latest_committed)
    }

    pub fn active_reader_count(&self) -> Result<usize> {
        let st = self.state.lock().map_err(|_| poisoned_lock())?;
        Ok(st.active_snapshots.len())
    }

    /// Blocks new reader admission if and only if zero active snapshots remain.
    /// Returns `Ok(true)` if admission is now blocked (caller must call `unblock_admission`),
    /// or `Ok(false)` if active snapshots are still present.
    pub fn try_block_admission_when_idle(&self) -> Result<bool> {
        let mut st = self.state.lock().map_err(|_| poisoned_lock())?;
        if st.poisoned {
            return Err(engine_poisoned(
                "SingleProcessCoordinator::try_block_admission_when_idle",
            ));
        }
        if !st.active_snapshots.is_empty() {
            return Ok(false);
        }
        st.admission_blocked = true;
        Ok(true)
    }

    pub fn unblock_admission(&self) {
        if let Ok(mut st) = self.state.lock() {
            st.admission_blocked = false;
            self.admission_cv.notify_all();
        }
    }

    /// Permanently poisons the coordinator after an uncertain commit write/sync failure.
    pub fn poison(&self) {
        if let Ok(mut st) = self.state.lock() {
            st.poisoned = true;
            st.admission_blocked = false;
            self.writer_cv.notify_all();
            self.admission_cv.notify_all();
        }
    }

    pub fn is_poisoned(&self) -> bool {
        self.state.lock().map(|g| g.poisoned).unwrap_or(true)
    }
}

/// RAII reader snapshot pin. Dropping unregisters the pin from [`SingleProcessCoordinator`].
#[derive(Debug)]
pub struct SnapshotGuard {
    state: Arc<Mutex<CoordState>>,
    token_id: u64,
    snapshot_id: SnapshotId,
}

impl SnapshotGuard {
    #[inline]
    pub fn snapshot_id(&self) -> SnapshotId {
        self.snapshot_id
    }
}

impl Drop for SnapshotGuard {
    fn drop(&mut self) {
        if let Ok(mut st) = self.state.lock() {
            st.active_snapshots.remove(&self.token_id);
        }
    }
}

/// RAII exclusive writer ownership token (movable `Send`, holds no `MutexGuard`).
#[derive(Debug)]
pub struct WriterPermit {
    state: Arc<Mutex<CoordState>>,
    writer_cv: Arc<Condvar>,
    base_tx_id: TxId,
}

impl WriterPermit {
    #[inline]
    pub fn base_tx_id(&self) -> TxId {
        self.base_tx_id
    }
}

impl Drop for WriterPermit {
    fn drop(&mut self) {
        if let Ok(mut st) = self.state.lock() {
            st.writer_active = false;
            self.writer_cv.notify_one();
        }
    }
}

fn poisoned_lock() -> ErrorEnvelope {
    ErrorEnvelope::new(
        ErrorCategory::Internal,
        MODULE_ID,
        "SingleProcessCoordinator",
        "coordinator mutex poisoned",
    )
}

fn engine_poisoned(op: &'static str) -> ErrorEnvelope {
    ErrorEnvelope::new(
        ErrorCategory::CommitOutcomeUnknown,
        MODULE_ID,
        op,
        "engine is poisoned after an uncertain commit write/sync failure; close and reopen to recover",
    )
}
