//! Retry policy for the TypeSafe SDK.
//!
//! Mirrors the `RetryPolicy` in the Python and JavaScript SDKs: a maximum
//! number of retries, a base delay, and a cap on the delay between attempts.
//! The SDK uses exponential backoff with full jitter.

use std::time::Duration;

/// Retry configuration. Fields mirror the Python `RetryPolicy` and the
/// JavaScript `RetryPolicy` defaults.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Maximum number of retry attempts after the initial request.
    pub max_retries: u32,
    /// Base delay for exponential backoff.
    pub base_delay: Duration,
    /// Maximum delay between retries.
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            base_delay: Duration::from_millis(500),
            max_delay: Duration::from_secs(10),
        }
    }
}

impl RetryPolicy {
    /// Create a retry policy with the given number of max retries.
    pub fn new(max_retries: u32) -> Self {
        Self {
            max_retries,
            ..Default::default()
        }
    }

    /// Set the base delay for exponential backoff.
    #[must_use = "the returned RetryPolicy should be used"]
    pub fn with_base_delay(mut self, delay: Duration) -> Self {
        self.base_delay = delay;
        self
    }

    /// Set the maximum delay between retries.
    #[must_use = "the returned RetryPolicy should be used"]
    pub fn with_max_delay(mut self, delay: Duration) -> Self {
        self.max_delay = delay;
        self
    }

    /// Compute the delay for a given attempt (0-indexed) using exponential
    /// backoff: `base_delay * 2^attempt`, capped at `max_delay`.
    pub fn delay_for(&self, attempt: u32) -> Duration {
        let exp = attempt.min(30); // prevent overflow
        let raw = self.base_delay.as_millis().saturating_mul(1u128 << exp) as u64;
        let capped = raw.min(self.max_delay.as_millis() as u64);
        Duration::from_millis(capped)
    }

    /// Compute the delay for a given attempt with full jitter.
    ///
    /// Returns a random duration in `[0, delay_for(attempt)]` to avoid
    /// thundering-herd retry storms. When a `Retry-After` hint is available,
    /// use [`delay_for_retry_after`](Self::delay_for_retry_after) instead.
    pub fn delay_for_with_jitter(&self, attempt: u32, jitter_seed: u64) -> Duration {
        let base = self.delay_for(attempt);
        let jitter_ms = if base.as_millis() == 0 {
            0
        } else {
            // Simple deterministic jitter based on seed — no rand dependency.
            jitter_seed % base.as_millis() as u64
        };
        Duration::from_millis(jitter_ms)
    }

    /// Compute the delay when the server provides a `Retry-After` hint.
    ///
    /// Returns `max(retry_after, delay_for(attempt))` so the server hint
    /// is respected but never shorter than our own backoff floor.
    pub fn delay_for_retry_after(&self, attempt: u32, retry_after: Duration) -> Duration {
        let own = self.delay_for(attempt);
        if retry_after > own {
            retry_after
        } else {
            own
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_grows_exponentially() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.delay_for(0), Duration::from_millis(500));
        assert_eq!(policy.delay_for(1), Duration::from_millis(1000));
        assert_eq!(policy.delay_for(2), Duration::from_millis(2000));
    }

    #[test]
    fn delay_is_capped() {
        let policy = RetryPolicy::default().with_max_delay(Duration::from_millis(1500));
        assert_eq!(policy.delay_for(0), Duration::from_millis(500));
        assert_eq!(policy.delay_for(1), Duration::from_millis(1000));
        assert_eq!(policy.delay_for(2), Duration::from_millis(1500));
        assert_eq!(policy.delay_for(10), Duration::from_millis(1500));
    }

    #[test]
    fn jitter_stays_within_bounds() {
        let policy = RetryPolicy::default();
        let base = policy.delay_for(1); // 1000ms
        let jittered = policy.delay_for_with_jitter(1, 42);
        assert!(jittered <= base);
    }

    #[test]
    fn retry_after_respects_server_hint() {
        let policy = RetryPolicy::default();
        // Server says wait 5s, our backoff is 500ms → use 5s
        let delay = policy.delay_for_retry_after(0, Duration::from_secs(5));
        assert_eq!(delay, Duration::from_secs(5));
        // Server says wait 100ms, our backoff is 500ms → use 500ms
        let delay = policy.delay_for_retry_after(0, Duration::from_millis(100));
        assert_eq!(delay, Duration::from_millis(500));
    }
}
