use super::*;

// Tests call the pure `resolve_timeout(env_value)` helper rather than
// `http_timeout()` directly — that way we don't mutate the shared
// CHENI_HTTP_TIMEOUT env var and parallel test runs (cargo test
// default + the Nix build) stop racing.

#[test]
fn default_when_unset() {
    assert_eq!(resolve_timeout(None), Duration::from_secs(DEFAULT_TIMEOUT_SECS));
}

#[test]
fn respects_override() {
    assert_eq!(resolve_timeout(Some("45")), Duration::from_secs(45));
}

#[test]
fn respects_override_with_whitespace() {
    // Shell users sometimes wrap values in quotes that survive into the
    // env; trim handles that case without complaint.
    assert_eq!(resolve_timeout(Some("  60  ")), Duration::from_secs(60));
}

#[test]
fn accepts_exact_minimum() {
    // Boundary: MIN_TIMEOUT_SECS itself must be accepted, not rejected.
    assert_eq!(
        resolve_timeout(Some("5")),
        Duration::from_secs(5)
    );
}

#[test]
fn rejects_too_small() {
    // Below MIN_TIMEOUT_SECS (5) the user almost certainly made a typo —
    // fall back to the default with a debug log.
    assert_eq!(
        resolve_timeout(Some("1")),
        Duration::from_secs(DEFAULT_TIMEOUT_SECS)
    );
}

#[test]
fn rejects_garbage() {
    assert_eq!(
        resolve_timeout(Some("banana")),
        Duration::from_secs(DEFAULT_TIMEOUT_SECS)
    );
}

#[test]
fn rejects_empty_string() {
    // `CHENI_HTTP_TIMEOUT=` unset-ish case — not a valid number, falls
    // through to the default.
    assert_eq!(
        resolve_timeout(Some("")),
        Duration::from_secs(DEFAULT_TIMEOUT_SECS)
    );
}
