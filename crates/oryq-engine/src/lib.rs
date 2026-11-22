#![forbid(unsafe_code)]

//! `oryq-engine`: Physical storage, positioned VFS/PAL, Page 0 dual shadow headers,
//! slotted pages, freelist, overflow chains, bounded page cache, version metadata,
//! logical-page byte B+ trees, WAL, single-process MVCC transactions, savepoints,
//! checkpointing, and restartable crash recovery.

pub mod btree;
pub mod cache;
pub mod checkpoint;
pub mod config;
pub mod coordinator;
pub mod format;
pub mod inspect;
pub mod io;
pub mod pager;
pub mod ports;
pub mod recovery;
pub mod transaction;

pub use btree::{
    BTree, BTreeCursor, InternalNode, LeafEntry, PageStore, PageStoreMut, PagerPageStore, TreeKind,
};
pub use cache::{
    CacheStats, PageCache, PageVersionEntry, PageVersionLocation, PinnedPage, VersionIndex,
};
pub use config::{EngineConfig, PageSize, SyncMode};
pub use coordinator::{
    shm_path_for, SharedProcessConfig, SharedProcessCoordinator, SharedSnapshotGuard,
    SharedWriterPermit, ShmInspectionReport, SingleProcessCoordinator, SnapshotGuard,
    StaleReclaimReport, WalResetGuard, WriterPermit, MAX_SHM_READERS,
};
pub use format::{
    CommitPayload, FreelistEntry, FreelistTrunkHeader, HeaderSlot, OverflowHeader, OverflowRef,
    PageFlags, PageHeader, PageType, SlotEntry, SlotFlags, SlottedPageMut, SlottedPageView,
    StorageHeader, WalFrameHeader, WalFrameKind, WalHeader, FORMAT_VERSION, ORYQ_MAGIC,
    PAGE_HEADER_SIZE, SLOT_ENTRY_SIZE, STORAGE_HEADER_SIZE, WAL_COMMIT_PAYLOAD_SIZE,
    WAL_FRAME_HEADER_SIZE, WAL_HEADER_SIZE, WAL_MAGIC,
};
pub use inspect::{
    inspect_locks_readonly, inspect_page_readonly, inspect_wal_readonly, LockInspectionReport,
    PageInspectionReport, WalInspectionReport,
};
pub use io::{FileHandle, FileIdentity, FileLease, FileRegistry, OpenMode, OsVfs, StorageIo};
pub use oryq_types::{PageId, Result, SnapshotId, TxId};
pub use pager::{FreelistManager, OverflowStore, PageAllocator, PageBuf, PageGuard, Pager};
pub use ports::{CheckpointOutcome, CommitReceipt, RecordCompactor};
pub use recovery::{CommittedDbMeta, RecoveredPageFrame, RecoveredTx, WalScanResult};
pub use transaction::{
    clone_error_envelope, commit_batch, wal_path_for, BatchCommitReport, BatchItem,
    GroupCommitConfig, GroupCommitQueue, ReadTxn, SavepointId, StorageEngine, WriteTxn,
};
