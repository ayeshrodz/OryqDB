//! Shared query execution context (memory budget, cancellation token, and explain counters).

use std::sync::atomic::{AtomicU64, Ordering};

use oryq_types::{BudgetDomain, BudgetHandle, CancellationSource, CancellationToken, Result};

/// Execution context shared across all stages of a single query or pipeline.
#[derive(Debug)]
pub struct QueryContext {
    budget: BudgetHandle,
    cancel: CancellationToken,
    keys_examined: AtomicU64,
    docs_examined: AtomicU64,
    docs_returned: AtomicU64,
}

impl QueryContext {
    pub fn new(budget: BudgetHandle, cancel: CancellationToken) -> Self {
        Self {
            budget,
            cancel,
            keys_examined: AtomicU64::new(0),
            docs_examined: AtomicU64::new(0),
            docs_returned: AtomicU64::new(0),
        }
    }

    pub fn default_with_limit(limit_bytes: usize) -> Self {
        let src = CancellationSource::new();
        Self::new(
            BudgetHandle::new(BudgetDomain::Query, limit_bytes),
            src.token(),
        )
    }

    #[inline]
    pub fn budget(&self) -> &BudgetHandle {
        &self.budget
    }

    #[inline]
    pub fn cancel_token(&self) -> &CancellationToken {
        &self.cancel
    }

    #[inline]
    pub fn check_cancelled(&self) -> Result<()> {
        self.cancel.check("query", "QueryContext::check_cancelled")
    }

    #[inline]
    pub fn add_keys_examined(&self, n: u64) {
        self.keys_examined.fetch_add(n, Ordering::Relaxed);
    }

    #[inline]
    pub fn add_docs_examined(&self, n: u64) {
        self.docs_examined.fetch_add(n, Ordering::Relaxed);
    }

    #[inline]
    pub fn add_docs_returned(&self, n: u64) {
        self.docs_returned.fetch_add(n, Ordering::Relaxed);
    }

    #[inline]
    pub fn keys_examined(&self) -> u64 {
        self.keys_examined.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn docs_examined(&self) -> u64 {
        self.docs_examined.load(Ordering::Relaxed)
    }

    #[inline]
    pub fn docs_returned(&self) -> u64 {
        self.docs_returned.load(Ordering::Relaxed)
    }
}
