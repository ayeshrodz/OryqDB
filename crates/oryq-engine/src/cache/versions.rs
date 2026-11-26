//! Bounded MVCC page-version metadata index for snapshot resolution and WAL retention.

use std::collections::HashMap;

use oryq_types::{ErrorCategory, ErrorEnvelope, PageId, Result, SnapshotId, TxId};

const MODULE_ID: &str = "engine";

/// Physical location of a committed page version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageVersionLocation {
    /// Stored directly in the main `.oryq` database file at `page_id * page_size`.
    MainFile,
    /// Stored in `.oryq-wal` at a specific `frame_seq` and byte offset.
    WalFrame { frame_seq: u64, file_offset: u64 },
}

/// Metadata for a single committed version of a logical page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageVersionEntry {
    pub tx_id: TxId,
    pub location: PageVersionLocation,
}

/// Bounded in-memory index mapping each [`PageId`] to its committed version chain
/// ordered by strictly increasing [`TxId`].
#[derive(Debug)]
pub struct VersionIndex {
    versions: HashMap<PageId, Vec<PageVersionEntry>>,
    total_entries: usize,
    max_entries: usize,
}

impl VersionIndex {
    /// Creates a new `VersionIndex` capped at `max_entries` total version metadata records.
    pub fn new(max_entries: usize) -> Self {
        Self {
            versions: HashMap::new(),
            total_entries: 0,
            max_entries,
        }
    }

    #[inline]
    pub fn total_entries(&self) -> usize {
        self.total_entries
    }

    #[inline]
    pub fn max_entries(&self) -> usize {
        self.max_entries
    }

    /// Returns `true` if recording `additional` page versions stays within `max_entries`.
    #[inline]
    pub fn can_accommodate(&self, additional: usize) -> bool {
        self.total_entries.saturating_add(additional) <= self.max_entries
    }

    /// Records a newly committed version for `page_id`.
    ///
    /// Enforces strictly monotonic `tx_id` per page and the `max_entries` quota.
    pub fn record_version(&mut self, page_id: PageId, entry: PageVersionEntry) -> Result<()> {
        if self.total_entries >= self.max_entries {
            return Err(ErrorEnvelope::new(
                ErrorCategory::QuotaExceeded,
                MODULE_ID,
                "VersionIndex::record_version",
                format!(
                    "version metadata index reached max_entries limit ({})",
                    self.max_entries
                ),
            ));
        }

        let chain = self.versions.entry(page_id).or_default();
        if let Some(last) = chain.last() {
            if entry.tx_id <= last.tx_id {
                return Err(ErrorEnvelope::new(
                    ErrorCategory::InvalidArgument,
                    MODULE_ID,
                    "VersionIndex::record_version",
                    format!(
                        "non-monotonic tx_id {} after {} for page {}",
                        entry.tx_id.raw(),
                        last.tx_id.raw(),
                        page_id.raw()
                    ),
                ));
            }
        }
        chain.push(entry);
        self.total_entries += 1;
        Ok(())
    }

    /// Resolves the newest committed version of `page_id` visible to `snapshot_id`
    /// (`entry.tx_id <= snapshot_id`).
    pub fn resolve_for_snapshot(
        &self,
        page_id: PageId,
        snapshot_id: SnapshotId,
    ) -> Option<PageVersionEntry> {
        let chain = self.versions.get(&page_id)?;
        let idx = chain.partition_point(|e| e.tx_id <= snapshot_id);
        if idx == 0 {
            None
        } else {
            Some(chain[idx - 1])
        }
    }

    /// Prunes superseded versions that are strictly older than the version visible to
    /// `min_active_snapshot_id`. Always retains at least the latest version `<= min_active_snapshot_id`
    /// (if any) so active snapshots never lose visibility.
    pub fn prune_below(&mut self, min_active_snapshot_id: SnapshotId) -> usize {
        let mut pruned = 0usize;
        self.versions.retain(|_, chain| {
            let visible_count = chain.partition_point(|e| e.tx_id <= min_active_snapshot_id);
            if visible_count > 1 {
                let drop_count = visible_count - 1;
                chain.drain(0..drop_count);
                pruned += drop_count;
            }
            !chain.is_empty()
        });
        self.total_entries = self.total_entries.saturating_sub(pruned);
        pruned
    }
}
