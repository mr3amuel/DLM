//! One download-level retry budget. Range workers must return failures, not retry them.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};

/// Maximum supported server cooldown, shared with persisted deadline validation.
pub const MAX_RETRY_AFTER_SECS: u64 = 365 * 24 * 60 * 60;

/// Attempts include the initial invocation. Explicit user Resume starts a new budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub jitter_percent: u32,
}
impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            initial_delay_ms: 500,
            max_delay_ms: 30_000,
            jitter_percent: 20,
        }
    }
}
impl RetryPolicy {
    pub fn validate(&self) -> Result<()> {
        if !(1..=100).contains(&self.max_attempts)
            || self.initial_delay_ms == 0
            || self.max_delay_ms < self.initial_delay_ms
            || self.max_delay_ms > 24 * 60 * 60 * 1000
            || self.jitter_percent > 100
        {
            return Err(Error::Invalid("Retry attempts must be 1–100, delays 1 ms–24 hours with initial <= maximum, jitter 0–100%".into()));
        }
        Ok(())
    }

    /// Returns an authoritative epoch-millisecond deadline, or no automatic retry.
    /// `attempt` is the failed invocation, starting at one. `jitter_sample` is supplied
    /// by the controller so policy tests need neither randomness nor wall-clock sleeps.
    /// Server cooldowns are lower bounds and are never shortened by jitter or caps.
    pub fn deadline_ms(
        &self,
        error: &Error,
        attempt: u32,
        now_ms: u64,
        jitter_sample: u64,
    ) -> Option<u64> {
        if self.validate().is_err()
            || attempt == 0
            || attempt >= self.max_attempts
            || !is_retryable(error)
        {
            return None;
        }
        let exponential = self
            .initial_delay_ms
            .saturating_mul(1_u64.checked_shl(attempt - 1).unwrap_or(u64::MAX))
            .min(self.max_delay_ms);
        let spread = exponential.saturating_mul(self.jitter_percent as u64) / 100;
        let delay = exponential
            .saturating_sub(spread)
            .saturating_add(jitter_sample % (spread.saturating_mul(2).saturating_add(1)))
            .min(self.max_delay_ms);
        let server_delay = match error {
            Error::Http {
                retry_after_secs, ..
            } => {
                if *retry_after_secs > MAX_RETRY_AFTER_SECS {
                    return None;
                }
                retry_after_secs.saturating_mul(1000)
            }
            _ => 0,
        };
        now_ms.checked_add(delay.max(server_delay))
    }
}

/// Local, integrity, cancellation and authentication failures require user action.
pub fn is_retryable(error: &Error) -> bool {
    match error {
        Error::Network(error) => {
            !error.is_builder()
                && (error.is_timeout()
                    || error.is_connect()
                    || error.is_body()
                    // Reqwest wraps truncated/chunked transport bodies as Decode.
                    // Core streams identity bytes only: no JSON or decompression
                    // decoder can produce an application-level decode failure.
                    || error.is_decode()
                    || (error.is_request() && !error.is_decode()))
        }
        Error::Http {
            status: 429 | 500 | 502 | 503 | 504,
            ..
        } => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn unavailable(seconds: u64) -> Error {
        Error::Http {
            status: 503,
            retry_after_secs: seconds,
        }
    }
    #[test]
    fn exponential_jitter_is_bounded_and_exhausts() {
        let policy = RetryPolicy::default();
        for (attempt, base) in [(1, 500), (2, 1000), (3, 2000)] {
            for sample in [0, 1, 999, u64::MAX] {
                let delay = policy
                    .deadline_ms(&unavailable(0), attempt, 100, sample)
                    .unwrap()
                    - 100;
                assert!((base * 80 / 100..=base * 120 / 100).contains(&delay));
            }
        }
        assert_eq!(policy.deadline_ms(&unavailable(0), 4, 100, 0), None);
        assert_eq!(policy.deadline_ms(&unavailable(0), 0, 100, 0), None);
        let capped = RetryPolicy {
            max_attempts: 100,
            ..policy
        };
        assert!(
            capped
                .deadline_ms(&unavailable(0), 99, 0, u64::MAX)
                .unwrap()
                <= 30_000
        );
    }
    #[test]
    fn retry_after_is_a_minimum_and_overflow_fails_closed() {
        let policy = RetryPolicy::default();
        assert_eq!(
            policy.deadline_ms(&unavailable(60), 1, 100, 0),
            Some(60_100)
        );
        assert_eq!(
            policy.deadline_ms(&unavailable(MAX_RETRY_AFTER_SECS + 1), 1, 0, 0),
            None
        );
        assert_eq!(policy.deadline_ms(&unavailable(0), 1, u64::MAX, 0), None);
    }
    #[test]
    fn permanent_and_cancelled_failures_never_retry() {
        for error in [
            Error::Stopped,
            Error::Protocol("identity changed".into()),
            Error::Disk(std::io::Error::other("full")),
            Error::Storage("database".into()),
            Error::Http {
                status: 403,
                retry_after_secs: 60,
            },
            Error::Http {
                status: 404,
                retry_after_secs: 0,
            },
        ] {
            assert!(!is_retryable(&error));
            assert_eq!(RetryPolicy::default().deadline_ms(&error, 1, 0, 0), None);
        }
        for status in [429, 500, 502, 503, 504] {
            assert!(is_retryable(&Error::Http {
                status,
                retry_after_secs: 0
            }));
        }
    }
    #[test]
    fn recovery_defaults_and_validation() {
        let policy: RetryPolicy = serde_json::from_str("{}").unwrap();
        assert_eq!(policy, RetryPolicy::default());
        assert!(policy.validate().is_ok());
        assert!(
            RetryPolicy {
                max_attempts: 0,
                ..policy.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            RetryPolicy {
                jitter_percent: 101,
                ..policy.clone()
            }
            .validate()
            .is_err()
        );
        assert_eq!(
            policy.deadline_ms(&unavailable(10), 1, 5_000, 0),
            Some(15_000)
        );
    }
}
