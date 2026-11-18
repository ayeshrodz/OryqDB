//! Cooperative cross-thread cancellation signals.
//!
//! A cancellation signal never revokes or releases a live snapshot pin.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::error::{ErrorCategory, ErrorEnvelope, Result};

/// Owner handle capable of triggering cooperative cancellation.
#[derive(Debug, Default)]
pub struct CancellationSource {
    cancelled: Arc<AtomicBool>,
}

impl CancellationSource {
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a shareable read-only [`CancellationToken`] linked to this source.
    pub fn token(&self) -> CancellationToken {
        CancellationToken {
            cancelled: Arc::clone(&self.cancelled),
        }
    }

    /// Signals cancellation to all observers holding tokens from this source.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// Cloneable read-only handle for observing cancellation requests.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    /// Returns a token that is never cancelled.
    pub fn never() -> Self {
        Self::default()
    }

    #[inline]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Returns `Err(ErrorCategory::Cancelled)` if cancellation has been requested.
    pub fn check(&self, module_id: &'static str, operation: &'static str) -> Result<()> {
        if self.is_cancelled() {
            return Err(ErrorEnvelope::new(
                ErrorCategory::Cancelled,
                module_id,
                operation,
                "operation cancelled by caller",
            ));
        }
        Ok(())
    }
}
