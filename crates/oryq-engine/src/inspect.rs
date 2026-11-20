//! Read-only, non-mutating forensic inspection of `.oryq` pages, `.wal` files, and OS locks
//! (`docs/modules/engine.md` & `W14`).

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;
use oryq_types::{ErrorCategory, ErrorEnvelope, PageId, Result};

use crate::format::{
    select_header_slots, SlottedPageView, StorageHeader, PAGE_HEADER_SIZE, SLOT_ENTRY_SIZE,
};
use crate::io::{OpenMode, OsVfs, StorageIo};
use crate::recovery::scan_wal;
use crate::transaction::wal_path_for;

const MODULE_ID: &str = "engine";

/// Bounded forensic summary of a single physical page in an `.oryq` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageInspectionReport {
    pub page_id: u64,
    pub page_size: u32,
    pub page_type: String,
    pub slot_count: u16,
    pub free_bytes: usize,
    pub checksum_valid: bool,
    pub header_generation: Option<u64>,
    pub wal_required: Option<bool>,
}

/// Bounded forensic summary of the companion `.wal` file without mutating or replaying it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalInspectionReport {
    pub wal_path: PathBuf,
    pub wal_exists: bool,
    pub wal_bytes: u64,
    pub committed_txns: usize,
    pub valid_bytes: u64,
    pub trailing_bytes: u64,
}

/// Non-mutating OS file-lock availability probe for an `.oryq` database file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockInspectionReport {
    pub db_path: PathBuf,
    pub file_exists: bool,
    pub file_bytes: u64,
    pub exclusive_lock_available: bool,
}

/// Inspects a single page (`page_id`) of `db_path` read-only without recovering or mutating.
pub fn inspect_page_readonly(db_path: &Path, page_id: u64) -> Result<PageInspectionReport> {
    let mut file = File::open(db_path).map_err(|err| {
        ErrorEnvelope::new(
            ErrorCategory::Io,
            MODULE_ID,
            "inspect::inspect_page_readonly",
            format!("failed to open '{}' read-only: {err}", db_path.display()),
        )
        .with_source(err)
    })?;

    let mut probe = [0u8; 512];
    file.read_exact(&mut probe).map_err(|err| {
        ErrorEnvelope::new(
            ErrorCategory::Corruption,
            MODULE_ID,
            "inspect::inspect_page_readonly",
            format!("database file shorter than 512-byte Page 0 probe: {err}"),
        )
        .with_source(err)
    })?;
    let (_, active_hdr): (_, StorageHeader) = select_header_slots(&probe)?;
    let page_size = active_hdr.page_size.bytes() as u32;

    if page_id == 0 {
        return Ok(PageInspectionReport {
            page_id: 0,
            page_size,
            page_type: "StorageHeader".to_owned(),
            slot_count: 2,
            free_bytes: (page_size as usize).saturating_sub(512),
            checksum_valid: true,
            header_generation: Some(active_hdr.header_generation),
            wal_required: Some(active_hdr.wal_required),
        });
    }

    let offset = page_id.checked_mul(u64::from(page_size)).ok_or_else(|| {
        ErrorEnvelope::new(
            ErrorCategory::Overflow,
            MODULE_ID,
            "inspect::inspect_page_readonly",
            "page offset overflow",
        )
    })?;
    let file_len = file
        .metadata()
        .map_err(|err| {
            ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "inspect::inspect_page_readonly",
                format!("stat failed: {err}"),
            )
            .with_source(err)
        })?
        .len();
    if offset + u64::from(page_size) > file_len {
        return Err(ErrorEnvelope::new(
            ErrorCategory::NotFound,
            MODULE_ID,
            "inspect::inspect_page_readonly",
            format!("page {page_id} is out of bounds for file length {file_len}"),
        ));
    }

    file.seek(SeekFrom::Start(offset)).map_err(|err| {
        ErrorEnvelope::new(
            ErrorCategory::Io,
            MODULE_ID,
            "inspect::inspect_page_readonly",
            format!("seek failed: {err}"),
        )
        .with_source(err)
    })?;
    let mut page_buf = vec![0u8; page_size as usize];
    file.read_exact(&mut page_buf).map_err(|err| {
        ErrorEnvelope::new(
            ErrorCategory::Io,
            MODULE_ID,
            "inspect::inspect_page_readonly",
            format!("read failed: {err}"),
        )
        .with_source(err)
    })?;

    match SlottedPageView::parse(&page_buf, PageId::new(page_id)) {
        Ok(view) => {
            let hdr = view.header();
            let slots_end = PAGE_HEADER_SIZE + usize::from(hdr.slot_count) * SLOT_ENTRY_SIZE;
            let free_bytes = usize::from(hdr.free_space_end).saturating_sub(slots_end);
            Ok(PageInspectionReport {
                page_id,
                page_size,
                page_type: format!("{:?}", hdr.page_type),
                slot_count: hdr.slot_count,
                free_bytes,
                checksum_valid: true,
                header_generation: None,
                wal_required: None,
            })
        }
        Err(_) => Ok(PageInspectionReport {
            page_id,
            page_size,
            page_type: "UnparseableOrCorrupt".to_owned(),
            slot_count: 0,
            free_bytes: 0,
            checksum_valid: false,
            header_generation: None,
            wal_required: None,
        }),
    }
}

