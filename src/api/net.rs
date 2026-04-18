//! Shared HTTP timeout logic for the Repology and flake-input clients.
//!
//! Rationale: default timeouts are generous (network hiccups and slow
//! mirrors happen on real connections) but overridable via an environment
//! variable for users on bad links or weak machines who prefer to wait
//! longer rather than see partial results.
//!
//! ```text
//! CHENI_HTTP_TIMEOUT=60 cheni check     # wait up to 60s per request
//! ```
//!
//! Valid values: a positive integer number of seconds. Bogus values
//! fall back to the default with a debug log.

use std::time::Duration;
use tracing::debug;

/// Default per-request timeout, in seconds.
///
/// Higher than the previous 10s because on a slow DSL or mobile
/// connection the first HTTP handshake alone can eat several seconds.
/// Still low enough to catch a truly stuck request in a reasonable time.
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Minimum timeout we'll accept from the environment variable.
/// Below this the user probably made a typo (`CHENI_HTTP_TIMEOUT=2`)
/// and would get cascading failures without understanding why.
const MIN_TIMEOUT_SECS: u64 = 5;

/// Resolve the per-request HTTP timeout, respecting the
/// `CHENI_HTTP_TIMEOUT` environment variable if set to a valid value.
pub fn http_timeout() -> Duration {
    match std::env::var("CHENI_HTTP_TIMEOUT") {
        Ok(s) => match s.trim().parse::<u64>() {
            Ok(n) if n >= MIN_TIMEOUT_SECS => Duration::from_secs(n),
            Ok(n) => {
                debug!(
                    "CHENI_HTTP_TIMEOUT={} too small (min={}), using default {}s",
                    n, MIN_TIMEOUT_SECS, DEFAULT_TIMEOUT_SECS
                );
                Duration::from_secs(DEFAULT_TIMEOUT_SECS)
            }
            Err(_) => {
                debug!(
                    "CHENI_HTTP_TIMEOUT={:?} not a number, using default {}s",
                    s, DEFAULT_TIMEOUT_SECS
                );
                Duration::from_secs(DEFAULT_TIMEOUT_SECS)
            }
        },
        Err(_) => Duration::from_secs(DEFAULT_TIMEOUT_SECS),
    }
}

#[cfg(test)]
#[path = "tests/net.rs"]
mod tests;
