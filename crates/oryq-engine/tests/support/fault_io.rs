#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use oryq_engine::{FileHandle, FileIdentity, FileRegistry, OpenMode, StorageIo};
use oryq_types::{ErrorCategory, ErrorEnvelope, Result};

const MODULE_ID: &str = "engine_test";

/// In-memory VFS with deterministic fault injection and operation counters.
#[derive(Debug)]
pub struct FaultVfs {
    files: Mutex<HashMap<PathBuf, Arc<MemFileState>>>,
    registry: Arc<FileRegistry>,
    pub sync_count: AtomicUsize,
    pub dir_sync_count: AtomicUsize,
    fail_write_after: Mutex<Option<usize>>,
    fail_sync_after: Mutex<Option<usize>>,
}

#[derive(Debug)]
struct MemFileState {
    data: Mutex<Vec<u8>>,
    locked: Mutex<bool>,
    identity: FileIdentity,
}

#[derive(Debug)]
struct FaultFileHandle {
    state: Arc<MemFileState>,
    vfs: Arc<FaultVfs>,
}

impl FaultVfs {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            files: Mutex::new(HashMap::new()),
            registry: Arc::new(FileRegistry::new()),
            sync_count: AtomicUsize::new(0),
            dir_sync_count: AtomicUsize::new(0),
            fail_write_after: Mutex::new(None),
            fail_sync_after: Mutex::new(None),
        })
    }

    pub fn fail_next_write(&self) {
        *self.fail_write_after.lock().unwrap() = Some(0);
    }

    pub fn fail_next_sync(&self) {
        *self.fail_sync_after.lock().unwrap() = Some(0);
    }

    pub fn corrupt_bytes(&self, path: &Path, offset: usize, bytes: &[u8]) {
        let files = self.files.lock().unwrap();
        let state = files.get(path).expect("file must exist");
        let mut data = state.data.lock().unwrap();
        data[offset..offset + bytes.len()].copy_from_slice(bytes);
    }

    pub fn snapshot_bytes(&self, path: &Path) -> Vec<u8> {
        let files = self.files.lock().unwrap();
        let state = files.get(path).expect("file must exist");
        let out = state.data.lock().unwrap().clone();
        out
    }

    fn check_write_fault(&self) -> Result<()> {
        let mut guard = self.fail_write_after.lock().unwrap();
        if let Some(rem) = *guard {
            if rem == 0 {
                *guard = None;
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Io,
                    MODULE_ID,
                    "FaultVfs::write_all_at",
                    "injected write I/O failure",
                ));
            }
            *guard = Some(rem - 1);
        }
        Ok(())
    }

    fn check_sync_fault(&self) -> Result<()> {
        let mut guard = self.fail_sync_after.lock().unwrap();
        if let Some(rem) = *guard {
            if rem == 0 {
                *guard = None;
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Io,
                    MODULE_ID,
                    "FaultVfs::sync",
                    "injected sync I/O failure",
                ));
            }
            *guard = Some(rem - 1);
        }
        Ok(())
    }
}

impl FileHandle for FaultFileHandle {
    fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> Result<()> {
        let data = self.state.data.lock().unwrap();
        let start = offset as usize;
        let end = start + buf.len();
        if end > data.len() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "FaultFileHandle::read_exact_at",
                format!(
                    "unexpected EOF reading [{start}..{end}) of len {}",
                    data.len()
                ),
            ));
        }
        buf.copy_from_slice(&data[start..end]);
        Ok(())
    }

    fn write_all_at(&self, buf: &[u8], offset: u64) -> Result<()> {
        self.vfs.check_write_fault()?;
        let mut data = self.state.data.lock().unwrap();
        let start = offset as usize;
        let end = start + buf.len();
        if end > data.len() {
            data.resize(end, 0);
        }
        data[start..end].copy_from_slice(buf);
        Ok(())
    }

    fn sync_data(&self) -> Result<()> {
        self.vfs.check_sync_fault()?;
        self.vfs.sync_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn sync_all(&self) -> Result<()> {
        self.vfs.check_sync_fault()?;
        self.vfs.sync_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn len(&self) -> Result<u64> {
        Ok(self.state.data.lock().unwrap().len() as u64)
    }

    fn set_len(&self, size: u64) -> Result<()> {
        self.state.data.lock().unwrap().resize(size as usize, 0);
        Ok(())
    }

    fn try_lock_exclusive(&self) -> Result<()> {
        let mut locked = self.state.locked.lock().unwrap();
        if *locked {
            return Err(ErrorEnvelope::new(
                ErrorCategory::LockedByProcess,
                MODULE_ID,
                "FaultFileHandle::try_lock_exclusive",
                "file already exclusively locked",
            ));
        }
        *locked = true;
        Ok(())
    }

    fn unlock(&self) -> Result<()> {
        *self.state.locked.lock().unwrap() = false;
        Ok(())
    }

    fn identity(&self) -> &FileIdentity {
        &self.state.identity
    }
}

/// Wrapper implementing `StorageIo` for `Arc<FaultVfs>` so tests can keep a handle to `FaultVfs`.
#[derive(Debug, Clone)]
pub struct FaultVfsIo(pub Arc<FaultVfs>);

impl StorageIo for FaultVfsIo {
    fn open(&self, path: &Path, mode: OpenMode) -> Result<Arc<dyn FileHandle>> {
        let mut files = self.0.files.lock().unwrap();
        let state = if let Some(existing) = files.get(path) {
            if mode.create_new {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::Io,
                    MODULE_ID,
                    "FaultVfs::open",
                    "file already exists",
                ));
            }
            if mode.truncate {
                existing.data.lock().unwrap().clear();
            }
            Arc::clone(existing)
        } else if mode.create || mode.create_new {
            let st = Arc::new(MemFileState {
                data: Mutex::new(Vec::new()),
                locked: Mutex::new(false),
                identity: FileIdentity::Path(path.to_string_lossy().into_owned()),
            });
            files.insert(path.to_path_buf(), Arc::clone(&st));
            st
        } else {
            return Err(ErrorEnvelope::new(
                ErrorCategory::NotFound,
                MODULE_ID,
                "FaultVfs::open",
                format!("file {} not found", path.display()),
            ));
        };

        Ok(Arc::new(FaultFileHandle {
            state,
            vfs: Arc::clone(&self.0),
        }))
    }

    fn sync_parent_dir(&self, _path: &Path) -> Result<()> {
        self.0.dir_sync_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> Result<()> {
        self.0.files.lock().unwrap().remove(path);
        Ok(())
    }

    fn exists(&self, path: &Path) -> bool {
        self.0.files.lock().unwrap().contains_key(path)
    }

    fn file_registry(&self) -> Arc<FileRegistry> {
        Arc::clone(&self.0.registry)
    }
}
