//! Authoritative encoders and decoders for OryqDB persistent byte formats
//! (`docs/file-format.md` §4.1 and §4.3).

pub mod header;
pub mod page;
pub mod wal;

pub use header::{
    select_active_header, select_header_slots, write_page0_dual, write_page0_slot, HeaderSlot,
    StorageHeader, FORMAT_VERSION, ORYQ_MAGIC, STORAGE_HEADER_SIZE,
};
pub use page::{
    FreelistEntry, FreelistTrunkHeader, OverflowHeader, OverflowRef, PageFlags, PageHeader,
    PageType, SlotEntry, SlotFlags, SlottedPageMut, SlottedPageView, FREELIST_ENTRY_SIZE,
    FREELIST_HEADER_SIZE, OVERFLOW_HEADER_SIZE, OVERFLOW_REF_SIZE, PAGE_HEADER_SIZE,
    SLOT_ENTRY_SIZE,
};
pub use wal::{
    CommitPayload, WalFrameHeader, WalFrameKind, WalHeader, WAL_COMMIT_PAYLOAD_SIZE,
    WAL_FRAME_HEADER_SIZE, WAL_HEADER_SIZE, WAL_MAGIC,
};
