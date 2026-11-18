//! Shared memory/spill budget accounting with non-duplicable RAII reservation tokens.

use core::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::error::{ErrorCategory, ErrorEnvelope, Result};

const MODULE_ID: &str = "types";

/// Explicit resource domains tracked separately across OryqDB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BudgetDomain {
    Cache,
    Query,
    Transaction,
    Spill,
}

impl fmt::Display for BudgetDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Cache => "cache",
            Self::Query => "query",
            Self::Transaction => "transaction",
            Self::Spill => "spill",
        };
        f.write_str(label)
    }
}

#[derive(Debug)]
struct BudgetInner {
    domain: BudgetDomain,
    limit_bytes: usize,
    used_bytes: AtomicUsize,
}

/// Shareable handle to a bounded byte budget for one resource domain.
#[derive(Debug, Clone)]
pub struct BudgetHandle {
    inner: Arc<BudgetInner>,
}

impl BudgetHandle {
    /// Creates a new bounded budget for `domain`.
    pub fn new(domain: BudgetDomain, limit_bytes: usize) -> Self {
        Self {
            inner: Arc::new(BudgetInner {
                domain,
                limit_bytes,
                used_bytes: AtomicUsize::new(0),
            }),
        }
    }

    #[inline]
    pub fn domain(&self) -> BudgetDomain {
        self.inner.domain
    }

    #[inline]
    pub fn limit_bytes(&self) -> usize {
        self.inner.limit_bytes
    }

    #[inline]
    pub fn used_bytes(&self) -> usize {
        self.inner.used_bytes.load(Ordering::Acquire)
    }

    #[inline]
    pub fn available_bytes(&self) -> usize {
        self.inner.limit_bytes.saturating_sub(self.used_bytes())
    }

    /// Atomically reserves `bytes`, returning an RAII [`Reservation`] token that releases
    /// its charge exactly once when dropped.
    pub fn reserve(&self, bytes: usize) -> Result<Reservation> {
        self.try_charge(bytes, "BudgetHandle::reserve")?;
        Ok(Reservation {
            inner: Arc::clone(&self.inner),
            bytes,
        })
    }

    fn try_charge(&self, additional: usize, operation: &'static str) -> Result<()> {
        if additional == 0 {
            return Ok(());
        }
        let mut current = self.inner.used_bytes.load(Ordering::Acquire);
        loop {
            let next = match current.checked_add(additional) {
                Some(n) if n <= self.inner.limit_bytes => n,
                _ => {
                    return Err(ErrorEnvelope::new(
                        ErrorCategory::QuotaExceeded,
                        MODULE_ID,
                        operation,
                        format!(
                            "{} budget exceeded: requested {} bytes with {} / {} bytes used",
                            self.inner.domain, additional, current, self.inner.limit_bytes
                        ),
                    ));
                }
            };
            match self.inner.used_bytes.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(observed) => current = observed,
            }
        }
    }
}

/// Owned, non-duplicable reservation token. Releases its charged bytes exactly once on drop.
#[must_use = "dropping a Reservation immediately releases its reserved budget"]
#[derive(Debug)]
pub struct Reservation {
    inner: Arc<BudgetInner>,
    bytes: usize,
}

impl Reservation {
    #[inline]
    pub fn domain(&self) -> BudgetDomain {
        self.inner.domain
    }

    #[inline]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Atomically increases this reservation by `additional` bytes.
    pub fn try_grow(&mut self, additional: usize) -> Result<()> {
        let handle = BudgetHandle {
            inner: Arc::clone(&self.inner),
        };
        handle.try_charge(additional, "Reservation::try_grow")?;
        self.bytes += additional;
        Ok(())
    }

    /// Releases `release_bytes` back to the budget, clamping at the current reservation size.
    pub fn shrink(&mut self, release_bytes: usize) {
        let delta = release_bytes.min(self.bytes);
        if delta > 0 {
            self.bytes -= delta;
            self.inner.used_bytes.fetch_sub(delta, Ordering::Release);
        }
    }

    /// Splits `amount` bytes off this reservation into a new independent token without
    /// duplicating or re-charging the underlying budget.
    pub fn split(&mut self, amount: usize) -> Result<Self> {
        if amount > self.bytes {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "Reservation::split",
                format!(
                    "cannot split {} bytes from a {}-byte reservation",
                    amount, self.bytes
                ),
            ));
        }
        self.bytes -= amount;
        Ok(Self {
            inner: Arc::clone(&self.inner),
            bytes: amount,
        })
    }

    /// Absorbs another reservation belonging to the exact same underlying budget instance.
    pub fn merge(&mut self, mut other: Self) -> Result<()> {
        if !Arc::ptr_eq(&self.inner, &other.inner) {
            return Err(ErrorEnvelope::new(
                ErrorCategory::InvalidArgument,
                MODULE_ID,
                "Reservation::merge",
                "cannot merge reservations from different budget instances",
            ));
        }
        self.bytes += other.bytes;
        other.bytes = 0;
        Ok(())
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.bytes > 0 {
            let released = self.bytes;
            self.bytes = 0;
            self.inner.used_bytes.fetch_sub(released, Ordering::Release);
        }
    }
}
