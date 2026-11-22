//! Pure-safe, zero-disk in-memory [`StorageIo`] and [`FileHandle`] implementation (`MemoryVfs`).
//!
//! Each [`MemoryVfs`] instance owns an isolated virtual file table and its own [`FileRegistry`],
//! allowing multiple independent `:memory:` databases to coexist in the same process without
//! colliding or touching the host filesystem.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use oryq_types::{ErrorCategory, ErrorEnvelope, Result};

use crate::io::registry::{FileIdentity, FileRegistry};
use crate::io::vfs::{FileHandle, OpenMode, StorageIo};

const MODULE_ID: &str = "engine";

#[derive(Debug, Default)]
struct MemoryFile {
    data: Mutex<Vec<u8>>,
    locked: Mutex<bool>,
}

/// Pure-safe in-memory [`StorageIo`] virtual filesystem (`#![forbid(unsafe_code)]`).
#[derive(Debug, Clone)]
pub struct MemoryVfs {
    files: Arc<Mutex<HashMap<PathBuf, Arc<MemoryFile>>>>,
    registry: Arc<FileRegistry>,
}

impl Default for MemoryVfs {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryVfs {
    /// Creates an empty, isolated in-memory VFS with its own [`FileRegistry`].
    pub fn new() -> Self {
        Self {
            files: Arc::new(Mutex::new(HashMap::new())),
            registry: Arc::new(FileRegistry::new()),
        }
    }

    /// Creates an empty in-memory VFS with an explicit [`FileRegistry`].
    pub fn with_registry(registry: Arc<FileRegistry>) -> Self {
        Self {
            files: Arc::new(Mutex::new(HashMap::new())),
            registry,
        }
    }

    /// Returns the number of virtual files currently stored in this VFS.
    pub fn file_count(&self) -> usize {
        self.files.lock().map(|g| g.len()).unwrap_or(0)
    }
}

#[derive(Debug)]
struct MemoryFileHandle {
    file: Arc<MemoryFile>,
    identity: FileIdentity,
    readable: bool,
    writable: bool,
    holds_lock: AtomicBool,
}

impl Drop for MemoryFileHandle {
    fn drop(&mut self) {
        if self.holds_lock.swap(false, Ordering::AcqRel) {
            if let Ok(mut locked) = self.file.locked.lock() {
                *locked = false;
            }
        }
    }
}

impl FileHandle for MemoryFileHandle {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> Result<()> {
        if !self.readable {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "MemoryFileHandle::read_exact_at",
                "file handle is not opened for reading",
            ));
        }
        let start = usize::try_from(offset).map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "MemoryFileHandle::read_exact_at",
                format!("read offset {offset} exceeds platform usize"),
            )
        })?;
        let end = start.checked_add(buf.len()).ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "MemoryFileHandle::read_exact_at",
                "read offset + length overflow",
            )
        })?;
        let data = self.file.data.lock().map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Internal,
                MODULE_ID,
                "MemoryFileHandle::read_exact_at",
                "memory file lock poisoned",
            )
        })?;
        if end > data.len() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "MemoryFileHandle::read_exact_at",
                format!(
                    "unexpected EOF reading {} bytes at offset {offset} (len={})",
                    buf.len(),
                    data.len()
                ),
            ));
        }
        buf.copy_from_slice(&data[start..end]);
        Ok(())
    }

    fn write_all_at(&self, buf: &[u8], offset: u64) -> Result<()> {
        if !self.writable {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "MemoryFileHandle::write_all_at",
                "file handle is not opened for writing",
            ));
        }
        let start = usize::try_from(offset).map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "MemoryFileHandle::write_all_at",
                format!("write offset {offset} exceeds platform usize"),
            )
        })?;
        let end = start.checked_add(buf.len()).ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "MemoryFileHandle::write_all_at",
                "write offset + length overflow",
            )
        })?;
        let mut data = self.file.data.lock().map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Internal,
                MODULE_ID,
                "MemoryFileHandle::write_all_at",
                "memory file lock poisoned",
            )
        })?;
        if end > data.len() {
            data.resize(end, 0);
        }
        data[start..end].copy_from_slice(buf);
        Ok(())
    }

    fn sync_data(&self) -> Result<()> {
        Ok(())
    }

    fn sync_all(&self) -> Result<()> {
        Ok(())
    }

    fn len(&self) -> Result<u64> {
        let data = self.file.data.lock().map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Internal,
                MODULE_ID,
                "MemoryFileHandle::len",
                "memory file lock poisoned",
            )
        })?;
        Ok(data.len() as u64)
    }

    fn set_len(&self, size: u64) -> Result<()> {
        if !self.writable {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "MemoryFileHandle::set_len",
                "file handle is not opened for writing",
            ));
        }
        let new_len = usize::try_from(size).map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Overflow,
                MODULE_ID,
                "MemoryFileHandle::set_len",
                format!("file size {size} exceeds platform usize"),
            )
        })?;
        let mut data = self.file.data.lock().map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Internal,
                MODULE_ID,
                "MemoryFileHandle::set_len",
                "memory file lock poisoned",
            )
        })?;
        data.resize(new_len, 0);
        Ok(())
    }

    fn try_lock_exclusive(&self) -> Result<()> {
        if self.holds_lock.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut locked = self.file.locked.lock().map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Internal,
                MODULE_ID,
                "MemoryFileHandle::try_lock_exclusive",
                "memory file lock poisoned",
            )
        })?;
        if *locked {
            return Err(ErrorEnvelope::new(
                ErrorCategory::LockedByProcess,
                MODULE_ID,
                "MemoryFileHandle::try_lock_exclusive",
                "in-memory file is already exclusively locked",
            ));
        }
        *locked = true;
        self.holds_lock.store(true, Ordering::Release);
        Ok(())
    }

    fn unlock(&self) -> Result<()> {
        if self.holds_lock.swap(false, Ordering::AcqRel) {
            let mut locked = self.file.locked.lock().map_err(|_| {
                ErrorEnvelope::new(
                    ErrorCategory::Internal,
                    MODULE_ID,
                    "MemoryFileHandle::unlock",
                    "memory file lock poisoned",
                )
            })?;
            *locked = false;
        }
        Ok(())
    }

    fn identity(&self) -> &FileIdentity {
        &self.identity
    }
}

