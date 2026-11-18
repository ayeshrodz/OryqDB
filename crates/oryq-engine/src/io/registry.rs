//! Process-local file identity registry to prevent duplicate concurrent opens
//! of the same physical file via symlinks, hardlinks, or relative path aliases.

use std::collections::HashSet;
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

/// Process-local registry tracking currently opened physical files.
#[derive(Debug, Default)]
pub struct FileRegistry {
    active: Mutex<HashSet<FileIdentity>>,
}

impl FileRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquires an exclusive process-local lease for `identity`.
    ///
    /// Returns [`ErrorCategory::LockedByProcess`] if the same physical file is
    /// already open in this process.
    pub fn acquire(self: &Arc<Self>, identity: FileIdentity) -> Result<FileLease> {
        let mut guard = self.active.lock().map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Internal,
                MODULE_ID,
                "FileRegistry::acquire",
                "file registry lock poisoned",
            )
        })?;
        if !guard.insert(identity.clone()) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::LockedByProcess,
                MODULE_ID,
                "FileRegistry::acquire",
                format!("database file is already open in this process ({identity:?})"),
            ));
        }
        Ok(FileLease {
            registry: Arc::clone(self),
            identity,
        })
    }

    pub fn is_active(&self, identity: &FileIdentity) -> bool {
        self.active
            .lock()
            .map(|g| g.contains(identity))
            .unwrap_or(false)
    }
}

/// RAII lease releasing a [`FileIdentity`] from its [`FileRegistry`] on drop.
#[derive(Debug)]
pub struct FileLease {
    registry: Arc<FileRegistry>,
    identity: FileIdentity,
}

impl FileLease {
    pub fn identity(&self) -> &FileIdentity {
        &self.identity
    }
}

impl Drop for FileLease {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.registry.active.lock() {
            guard.remove(&self.identity);
        }
    }
}
