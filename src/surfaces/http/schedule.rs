//! Retry timing policy.

use std::time::Duration;

use super::retry::{MAX_BACKOFF, RETRY_BACKOFF};

/// Compute the retry delay for `attempt`.
///
/// `attempt` is one-based for retries: the first retry uses `attempt == 1`.
/// Delta-seconds from `Retry-After` override the exponential schedule, and both
/// paths clamp to [`MAX_BACKOFF`].
#[must_use]
pub fn retry_delay(attempt: u32, retry_after_secs: Option<u64>) -> Duration {
    let base = if let Some(secs) = retry_after_secs {
        Duration::from_secs(secs)
    } else {
        let shift = attempt.saturating_sub(1).min(20);
        RETRY_BACKOFF.saturating_mul(1_u32.checked_shl(shift).unwrap_or(u32::MAX))
    };
    base.min(MAX_BACKOFF)
}
