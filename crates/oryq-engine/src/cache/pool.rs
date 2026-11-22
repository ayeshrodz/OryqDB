//! Bounded, pin-aware LRU page cache backed by [`BudgetHandle`].
//!
//! Cached pages are stored as immutable `Arc<[u8]>`. A page is pinned whenever any
//! [`PinnedPage`] handle outside the cache still references its `Arc` (`strong_count > 1`).
//! Eviction skips pinned pages and returns [`ErrorCategory::QuotaExceeded`] if the
//! configured memory budget cannot be satisfied without evicting pinned pages.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use oryq_types::{
    BudgetDomain, BudgetHandle, ErrorCategory, ErrorEnvelope, PageId, Reservation, Result, TxId,
};

const MODULE_ID: &str = "engine";

/// Cache lookup key identifying an immutable logical page version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub page_id: PageId,
    pub tx_id: TxId,
}

/// Pinned, immutable reference to a cached page image.
///
/// While alive, this handle keeps `Arc::strong_count > 1`, preventing the [`PageCache`]
/// from evicting the underlying page.
#[derive(Debug, Clone)]
pub struct PinnedPage {
    key: CacheKey,
    bytes: Arc<[u8]>,
}

impl PinnedPage {
    #[inline]
    pub fn page_id(&self) -> PageId {
        self.key.page_id
    }

    #[inline]
    pub fn tx_id(&self) -> TxId {
        self.key.tx_id
    }

    #[inline]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[inline]
    pub fn as_arc(&self) -> Arc<[u8]> {
        Arc::clone(&self.bytes)
    }
}

/// Snapshot of cache hit/miss/eviction statistics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheStats {
    pub entries: usize,
    pub used_bytes: usize,
    pub limit_bytes: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

#[derive(Debug)]
struct CacheEntry {
    bytes: Arc<[u8]>,
    _reservation: Reservation,
    last_touch: u64,
}

#[derive(Debug)]
struct CacheInner {
    entries: HashMap<CacheKey, CacheEntry>,
    lru: VecDeque<(CacheKey, u64)>,
    clock: u64,
}

/// Bounded pin-aware page cache.
#[derive(Debug)]
pub struct PageCache {
    budget: BudgetHandle,
    inner: Mutex<CacheInner>,
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
}

