//! Physical engine configuration options.

use oryq_types::{ErrorCategory, ErrorEnvelope, Result};

const MODULE_ID: &str = "engine";

/// Supported persistent `.oryq` logical page sizes (`docs/file-format.md` §4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PageSize {
    Size4096 = 12,
    #[default]
    Size8192 = 13,
    Size16384 = 14,
}

impl PageSize {
    #[inline]
    pub const fn log2(self) -> u16 {
        self as u16
    }

    #[inline]
    pub const fn bytes(self) -> usize {
        1usize << (self as usize)
    }

    pub fn from_log2(log2: u16) -> Result<Self> {
        match log2 {
            12 => Ok(Self::Size4096),
            13 => Ok(Self::Size8192),
            14 => Ok(Self::Size16384),
            other => Err(ErrorEnvelope::new(
                ErrorCategory::Corruption,
                MODULE_ID,
                "PageSize::from_log2",
                format!("unsupported page_size_log2 {other}; expected 12, 13, or 14"),
            )),
        }
    }
}

/// Durability sync policy for persistent flushes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SyncMode {
    /// Flush both file data and metadata (`sync_all`) and parent directory on creation/rename.
    #[default]
    Full,
    /// Flush file data (`sync_data`) on normal commits.
    Normal,
    /// Skip OS fsync calls (for ephemeral or benchmark-only configurations).
    Off,
}

/// Physical storage engine configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineConfig {
    pub page_size: PageSize,
    pub sync_mode: SyncMode,
    pub create_if_missing: bool,
    pub read_only: bool,
    /// Maximum cached page bytes in [`crate::cache::PageCache`].
    pub max_cache_bytes: usize,
    /// Maximum version metadata entries retained in [`crate::cache::VersionIndex`].
    pub max_version_entries: usize,
    /// Hard cap on `.oryq-wal` byte size before rejecting new writes (`QuotaExceeded`).
    pub max_wal_hard_bytes: u64,
    /// Maximum concurrent active read snapshots in [`crate::coordinator::SingleProcessCoordinator`].
    pub max_readers: usize,
    /// When `true` (and not `read_only`), dropping [`crate::StorageEngine`] automatically
    /// checkpoints, seals, and removes the empty `.oryq-wal` sidecar so only the single `.oryq`
    /// file remains on disk.
    pub auto_checkpoint_on_close: bool,
    /// Soft `.oryq-wal` byte threshold (`0` disables automatic checkpointing) above which a
    /// committing writer automatically triggers a passive/reset checkpoint (`4 MiB` default).
    pub wal_autocheckpoint_bytes: u64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            page_size: PageSize::Size8192,
            sync_mode: SyncMode::Full,
            create_if_missing: true,
            read_only: false,
            max_cache_bytes: 16 * 1024 * 1024,
            max_version_entries: 65_536,
            max_wal_hard_bytes: 256 * 1024 * 1024,
            max_readers: 64,
            auto_checkpoint_on_close: false,
            wal_autocheckpoint_bytes: 4 * 1024 * 1024,
        }
    }
}