impl StorageIo for MemoryVfs {
    fn open(&self, path: &Path, mode: OpenMode) -> Result<Arc<dyn FileHandle>> {
        let key = path.to_path_buf();
        let mut files = self.files.lock().map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Internal,
                MODULE_ID,
                "MemoryVfs::open",
                "memory VFS map lock poisoned",
            )
        })?;

        let file = match files.get(&key) {
            Some(existing) => {
                if mode.create_new {
                    return Err(ErrorEnvelope::new(
                        ErrorCategory::Io,
                        MODULE_ID,
                        "MemoryVfs::open",
                        format!("virtual file already exists: {}", path.display()),
                    ));
                }
                Arc::clone(existing)
            }
            None => {
                if !mode.create && !mode.create_new {
                    return Err(ErrorEnvelope::new(
                        ErrorCategory::NotFound,
                        MODULE_ID,
                        "MemoryVfs::open",
                        format!("virtual file not found: {}", path.display()),
                    ));
                }
                let created = Arc::new(MemoryFile::default());
                files.insert(key, Arc::clone(&created));
                created
            }
        };

        if mode.truncate && mode.write {
            let mut data = file.data.lock().map_err(|_| {
                ErrorEnvelope::new(
                    ErrorCategory::Internal,
                    MODULE_ID,
                    "MemoryVfs::open",
                    "memory file lock poisoned",
                )
            })?;
            data.clear();
        }

        let identity = FileIdentity::Path(format!("memory://{}", path.display()));
        Ok(Arc::new(MemoryFileHandle {
            file,
            identity,
            readable: mode.read,
            writable: mode.write,
            holds_lock: AtomicBool::new(false),
        }))
    }

    fn sync_parent_dir(&self, _path: &Path) -> Result<()> {
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> Result<()> {
        let mut files = self.files.lock().map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Internal,
                MODULE_ID,
                "MemoryVfs::remove_file",
                "memory VFS map lock poisoned",
            )
        })?;
        if files.remove(path).is_none() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::NotFound,
                MODULE_ID,
                "MemoryVfs::remove_file",
                format!("virtual file not found: {}", path.display()),
            ));
        }
        Ok(())
    }

    fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        let mut files = self.files.lock().map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Internal,
                MODULE_ID,
                "MemoryVfs::rename",
                "memory VFS map lock poisoned",
            )
        })?;
        let file = files.remove(from).ok_or_else(|| {
            ErrorEnvelope::new(
                ErrorCategory::NotFound,
                MODULE_ID,
                "MemoryVfs::rename",
                format!("virtual source file not found: {}", from.display()),
            )
        })?;
        files.insert(to.to_path_buf(), file);
        Ok(())
    }

    fn exists(&self, path: &Path) -> bool {
        self.files
            .lock()
            .map(|g| g.contains_key(path))
            .unwrap_or(false)
    }

    fn file_registry(&self) -> Arc<FileRegistry> {
        Arc::clone(&self.registry)
    }
}
