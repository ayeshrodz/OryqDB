//! Unix platform I/O adapter (`pread`/`pwrite`, `(st_dev, st_ino)` identity, `flock`, dir `fsync`).

use std::fs::File;
use std::io;
use std::os::unix::fs::{FileExt, MetadataExt};
use std::path::Path;

use fs4::fs_std::FileExt as LockExt;
use oryq_types::{ErrorCategory, ErrorEnvelope, Result};

use crate::io::registry::FileIdentity;

const MODULE_ID: &str = "engine";

pub fn read_exact_at(file: &File, buf: &mut [u8], offset: u64) -> Result<()> {
    file.read_exact_at(buf, offset).map_err(|err| {
        ErrorEnvelope::new(
            ErrorCategory::Io,
            MODULE_ID,
            "platform::unix::read_exact_at",
            format!(
                "pread failed at offset {offset} ({} bytes): {err}",
                buf.len()
            ),
        )
        .with_source(err)
    })
}

pub fn write_all_at(file: &File, buf: &[u8], offset: u64) -> Result<()> {
    file.write_all_at(buf, offset).map_err(|err| {
        ErrorEnvelope::new(
            ErrorCategory::Io,
            MODULE_ID,
            "platform::unix::write_all_at",
            format!(
                "pwrite failed at offset {offset} ({} bytes): {err}",
                buf.len()
            ),
        )
        .with_source(err)
    })
}

pub fn file_identity(file: &File, _path: &Path) -> Result<FileIdentity> {
    let meta = file.metadata().map_err(|err| {
        ErrorEnvelope::new(
            ErrorCategory::Io,
            MODULE_ID,
            "platform::unix::file_identity",
            format!("fstat failed: {err}"),
        )
        .with_source(err)
    })?;
    Ok(FileIdentity::Unix {
        dev: meta.dev(),
        ino: meta.ino(),
    })
}

pub fn try_lock_exclusive(file: &File) -> Result<()> {
    match file.try_lock_exclusive() {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::WouldBlock => Err(ErrorEnvelope::new(
            ErrorCategory::LockedByProcess,
            MODULE_ID,
            "platform::unix::try_lock_exclusive",
            "database file is exclusively locked by another handle or process",
        )
        .with_source(err)),
        Err(err) => Err(ErrorEnvelope::new(
            ErrorCategory::Io,
            MODULE_ID,
            "platform::unix::try_lock_exclusive",
            format!("exclusive file lock failed: {err}"),
        )
        .with_source(err)),
    }
}

pub fn try_lock_shared(file: &File) -> Result<()> {
    match LockExt::try_lock_shared(file) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::WouldBlock => Err(ErrorEnvelope::new(
            ErrorCategory::LockedByProcess,
            MODULE_ID,
            "platform::unix::try_lock_shared",
            "database file is exclusively locked by another handle or process",
        )
        .with_source(err)),
        Err(err) => Err(ErrorEnvelope::new(
            ErrorCategory::Io,
            MODULE_ID,
            "platform::unix::try_lock_shared",
            format!("shared file lock failed: {err}"),
        )
        .with_source(err)),
    }
}

pub fn unlock(file: &File) -> Result<()> {
    LockExt::unlock(file).map_err(|err| {
        ErrorEnvelope::new(
            ErrorCategory::Io,
            MODULE_ID,
            "platform::unix::unlock",
            format!("file unlock failed: {err}"),
        )
        .with_source(err)
    })
}

pub fn sync_parent_dir(path: &Path) -> Result<()> {
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let dir = File::open(parent).map_err(|err| {
        ErrorEnvelope::new(
            ErrorCategory::Io,
            MODULE_ID,
            "platform::unix::sync_parent_dir",
            format!(
                "failed to open parent directory {}: {err}",
                parent.display()
            ),
        )
        .with_source(err)
    })?;
    dir.sync_all().map_err(|err| {
        ErrorEnvelope::new(
            ErrorCategory::Io,
            MODULE_ID,
            "platform::unix::sync_parent_dir",
            format!(
                "fsync on parent directory {} failed: {err}",
                parent.display()
            ),
        )
        .with_source(err)
    })
}
