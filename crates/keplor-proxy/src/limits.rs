//! Per-server concurrency limits.
//!
//! A [`ConcurrencyLimiter`] wraps a [`tokio::sync::Semaphore`] to cap the
//! number of in-flight requests.  When the limit is reached, new requests
//! are rejected with [`ProxyError::ConcurrencyLimit`].

use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::error::ProxyError;

/// Server-wide concurrency limiter backed by a [`Semaphore`].
#[derive(Debug, Clone)]
pub struct ConcurrencyLimiter {
    semaphore: Arc<Semaphore>,
    max: usize,
}

impl ConcurrencyLimiter {
    /// Create a new limiter that allows up to `max` concurrent requests.
    pub fn new(max: usize) -> Self {
        Self { semaphore: Arc::new(Semaphore::new(max)), max }
    }

    /// Acquire a permit.  Returns an [`OwnedSemaphorePermit`] that is held
    /// for the lifetime of the request and automatically released on drop.
    ///
    /// # Errors
    ///
    /// Returns [`ProxyError::ConcurrencyLimit`] if all permits are in use.
    pub fn try_acquire(&self) -> Result<OwnedSemaphorePermit, ProxyError> {
        Arc::clone(&self.semaphore)
            .try_acquire_owned()
            .map_err(|_| ProxyError::ConcurrencyLimit(self.max))
    }

    /// The configured maximum.
    pub fn max(&self) -> usize {
        self.max
    }

    /// How many permits are currently available.
    pub fn available(&self) -> usize {
        self.semaphore.available_permits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_up_to_max() {
        let lim = ConcurrencyLimiter::new(2);
        let _p1 = lim.try_acquire().unwrap();
        let _p2 = lim.try_acquire().unwrap();
        assert!(lim.try_acquire().is_err());
    }

    #[test]
    fn release_frees_permit() {
        let lim = ConcurrencyLimiter::new(1);
        let p = lim.try_acquire().unwrap();
        assert_eq!(lim.available(), 0);
        drop(p);
        assert_eq!(lim.available(), 1);
        let _p2 = lim.try_acquire().unwrap();
    }

    #[test]
    fn max_reports_configured_value() {
        let lim = ConcurrencyLimiter::new(42);
        assert_eq!(lim.max(), 42);
        assert_eq!(lim.available(), 42);
    }
}
