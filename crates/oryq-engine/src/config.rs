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
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            page_size: PageSize::Size8192,
            sync_mode: SyncMode::Full,
            create_if_missing: true,
            read_only: false,
        }
    }
}