impl PageCache {
    /// Creates a new `PageCache` bounded by `budget` (expected to be [`BudgetDomain::Cache`]).
    pub fn new(budget: BudgetHandle) -> Self {
        Self {
            budget,
            inner: Mutex::new(CacheInner {
                entries: HashMap::new(),
                lru: VecDeque::new(),
                clock: 0,
            }),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    /// Creates a new `PageCache` with its own [`BudgetDomain::Cache`] budget of `limit_bytes`.
    pub fn with_limit(limit_bytes: usize) -> Self {
        Self::new(BudgetHandle::new(BudgetDomain::Cache, limit_bytes))
    }

    /// Looks up `(page_id, tx_id)` in the cache and returns a pinned handle on hit.
    pub fn get(&self, page_id: PageId, tx_id: TxId) -> Option<PinnedPage> {
        let key = CacheKey { page_id, tx_id };
        let mut inner = self.inner.lock().ok()?;
        if inner.entries.contains_key(&key) {
            inner.clock = inner.clock.wrapping_add(1);
            let tick = inner.clock;
            let bytes = {
                let entry = inner.entries.get_mut(&key).unwrap();
                entry.last_touch = tick;
                Arc::clone(&entry.bytes)
            };
            inner.lru.push_back((key, tick));
            self.hits.fetch_add(1, Ordering::Relaxed);
            Some(PinnedPage { key, bytes })
        } else {
            self.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    /// Inserts an immutable page image into the cache, evicting unpinned LRU entries
    /// as needed to stay within `budget`.
    ///
    /// Returns [`ErrorCategory::QuotaExceeded`] if all resident entries are currently pinned
    /// and the budget cannot accommodate `bytes.len()`.
    pub fn insert(&self, page_id: PageId, tx_id: TxId, bytes: Arc<[u8]>) -> Result<PinnedPage> {
        let key = CacheKey { page_id, tx_id };
        let needed = bytes.len();
        let mut inner = self.inner.lock().map_err(|_| {
            ErrorEnvelope::new(
                ErrorCategory::Internal,
                MODULE_ID,
                "PageCache::insert",
                "page cache lock poisoned",
            )
        })?;

        if inner.entries.contains_key(&key) {
            inner.clock = inner.clock.wrapping_add(1);
            let tick = inner.clock;
            let existing = {
                let entry = inner.entries.get_mut(&key).unwrap();
                entry.last_touch = tick;
                Arc::clone(&entry.bytes)
            };
            inner.lru.push_back((key, tick));
            return Ok(PinnedPage {
                key,
                bytes: existing,
            });
        }

        // Try reserving from the cache budget; on QuotaExceeded, evict unpinned LRU entries.
        let reservation = loop {
            match self.budget.reserve(needed) {
                Ok(res) => break res,
                Err(err) if err.category() == ErrorCategory::QuotaExceeded => {
                    if !self.evict_one_unpinned(&mut inner) {
                        return Err(err);
                    }
                }
                Err(err) => return Err(err),
            }
        };

        inner.clock = inner.clock.wrapping_add(1);
        let tick = inner.clock;
        let pinned = PinnedPage {
            key,
            bytes: Arc::clone(&bytes),
        };
        inner.entries.insert(
            key,
            CacheEntry {
                bytes,
                _reservation: reservation,
                last_touch: tick,
            },
        );
        inner.lru.push_back((key, tick));
        Ok(pinned)
    }

    /// Inserts or replaces an entry for `(page_id, tx_id)` with `bytes`.
    ///
    /// Used when an in-progress writer updates the same `(page_id, tx_id)` in place.
    pub fn put_replace(
        &self,
        page_id: PageId,
        tx_id: TxId,
        bytes: Arc<[u8]>,
    ) -> Result<PinnedPage> {
        let key = CacheKey { page_id, tx_id };
        {
            let mut inner = self.inner.lock().map_err(|_| {
                ErrorEnvelope::new(
                    ErrorCategory::Internal,
                    MODULE_ID,
                    "PageCache::put_replace",
                    "page cache lock poisoned",
                )
            })?;
            if let Some(existing) = inner.entries.get(&key) {
                if existing.bytes.len() == bytes.len() {
                    inner.clock = inner.clock.wrapping_add(1);
                    let tick = inner.clock;
                    let entry = inner.entries.get_mut(&key).unwrap();
                    entry.bytes = Arc::clone(&bytes);
                    entry.last_touch = tick;
                    inner.lru.push_back((key, tick));
                    return Ok(PinnedPage { key, bytes });
                }
                inner.entries.remove(&key);
            }
        }
        self.insert(page_id, tx_id, bytes)
    }

    /// Removes a specific page version from the cache if it is not currently pinned.
    pub fn remove_if_unpinned(&self, page_id: PageId, tx_id: TxId) -> bool {
        let key = CacheKey { page_id, tx_id };
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        let can_remove = inner
            .entries
            .get(&key)
            .is_some_and(|e| Arc::strong_count(&e.bytes) == 1);
        if can_remove {
            inner.entries.remove(&key);
            true
        } else {
            false
        }
    }

    /// Evicts all currently unpinned entries from the cache, returning the number evicted.
    pub fn evict_all_unpinned(&self) -> usize {
        let Ok(mut inner) = self.inner.lock() else {
            return 0;
        };
        let mut removed = 0usize;
        inner.entries.retain(|_, entry| {
            if Arc::strong_count(&entry.bytes) == 1 {
                removed += 1;
                false
            } else {
                true
            }
        });
        if removed > 0 {
            self.evictions.fetch_add(removed as u64, Ordering::Relaxed);
        }
        removed
    }

    /// Returns a point-in-time snapshot of cache counters and budget usage.
    /// Clears all cached page entries and releases their budget reservations.
    pub fn clear(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.entries.clear();
            inner.lru.clear();
        }
    }

    /// Returns a point-in-time snapshot of cache utilization and hit/miss counters.
    pub fn stats(&self) -> CacheStats {
        let entries = self.inner.lock().map(|g| g.entries.len()).unwrap_or(0);
        CacheStats {
            entries,
            used_bytes: self.budget.used_bytes(),
            limit_bytes: self.budget.limit_bytes(),
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
        }
    }

    fn evict_one_unpinned(&self, inner: &mut CacheInner) -> bool {
        let max_scan = inner.lru.len();
        let mut scanned = 0usize;
        while scanned < max_scan {
            let Some((candidate_key, recorded_tick)) = inner.lru.pop_front() else {
                break;
            };
            scanned += 1;

            let Some(entry) = inner.entries.get(&candidate_key) else {
                continue;
            };
            // Skip stale LRU records superseded by a more recent touch.
            if entry.last_touch != recorded_tick {
                continue;
            }
            if Arc::strong_count(&entry.bytes) == 1 {
                inner.entries.remove(&candidate_key);
                self.evictions.fetch_add(1, Ordering::Relaxed);
                return true;
            }
            // Still pinned by an active reader/writer: push to back and continue scanning.
            inner.lru.push_back((candidate_key, recorded_tick));
        }
        false
    }
}
