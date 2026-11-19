//! Public [`DatabaseConfig`] facade options.

use oryq_engine::{EngineConfig, PageSize, SyncMode};

/// Application-facing configuration for opening or creating an OryqDB [`crate::Database`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseConfig {
    pub page_size: PageSize,
    pub sync_mode: SyncMode,
    pub create_if_missing: bool,
    pub read_only: bool,
    pub max_cache_bytes: usize,
    pub max_wal_hard_bytes: u64,
    pub max_readers: usize,
    pub max_document_bytes: usize,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        let eng = EngineConfig::default();
        Self {
            page_size: eng.page_size,
            sync_mode: eng.sync_mode,
            create_if_missing: eng.create_if_missing,
            read_only: eng.read_only,
            max_cache_bytes: eng.max_cache_bytes,
            max_wal_hard_bytes: eng.max_wal_hard_bytes,
            max_readers: eng.max_readers,
            max_document_bytes: 16 * 1024 * 1024,
        }
    }
}

impl DatabaseConfig {
    pub fn to_engine_config(&self) -> EngineConfig {
        EngineConfig {
            page_size: self.page_size,
            sync_mode: self.sync_mode,
            create_if_missing: self.create_if_missing,
            read_only: self.read_only,
            max_cache_bytes: self.max_cache_bytes,
            max_wal_hard_bytes: self.max_wal_hard_bytes,
            max_readers: self.max_readers,
            ..EngineConfig::default()
        }
    }
}
