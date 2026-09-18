//! Retry policy for the TypeSafe SDK.
//!
//! Mirrors the `RetryPolicy` in the Python and JavaScript SDKs: a maximum
//! number of retries, a base delay, and a cap on the delay between attempts.
//! The SDK uses exponential backoff with jitter.

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
        let raw = self.base_delay.as_millis() as u64 * (1u64 << exp);
        let capped = raw.min(self.max_delay.as_millis() as u64);
        Duration::from_millis(capped)
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
}