/// Inspects the companion `.wal` file for `db_path` read-only without mutating or replaying it.
pub fn inspect_wal_readonly(db_path: &Path) -> Result<WalInspectionReport> {
    let wal_path = wal_path_for(db_path);
    if !wal_path.exists() {
        return Ok(WalInspectionReport {
            wal_path,
            wal_exists: false,
            wal_bytes: 0,
            committed_txns: 0,
            valid_bytes: 0,
            trailing_bytes: 0,
        });
    }

    let wal_bytes = std::fs::metadata(&wal_path)
        .map_err(|err| {
            ErrorEnvelope::new(
                ErrorCategory::Io,
                MODULE_ID,
                "inspect::inspect_wal_readonly",
                format!("stat wal failed: {err}"),
            )
            .with_source(err)
        })?
        .len();

    let mut db_file = File::open(db_path).map_err(|err| {
        ErrorEnvelope::new(
            ErrorCategory::Io,
            MODULE_ID,
            "inspect::inspect_wal_readonly",
            format!("failed to open '{}' read-only: {err}", db_path.display()),
        )
        .with_source(err)
    })?;
    let mut probe = [0u8; 512];
    db_file.read_exact(&mut probe).map_err(|err| {
        ErrorEnvelope::new(
            ErrorCategory::Corruption,
            MODULE_ID,
            "inspect::inspect_wal_readonly",
            format!("failed reading Page 0 probe: {err}"),
        )
        .with_source(err)
    })?;
    let (_, active_hdr) = select_header_slots(&probe)?;
    let vfs = OsVfs::new();
    let wal_fh = vfs.open(&wal_path, OpenMode::READ_ONLY)?;

    match scan_wal(
        wal_fh.as_ref(),
        active_hdr.database_uuid,
        active_hdr.page_size,
        active_hdr.checkpointed_wal_seq,
    ) {
        Ok(res) => Ok(WalInspectionReport {
            wal_path,
            wal_exists: true,
            wal_bytes,
            committed_txns: res.committed_txs.len(),
            valid_bytes: res.last_valid_offset,
            trailing_bytes: wal_bytes.saturating_sub(res.last_valid_offset),
        }),
        Err(_) => Ok(WalInspectionReport {
            wal_path,
            wal_exists: true,
            wal_bytes,
            committed_txns: 0,
            valid_bytes: 0,
            trailing_bytes: wal_bytes,
        }),
    }
}

/// Probes OS exclusive lock availability on `db_path` without modifying the file.
pub fn inspect_locks_readonly(db_path: &Path) -> Result<LockInspectionReport> {
    if !db_path.exists() {
        return Ok(LockInspectionReport {
            db_path: db_path.to_path_buf(),
            file_exists: false,
            file_bytes: 0,
            exclusive_lock_available: false,
        });
    }
    let meta = std::fs::metadata(db_path).map_err(|err| {
        ErrorEnvelope::new(
            ErrorCategory::Io,
            MODULE_ID,
            "inspect::inspect_locks_readonly",
            format!("stat failed: {err}"),
        )
        .with_source(err)
    })?;
    let file = OpenOptions::new().read(true).open(db_path).map_err(|err| {
        ErrorEnvelope::new(
            ErrorCategory::Io,
            MODULE_ID,
            "inspect::inspect_locks_readonly",
            format!("open failed: {err}"),
        )
        .with_source(err)
    })?;

    let available = match file.try_lock_exclusive() {
        Ok(()) => {
            let _ = FileExt::unlock(&file);
            true
        }
        Err(_) => false,
    };

    Ok(LockInspectionReport {
        db_path: db_path.to_path_buf(),
        file_exists: true,
        file_bytes: meta.len(),
        exclusive_lock_available: available,
    })
}
