//! Process-local file identity registry to prevent duplicate concurrent opens
//! of the same physical file via symlinks, hardlinks, or relative path aliases.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use oryq_types::{ErrorCategory, ErrorEnvelope, Result};

const MODULE_ID: &str = "engine";

/// Platform-normalized identity for a physical file on disk or in a virtual filesystem.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FileIdentity {
    /// Unix `(st_dev, st_ino)` tuple.
    Unix { dev: u64, ino: u64 },
    /// Canonicalized path identity (Windows fallback or in-memory VFS).
    Path(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LeaseMode {
    Exclusive,
    Shared(usize),
}

/// Process-local registry tracking currently opened physical files.
#[derive(Debug, Default)]
pub struct FileRegistry {
    active: Mutex<HashMap<FileIdentity, LeaseMode>>,
}

impl FileRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquires an exclusive process-local lease for `identity`.
    ///
    /// Returns [`ErrorCategory::LockedByProcess`] if the same physical file is
    /// already open in this process (exclusively or shared).
    pub fn acquire(self: &Arc<Self>, identity: FileIdentity) -> Result<FileLease> {
        let mut guard = self.active.lock().map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Internal,
                MODULE_ID,
                "FileRegistry::acquire",
                "file registry lock poisoned",
            )
        })?;
        if guard.contains_key(&identity) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::LockedByProcess,
                MODULE_ID,
                "FileRegistry::acquire",
                format!("database file is already open in this process ({identity:?})"),
            ));
        }
        guard.insert(identity.clone(), LeaseMode::Exclusive);
        Ok(FileLease {
            registry: Arc::clone(self),
            identity,
            shared: false,
        })
    }

    /// Acquires a shared read-only process-local lease for `identity`.
    ///
    /// Multiple concurrent `read_only` handles may share a lease; fails with
    /// [`ErrorCategory::LockedByProcess`] if an exclusive writer lease is active.
    pub fn acquire_shared(self: &Arc<Self>, identity: FileIdentity) -> Result<FileLease> {
        let mut guard = self.active.lock().map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Internal,
                MODULE_ID,
                "FileRegistry::acquire_shared",
                "file registry lock poisoned",
            )
        })?;
        match guard.get_mut(&identity) {
            Some(LeaseMode::Exclusive) => {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::LockedByProcess,
                    MODULE_ID,
                    "FileRegistry::acquire_shared",
                    format!("database file is exclusively open in this process ({identity:?})"),
                ));
            }
            Some(LeaseMode::Shared(count)) => {
                *count += 1;
            }
            None => {
                guard.insert(identity.clone(), LeaseMode::Shared(1));
            }
        }
        Ok(FileLease {
            registry: Arc::clone(self),
            identity,
            shared: true,
        })
    }

    pub fn is_active(&self, identity: &FileIdentity) -> bool {
        self.active
            .lock()
            .map(|g| g.contains_key(identity))
            .unwrap_or(false)
    }
}

/// RAII lease releasing a [`FileIdentity`] from its [`FileRegistry`] on drop.
#[derive(Debug)]
pub struct FileLease {
    registry: Arc<FileRegistry>,
    identity: FileIdentity,
    shared: bool,
}

impl FileLease {
    pub fn identity(&self) -> &FileIdentity {
        &self.identity
    }
}

impl Drop for FileLease {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.registry.active.lock() {
            if self.shared {
                if let Some(LeaseMode::Shared(count)) = guard.get_mut(&self.identity) {
                    if *count <= 1 {
                        guard.remove(&self.identity);
                    } else {
                        *count -= 1;
                    }
                }
            } else {
                guard.remove(&self.identity);
            }
        }
    }
}
