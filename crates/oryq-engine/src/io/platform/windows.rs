//! Windows platform I/O adapter (`seek_read`/`seek_write`, canonical path identity, `LockFileEx`).

use std::fs::File;
use std::io;
use std::os::windows::fs::FileExt;
use std::path::Path;

use fs4::fs_std::FileExt as LockExt;
use oryq_types::{ErrorCategory, ErrorEnvelope, Result};

use crate::io::registry::FileIdentity;

const MODULE_ID: &str = "engine";

pub fn read_exact_at(file: &File, mut buf: &mut [u8], mut offset: u64) -> Result<()> {
    while !buf.is_empty() {
        let n = file.seek_read(buf, offset).map_err(|err| {
            ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "platform::windows::read_exact_at",
                format!("seek_read failed at offset {offset}: {err}"),
            )
            .with_source(err)
        })?;
        if n == 0 {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "platform::windows::read_exact_at",
                format!("unexpected EOF at offset {offset}"),
            ));
        }
        buf = &mut buf[n..];
        offset += n as u64;
    }
    Ok(())
}

pub fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> Result<()> {
    while !buf.is_empty() {
        let n = file.seek_write(buf, offset).map_err(|err| {
            ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "platform::windows::write_all_at",
                format!("seek_write failed at offset {offset}: {err}"),
            )
            .with_source(err)
        })?;
        if n == 0 {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "platform::windows::write_all_at",
                format!("short write (0 bytes) at offset {offset}"),
            ));
        }
        buf = &buf[n..];
        offset += n as u64;
    }
    Ok(())
}

pub fn file_identity(_file: &File, path: &Path) -> Result<FileIdentity> {
    let canonical = std::fs::canonicalize(path).map_err(|err| {
        ErrorEnvelope::new(
            ErrorCategory::Io,
            MODULE_ID,
            "platform::windows::file_identity",
            format!("canonicalize failed for {}: {err}", path.display()),
        )
        .with_source(err)
    })?;
    Ok(FileIdentity::Path(
        canonical.to_string_lossy().to_lowercase(),
    ))
}

pub fn try_lock_exclusive(file: &File) -> Result<()> {
    match file.try_lock_exclusive() {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::WouldBlock => Err(ErrorEnvelope::new(
            ErrorCategory::LockedByProcess,
            MODULE_ID,
            "platform::windows::try_lock_exclusive",
            "database file is exclusively locked by another handle or process",
        )
        .with_source(err)),
        Err(err) => Err(ErrorEnvelope::new(
            ErrorCategory::Io,
            MODULE_ID,
            "platform::windows::try_lock_exclusive",
            format!("exclusive file lock failed: {err}"),
        )
        .with_source(err)),
    }
}

pub fn unlock(file: &File) -> Result<()> {
    LockExt::unlock(file).map_err(|err| {
        ErrorEnvelope::new(
            ErrorCategory::Io,
            MODULE_ID,
            "platform::windows::unlock",
            format!("file unlock failed: {err}"),
        )
        .with_source(err)
    })
}

pub fn sync_parent_dir(_path: &Path) -> Result<()> {
    // Windows does not support fsync on directory handles via std::fs::File;
    // file-level FlushFileBuffers (sync_all) flushes NTFS metadata.
    Ok(())
}
