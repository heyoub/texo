//! Retry schedule helpers for OpenAI-compatible HTTP calls.

use std::time::Duration;

/// Maximum retry attempts after the first try.
pub const MAX_RETRIES: u32 = 4;
/// Initial exponential retry backoff.
pub const RETRY_BACKOFF: Duration = Duration::from_millis(500);
/// Maximum backoff, including `Retry-After` values.
pub const MAX_BACKOFF: Duration = Duration::from_secs(30);

pub use super::schedule::retry_delay;

/// Parse a delta-seconds `Retry-After` header from HTTP response headers.
#[must_use]
pub fn parse_retry_after(headers: &[(String, String)]) -> Option<u64> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        .and_then(|(_, value)| value.trim().parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_delay_doubles_and_clamps() {
        assert_eq!(retry_delay(1, None), Duration::from_millis(500));
        assert_eq!(retry_delay(2, None), Duration::from_secs(1));
        assert_eq!(retry_delay(3, None), Duration::from_secs(2));
        assert_eq!(retry_delay(99, None), MAX_BACKOFF);
    }

    #[test]
    fn retry_after_overrides_and_clamps() {
        assert_eq!(retry_delay(1, Some(7)), Duration::from_secs(7));
        assert_eq!(retry_delay(1, Some(60)), MAX_BACKOFF);
    }

    #[test]
    fn parse_retry_after_accepts_delta_seconds() {
        let headers = vec![("Retry-After".to_string(), "2".to_string())];
        assert_eq!(parse_retry_after(&headers), Some(2));
    }

    #[test]
    fn parse_retry_after_rejects_dates_and_bad_values() {
        let date = vec![(
            "retry-after".to_string(),
            "Wed, 21 Oct 2015 07:28:00 GMT".to_string(),
        )];
        let bad = vec![("retry-after".to_string(), "soon".to_string())];
        assert_eq!(parse_retry_after(&date), None);
        assert_eq!(parse_retry_after(&bad), None);
    }
}
