use super::*;

#[test]
fn infrastructure_inputs_excluded() {
    // Core infrastructure (always excluded)
    assert!(INFRASTRUCTURE_INPUTS.contains(&"nixpkgs"));
    assert!(INFRASTRUCTURE_INPUTS.contains(&"nixpkgs-latest"));
    assert!(INFRASTRUCTURE_INPUTS.contains(&"home-manager"));
    assert!(INFRASTRUCTURE_INPUTS.contains(&"cheni"));

    // User-facing package flakes (never excluded)
    assert!(!INFRASTRUCTURE_INPUTS.contains(&"zen-browser"));
    assert!(!INFRASTRUCTURE_INPUTS.contains(&"claude-code"));

    // Optional toolchain flakes should NOT be excluded — not every user
    // has them and those who do want update visibility.
    assert!(!INFRASTRUCTURE_INPUTS.contains(&"rust-overlay"));
    assert!(!INFRASTRUCTURE_INPUTS.contains(&"nixpkgs-esp-dev"));
    assert!(!INFRASTRUCTURE_INPUTS.contains(&"fenix"));
}

#[test]
fn short_hash_handles_short_input() {
    // The API may return a hash shorter than 12 chars (rare, but a
    // byte-slice would panic). Char-based truncation returns as many
    // chars as exist without panicking.
    assert_eq!(short_hash("abc"), "abc");
    assert_eq!(short_hash(""), "");
}

#[test]
fn short_hash_truncates_to_twelve() {
    assert_eq!(
        short_hash("abcdef1234567890"),
        "abcdef123456"
    );
}

#[test]
fn short_hash_survives_non_ascii() {
    // Not expected in real Git output, but we parse external JSON
    // so we can't assume it. Must not panic at a multi-byte boundary.
    assert_eq!(short_hash("é🦀x"), "é🦀x");
}

#[test]
fn short_date_handles_short_input() {
    assert_eq!(short_date("2026"), "2026");
    assert_eq!(short_date(""), "");
}

#[test]
fn is_revision_outdated_detects_change() {
    // flake.lock stores 12-char prefixes; API returns longer SHAs.
    // The comparison should truncate the remote to the local length.
    assert!(is_revision_outdated("abcdef123456", "000000000000"));
    assert!(is_revision_outdated("abcdef123456789", "000000000000"));
}

#[test]
fn is_revision_outdated_false_when_prefix_matches() {
    // Remote rev is longer but starts with the local prefix → up to date.
    assert!(!is_revision_outdated("abcdef1234", "abcdef1234"));
    assert!(!is_revision_outdated("abcdef123456", "abcdef123456"));
}

#[test]
fn is_revision_outdated_empty_inputs() {
    // Defensive: empty vs empty is not outdated; empty vs non-empty
    // compares prefix-of-length-0 which is always equal, so not outdated.
    // This matches the behaviour of the API-failed-to-respond path.
    assert!(!is_revision_outdated("", ""));
    assert!(!is_revision_outdated("", "abcdef"));
}

#[test]
fn is_revision_outdated_survives_non_ascii() {
    // Char-based slicing: mustn't panic on a multi-byte codepoint.
    assert!(is_revision_outdated("é🦀x000000", "abc000000000"));
}
