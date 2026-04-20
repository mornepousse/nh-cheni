//! Shared HTTP helpers — timeouts, body caps, Retry-After parsing.
//!
//! Used by every code path that shells out to HTTP: the Repology
//! client (`api::repology`), the flake-input probe against
//! GitHub/GitLab (`nix::flake`), and the release-tarball fetcher
//! for `cheni verify` / `cheni self-update` (`release`).
//!
//! Living at the crate root (rather than under `api/`) keeps the
//! layering clean: `nix/` and `release` both need these helpers
//! and shouldn't cross-import `api/` to get them.
//!
//! ```text
//! CHENI_HTTP_TIMEOUT=60 cheni check     # wait up to 60s per request
//! ```
//!
//! Valid values for `CHENI_HTTP_TIMEOUT`: a positive integer number
//! of seconds. Bogus values fall back to the default with a debug log.

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

/// Default wait after a 429 response when the server didn't set
/// `Retry-After`. Three seconds matches Repology's recommended back-off
/// and is short enough for a CLI command to retry transparently.
pub const RATE_LIMIT_RETRY_SECS: u64 = 3;

/// Upper bound on `Retry-After` values we'll honor. Beyond this we
/// fall back to `RATE_LIMIT_RETRY_SECS`: we'd rather give up and
/// return an "unknown" than block the user for half a minute+.
pub const RATE_LIMIT_MAX_WAIT_SECS: u64 = 30;

/// Parse the `Retry-After` header into a seconds value.
///
/// Honors the delta-seconds format (RFC 7231 §7.1.3). The HTTP-date
/// variant is uncommon on APIs we consume (Repology, GitHub, GitLab)
/// and we don't attempt to parse it — we fall back to the default
/// instead.
///
/// Returns:
/// - the header value when it parses as a u64 in `[1, RATE_LIMIT_MAX_WAIT_SECS]`
/// - `RATE_LIMIT_RETRY_SECS` otherwise (missing header, unparseable,
///   zero, or exceeding the cap)
pub fn parse_retry_after(header_value: Option<&str>) -> u64 {
    match header_value.and_then(|s| s.trim().parse::<u64>().ok()) {
        Some(secs) if (1..=RATE_LIMIT_MAX_WAIT_SECS).contains(&secs) => secs,
        _ => RATE_LIMIT_RETRY_SECS,
    }
}

#[cfg(test)]
#[path = "tests/http.rs"]
mod tests;
