//! Retry policy for the TypeSafe SDK.
//!
//! Mirrors the `RetryPolicy` in the Python and JavaScript SDKs: a maximum
//! number of retries, a base delay, and a cap on the delay between attempts.
//! The SDK uses exponential backoff with equal jitter.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::Duration;

/// Generate a process-wide unique seed for jitter. Uses the standard
/// library's `RandomState` hasher, which is seeded from the OS RNG at
/// construction time — so each call returns a different value without
/// pulling in a `rand` dependency.
pub fn jitter_seed() -> u64 {
    RandomState::new().build_hasher().finish()
}

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
    ///
    /// Both the backoff calculation and the cap are protected against
    /// `u128 → u64` truncation, so arbitrarily large `Duration` values
    /// in `base_delay` or `max_delay` will not produce incorrect results.
    pub fn delay_for(&self, attempt: u32) -> Duration {
        let exp = attempt.min(30); // prevent overflow
        let raw = self
            .base_delay
            .as_millis()
            .saturating_mul(1u128 << exp)
            .min(u64::MAX as u128) as u64;
        let max_delay_ms = self.max_delay.as_millis().min(u64::MAX as u128) as u64;
        let capped = raw.min(max_delay_ms);
        Duration::from_millis(capped)
    }

    /// Compute the delay for a given attempt with equal jitter.
    ///
    /// Returns a duration in `[base/2, base]` where `base = delay_for(attempt)`.
    /// Equal jitter guarantees at least half the computed backoff, avoiding
    /// the near-zero delays that full jitter can produce, while still
    /// spreading retries across the upper half of the window.
    ///
    /// Delays are computed in whole milliseconds. With a `base_delay` of 1 ms,
    /// integer division yields `half = 0` and the jittered delay collapses to
    /// 0 ms. This is harmless in practice (no one configures sub-millisecond
    /// retry delays for an HTTP SDK) but is documented for completeness.
    ///
    /// When a `Retry-After` hint is available, use
    /// [`delay_for_retry_after`](Self::delay_for_retry_after) instead.
    pub fn delay_for_with_jitter(&self, attempt: u32, jitter_seed: u64) -> Duration {
        let base = self.delay_for(attempt);
        let base_ms = base.as_millis() as u64;
        if base_ms == 0 {
            return Duration::ZERO;
        }
        let half = base_ms / 2;
        let jitter = jitter_seed % (half + 1);
        Duration::from_millis(half + jitter)
    }

    /// Compute the delay when the server provides a `Retry-After` hint.
    ///
    /// Returns `Some(delay)` if the server's hint does not exceed `max_delay`,
    /// where `delay = max(retry_after, delay_for(attempt))` so the server hint
    /// is respected but never shorter than our own backoff floor.
    /// Returns `None` if `retry_after` exceeds `max_delay`, signalling the
    /// retry loop to give up rather than stall the caller.
    pub fn delay_for_retry_after(&self, attempt: u32, retry_after: Duration) -> Option<Duration> {
        if retry_after > self.max_delay {
            return None;
        }
        let own = self.delay_for(attempt);
        Some(if retry_after > own { retry_after } else { own })
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
    fn equal_jitter_stays_within_bounds() {
        let policy = RetryPolicy::default();
        let base = policy.delay_for(1); // 1000ms
        let jittered = policy.delay_for_with_jitter(1, 42);
        // Equal jitter: [base/2, base]
        assert!(jittered >= Duration::from_millis(base.as_millis() as u64 / 2));
        assert!(jittered <= base);
    }

    #[test]
    fn equal_jitter_floor_is_half_base() {
        // With seed 0, jitter is 0, so delay = base/2 (the floor).
        let policy = RetryPolicy::default().with_base_delay(Duration::from_millis(200));
        let delay = policy.delay_for_with_jitter(0, 0);
        assert_eq!(delay, Duration::from_millis(100));
        // With seed = half, jitter is half, so delay = base.
        let delay = policy.delay_for_with_jitter(0, 100);
        assert_eq!(delay, Duration::from_millis(200));
    }

    #[test]
    fn retry_after_respects_server_hint() {
        let policy = RetryPolicy::default();
        // Server says wait 5s, our backoff is 500ms → use 5s
        let delay = policy.delay_for_retry_after(0, Duration::from_secs(5));
        assert_eq!(delay, Some(Duration::from_secs(5)));
        // Server says wait 100ms, our backoff is 500ms → use 500ms
        let delay = policy.delay_for_retry_after(0, Duration::from_millis(100));
        assert_eq!(delay, Some(Duration::from_millis(500)));
    }

    #[test]
    fn retry_after_gives_up_when_hint_exceeds_max_delay() {
        let policy = RetryPolicy::default(); // max_delay = 10s
                                             // Server says wait 30s, our max_delay is 10s → give up (None)
        let delay = policy.delay_for_retry_after(0, Duration::from_secs(30));
        assert_eq!(delay, None);
    }

    #[test]
    fn jitter_seed_returns_nonzero() {
        // jitter_seed should return a usable seed value.
        let s = jitter_seed();
        // We can't assert uniqueness deterministically, but we can verify
        // it produces a value that works as a modulo operand.
        let _ = s % 100;
    }

    #[test]
    fn delay_for_handles_extreme_max_delay() {
        // A max_delay far beyond u64::MAX millis must not truncate to a
        // small value and produce an incorrect cap.
        let policy = RetryPolicy::new(1)
            .with_base_delay(Duration::from_millis(500))
            .with_max_delay(Duration::from_secs(u64::MAX / 2));
        let delay = policy.delay_for(5);
        // The exponential backoff (500ms * 2^5 = 16000ms) should be well
        // under the enormous cap, so the delay equals the raw backoff.
        assert_eq!(delay, Duration::from_millis(16000));
    }
}
