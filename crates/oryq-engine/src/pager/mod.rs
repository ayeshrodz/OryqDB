//! Persistent slotted-page Pager managing Page 0 dual shadow headers, exclusive file
//! ownership (`FileRegistry` + OS lock), and validated logical page I/O.

pub mod allocator;
pub mod freelist;
pub mod guard;
pub mod overflow;

use std::path::Path;
use std::sync::Arc;

use oryq_types::{BudgetHandle, ErrorCategory, ErrorEnvelope, PageId, Result, TxId};

use crate::config::{EngineConfig, PageSize, SyncMode};
use crate::format::{
    select_active_header, select_header_slots, write_page0_dual, write_page0_slot, HeaderSlot,
    PageFlags, PageHeader, PageType, SlottedPageMut, StorageHeader,
};
use crate::io::{FileHandle, FileLease, OpenMode, StorageIo};

pub use allocator::PageAllocator;
pub use freelist::FreelistManager;
pub use guard::{PageBuf, PageGuard};
pub use overflow::OverflowStore;

const MODULE_ID: &str = "engine";

/// Physical pager owning an exclusively locked `.oryq` database file.
#[derive(Debug)]
pub struct Pager {
    io: Arc<dyn StorageIo>,
    file: Arc<dyn FileHandle>,
    _lease: FileLease,
    header: StorageHeader,
    active_slot: HeaderSlot,
    sync_mode: SyncMode,
    read_only: bool,
}

