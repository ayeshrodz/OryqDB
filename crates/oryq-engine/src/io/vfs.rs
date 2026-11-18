//! Virtual Filesystem (`StorageIo` & `FileHandle`) contract and default OS implementation.

use std::fmt::Debug;
use std::fs::{File, OpenOptions};
use std::path::Path;
use std::sync::{Arc, OnceLock};

use oryq_types::{ErrorCategory, ErrorEnvelope, Result};

use crate::io::platform;
use crate::io::registry::{FileIdentity, FileRegistry};

const MODULE_ID: &str = "engine";

/// File open options for [`StorageIo::open`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenMode {
    pub read: bool,
    pub write: bool,
    pub create: bool,
    pub create_new: bool,
    pub truncate: bool,
}

impl OpenMode {
    pub const READ_ONLY: Self = Self {
        read: true,
        write: false,
        create: false,
        create_new: false,
        truncate: false,
    };

    pub const READ_WRITE: Self = Self {
        read: true,
        write: true,
        create: false,
        create_new: false,
        truncate: false,
    };

    pub const CREATE_NEW: Self = Self {
        read: true,
        write: true,
        create: true,
        create_new: true,
        truncate: false,
    };

    pub const CREATE_OR_OPEN: Self = Self {
        read: true,
        write: true,
        create: true,
        create_new: false,
        truncate: false,
    };
}

/// Positioned, thread-safe file handle contract.
pub trait FileHandle: Debug + Send + Sync {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> Result<()>;
    fn write_all_at(&self, buf: &[u8], offset: u64) -> Result<()>;
    fn sync_data(&self) -> Result<()>;
    fn sync_all(&self) -> Result<()>;
    fn len(&self) -> Result<u64>;
    fn is_empty(&self) -> Result<bool> {
        Ok(self.len()? == 0)
    }
    fn set_len(&self, size: u64) -> Result<()>;
    fn try_lock_exclusive(&self) -> Result<()>;
    fn unlock(&self) -> Result<()>;
    fn identity(&self) -> &FileIdentity;
}

/// Virtual filesystem boundary for positioned I/O, locking, and directory durability.
pub trait StorageIo: Debug + Send + Sync {
    fn open(&self, path: &Path, mode: OpenMode) -> Result<Arc<dyn FileHandle>>;
    fn sync_parent_dir(&self, path: &Path) -> Result<()>;
    fn remove_file(&self, path: &Path) -> Result<()>;
    fn exists(&self, path: &Path) -> bool;
    fn file_registry(&self) -> Arc<FileRegistry>;
}

/// Process-global default [`FileRegistry`] shared by [`OsVfs::default()`].
pub fn global_file_registry() -> Arc<FileRegistry> {
    static REGISTRY: OnceLock<Arc<FileRegistry>> = OnceLock::new();
    Arc::clone(REGISTRY.get_or_init(|| Arc::new(FileRegistry::new())))
}

/// Default OS-backed [`StorageIo`] implementation.
#[derive(Debug, Clone)]
pub struct OsVfs {
    registry: Arc<FileRegistry>,
}

impl Default for OsVfs {
    fn default() -> Self {
        Self {
            registry: global_file_registry(),
        }
    }
}

impl OsVfs {
    /// Creates an `OsVfs` sharing the process-global [`FileRegistry`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates an `OsVfs` with an explicit [`FileRegistry`] (useful for isolated tests).
    pub fn with_registry(registry: Arc<FileRegistry>) -> Self {
        Self { registry }
    }
}

#[derive(Debug)]
struct OsFileHandle {
    file: File,
    identity: FileIdentity,
}

impl FileHandle for OsFileHandle {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> Result<()> {
        platform::read_exact_at(&self.file, buf, offset)
    }

    fn write_all_at(&self, buf: &[u8], offset: u64) -> Result<()> {
        platform::write_all_at(&self.file, buf, offset)
    }

    fn sync_data(&self) -> Result<()> {
        self.file.sync_data().map_err(|err| {
            ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "OsFileHandle::sync_data",
                format!("sync_data failed: {err}"),
            )
            .with_source(err)
        })
    }

    fn sync_all(&self) -> Result<()> {
        self.file.sync_all().map_err(|err| {
            ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "OsFileHandle::sync_all",
                format!("sync_all failed: {err}"),
            )
            .with_source(err)
        })
    }

    fn len(&self) -> Result<u64> {
        self.file.metadata().map(|m| m.len()).map_err(|err| {
            ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "OsFileHandle::len",
                format!("metadata len failed: {err}"),
            )
            .with_source(err)
        })
    }

    fn set_len(&self, size: u64) -> Result<()> {
        self.file.set_len(size).map_err(|err| {
            ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "OsFileHandle::set_len",
                format!("set_len({size}) failed: {err}"),
            )
            .with_source(err)
        })
    }

    fn try_lock_exclusive(&self) -> Result<()> {
        platform::try_lock_exclusive(&self.file)
    }

    fn unlock(&self) -> Result<()> {
        platform::unlock(&self.file)
    }

    fn identity(&self) -> &FileIdentity {
        &self.identity
    }
}

impl StorageIo for OsVfs {
    fn open(&self, path: &Path, mode: OpenMode) -> Result<Arc<dyn FileHandle>> {
        let mut opts = OpenOptions::new();
        opts.read(mode.read)
            .write(mode.write)
            .create(mode.create)
            .create_new(mode.create_new)
            .truncate(mode.truncate);

        let file = opts.open(path).map_err(|err| {
            let category = match err.kind() {
                std::io::ErrorKind::NotFound => ErrorCategory::NotFound,
                _ => ErrorCategory::Io,
            };
            ErrorEnvelope::new(
                category,
                MODULE_ID,
                "OsVfs::open",
                format!("failed to open {}: {err}", path.display()),
            )
            .with_source(err)
        })?;
        let identity = platform::file_identity(&file, path)?;
        Ok(Arc::new(OsFileHandle { file, identity }))
    }

    fn sync_parent_dir(&self, path: &Path) -> Result<()> {
        platform::sync_parent_dir(path)
    }

    fn remove_file(&self, path: &Path) -> Result<()> {
        std::fs::remove_file(path).map_err(|err| {
            ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "OsVfs::remove_file",
                format!("failed to remove {}: {err}", path.display()),
            )
            .with_source(err)
        })
    }

    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn file_registry(&self) -> Arc<FileRegistry> {
        Arc::clone(&self.registry)
    }
}
