//! Bounded pin-aware page cache and MVCC page-version metadata index.

pub mod pool;
pub mod versions;

pub use pool::{CacheStats, PageCache, PinnedPage};
pub use versions::{PageVersionEntry, PageVersionLocation, VersionIndex};