impl Pager {
    /// Creates a brand-new `.oryq` database file with dual identical shadow headers on Page 0
    /// and an empty initial catalog root page (`PageId(1)`), syncing file + parent directory.
    pub fn create_new(
        io: Arc<dyn StorageIo>,
        path: &Path,
        config: &EngineConfig,
        database_uuid: [u8; 16],
    ) -> Result<Self> {
        if config.read_only {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "Pager::create_new",
                "cannot create a new database in read_only mode",
            ));
        }
        let file = io.open(path, OpenMode::CREATE_NEW)?;
        let lease = io.file_registry().acquire(file.identity().clone())?;
        file.try_lock_exclusive()?;

        let header = StorageHeader::new_initial(config.page_size, database_uuid);
        let page_bytes = config.page_size.bytes();

        let mut page0 = vec![0u8; page_bytes];
        write_page0_dual(&header, &mut page0)?;

        let mut page1 = vec![0u8; page_bytes];
        let catalog_pid = header.catalog_root_page_id;
        let _ = SlottedPageMut::init(
            &mut page1,
            catalog_pid,
            PageType::ColLeaf,
            PageFlags::EMPTY.with(PageFlags::IS_ROOT_PAGE),
            TxId::INITIAL,
        )?;

        file.write_all_at(&page0, 0)?;
        file.write_all_at(&page1, page_bytes as u64)?;
        if config.sync_mode != SyncMode::Off {
            file.sync_all()?;
            io.sync_parent_dir(path)?;
        }

        Ok(Self {
            io,
            file,
            _lease: lease,
            header,
            active_slot: HeaderSlot::SlotA,
            sync_mode: config.sync_mode,
            read_only: false,
        })
    }

    /// Opens an existing `.oryq` database file, validates Page 0 dual shadow headers,
    /// and acquires exclusive process-local + OS locks.
    pub fn open_existing(
        io: Arc<dyn StorageIo>,
        path: &Path,
        config: &EngineConfig,
    ) -> Result<Self> {
        let mode = if config.read_only {
            OpenMode::READ_ONLY
        } else {
            OpenMode::READ_WRITE
        };
        let file = io.open(path, mode)?;
        let lease = io.file_registry().acquire(file.identity().clone())?;
        file.try_lock_exclusive()?;

        let file_len = file.len()?;
        if file_len < PageSize::Size4096.bytes() as u64 {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "Pager::open_existing",
                format!("database file too small ({file_len} bytes)"),
            ));
        }

        // Read the first 512 bytes (Slot A + Slot B) to discover `page_size`, then read full Page 0.
        let mut probe = [0u8; 512];
        file.read_exact_at(&mut probe, 0)?;
        let (_, probe_hdr) = select_header_slots(&probe)?;
        let page_bytes = probe_hdr.page_size.bytes();
        let expected_min_len = probe_hdr
            .total_page_count
            .checked_mul(page_bytes as u64)
            .ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::Corruption,
                    MODULE_ID,
                    "Pager::open_existing",
                    "total_page_count * page_size overflow",
                )
            })?;
        if file_len < expected_min_len {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "Pager::open_existing",
                format!(
                    "truncated database file: len={file_len}, expected at least {expected_min_len} bytes ({} pages)",
                    probe_hdr.total_page_count
                ),
            ));
        }

        let mut page0 = vec![0u8; page_bytes];
        file.read_exact_at(&mut page0, 0)?;
        let (active_slot, header) = select_active_header(&page0)?;

        Ok(Self {
            io,
            file,
            _lease: lease,
            header,
            active_slot,
            sync_mode: config.sync_mode,
            read_only: config.read_only,
        })
    }

    /// Opens an existing database or creates a new one if `config.create_if_missing` is true.
    pub fn open_or_create(
        io: Arc<dyn StorageIo>,
        path: &Path,
        config: &EngineConfig,
        database_uuid: [u8; 16],
    ) -> Result<Self> {
        if io.exists(path) {
            Self::open_existing(io, path, config)
        } else if config.create_if_missing {
            Self::create_new(io, path, config, database_uuid)
        } else {
            Err(ErrorEnvelope::new(
                ErrorCategory::NotFound,
                MODULE_ID,
                "Pager::open_or_create",
                format!("database file {} does not exist", path.display()),
            ))
        }
    }

    #[inline]
    pub fn header(&self) -> &StorageHeader {
        &self.header
    }

    #[inline]
    pub fn header_mut(&mut self) -> &mut StorageHeader {
        &mut self.header
    }

    #[inline]
    pub fn active_slot(&self) -> HeaderSlot {
        self.active_slot
    }

    #[inline]
    pub fn page_size(&self) -> PageSize {
        self.header.page_size
    }

    #[inline]
    pub fn io(&self) -> &Arc<dyn StorageIo> {
        &self.io
    }

    #[inline]
    pub fn total_page_count(&self) -> u64 {
        self.header.total_page_count
    }

    /// Reads and checksum-verifies a logical page (`PageId >= 1`).
    pub fn read_page(&self, page_id: PageId, budget: Option<&BudgetHandle>) -> Result<PageGuard> {
        if page_id == PageId::HEADER || page_id.raw() >= self.header.total_page_count {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "Pager::read_page",
                format!(
                    "page_id {} out of file bounds (total_page_count={})",
                    page_id.raw(),
                    self.header.total_page_count
                ),
            ));
        }
        let mut buf = PageBuf::zeroed(self.header.page_size, budget)?;
        let offset = page_id.raw() * self.header.page_size.bytes() as u64;
        self.file.read_exact_at(buf.as_mut_slice(), offset)?;
        let _ = PageHeader::decode_and_verify(buf.as_slice(), page_id)?;
        Ok(PageGuard::new(page_id, buf))
    }

    /// Writes a checksum-verified logical page (`PageId >= 1`) to disk at its page offset.
    pub fn write_page(&mut self, page_id: PageId, page_bytes: &[u8]) -> Result<()> {
        if self.read_only {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "Pager::write_page",
                "cannot write page to a read-only pager",
            ));
        }
        if page_id == PageId::HEADER {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "Pager::write_page",
                "cannot write Page 0 via write_page; use commit_header",
            ));
        }
        let expected_len = self.header.page_size.bytes();
        if page_bytes.len() != expected_len {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "Pager::write_page",
                format!(
                    "page slice len {} != page_size {expected_len}",
                    page_bytes.len()
                ),
            ));
        }
        // Always verify checksum and page_id before persisting.
        let _ = PageHeader::decode_and_verify(page_bytes, page_id)?;
        let offset = page_id.raw() * expected_len as u64;
        self.file.write_all_at(page_bytes, offset)?;
        if page_id.raw() >= self.header.total_page_count {
            self.header.total_page_count = page_id.raw() + 1;
        }
        Ok(())
    }

    /// Flushes data pages according to [`SyncMode`].
    pub fn sync_data(&self) -> Result<()> {
        match self.sync_mode {
            SyncMode::Full => self.file.sync_all(),
            SyncMode::Normal => self.file.sync_data(),
            SyncMode::Off => Ok(()),
        }
    }

    /// Atomically commits the current [`StorageHeader`] by incrementing `header_generation`,
    /// writing to the alternate shadow slot (A <-> B) on Page 0, and syncing.
    pub fn commit_header(&mut self) -> Result<()> {
        if self.read_only {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "Pager::commit_header",
                "cannot commit header on a read-only pager",
            ));
        }
        // Sync all data pages before updating Page 0 shadow header.
        self.sync_data()?;

        let next_slot = self.active_slot.alternate();
        let next_gen = self
            .header
            .header_generation
            .checked_add(1)
            .ok_or_else(|| {
                ErrorEnvelope::new(
                    ErrorCategory::Overflow,
                    MODULE_ID,
                    "Pager::commit_header",
                    "header_generation overflow",
                )
            })?;
        self.header.header_generation = next_gen;

        let mut page0 = vec![0u8; self.header.page_size.bytes()];
        self.file.read_exact_at(&mut page0, 0)?;
        write_page0_slot(&self.header, next_slot, &mut page0)?;

        let slot_off = next_slot.offset();
        self.file
            .write_all_at(&page0[slot_off..slot_off + 256], slot_off as u64)?;
        self.sync_data()?;

        self.active_slot = next_slot;
        Ok(())
    }
}

impl Drop for Pager {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}
