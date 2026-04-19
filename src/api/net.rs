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

use anyhow::{bail, Result};
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
    resolve_timeout(std::env::var("CHENI_HTTP_TIMEOUT").ok().as_deref())
}

/// Pure core of `http_timeout` — split out so tests don't race on the
/// shared CHENI_HTTP_TIMEOUT env var (cargo test runs in parallel and
/// `set_var` leaks across threads). Takes what the env *would* say;
/// returns the resolved duration.
pub(crate) fn resolve_timeout(env_value: Option<&str>) -> Duration {
    let Some(s) = env_value else {
        return Duration::from_secs(DEFAULT_TIMEOUT_SECS);
    };
    match s.trim().parse::<u64>() {
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
    }
}

/// Maximum body size we'll accept from any HTTP response, in bytes.
///
/// Real Repology project pages are a few kilobytes; GitHub/GitLab
/// commit responses are under a megabyte even for very large repos.
/// 5 MiB is a generous ceiling that keeps a compromised or buggy
/// upstream from turning cheni into a memory-DoS target.
pub const MAX_BODY_BYTES: usize = 5 * 1024 * 1024;

/// Reject responses whose advertised `Content-Length` already exceeds
/// `max_bytes`. Call this *before* reading the body so a lying-large
/// response is refused on the spot without pulling bytes into memory.
///
/// A missing `Content-Length` header means we cannot pre-check — the
/// server is chunked-transfer or headerless — so we let it through
/// and rely on `verify_body_size` after the fact.
pub fn check_content_length(content_length: Option<u64>, max_bytes: usize) -> Result<()> {
    if let Some(len) = content_length {
        if len as usize > max_bytes {
            bail!(
                "response Content-Length ({} bytes) exceeds {} byte limit",
                len,
                max_bytes
            );
        }
    }
    Ok(())
}

/// Reject a body that turned out larger than `max_bytes` after reading.
/// Complements `check_content_length` for servers that lied about or
/// omitted the header.
pub fn verify_body_size(actual: usize, max_bytes: usize) -> Result<()> {
    if actual > max_bytes {
        bail!(
            "response body ({} bytes) exceeds {} byte limit",
            actual,
            max_bytes
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/net.rs"]
mod tests;
