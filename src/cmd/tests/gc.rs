//! Tests for `cmd::gc`.

use super::*;

#[test]
fn min_safety_floor_is_three() {
    assert_eq!(MIN_SAFETY_FLOOR, 3);
}

#[test]
fn default_keep_is_ten() {
    assert_eq!(DEFAULT_KEEP, 10);
}

#[test]
fn default_options_match_documented_defaults() {
    let opts = GcOptions::default();
    assert_eq!(opts.keep, DEFAULT_KEEP);
    assert!(!opts.dry_run);
    assert!(!opts.yes);
    assert!(!opts.brief);
    assert!(!opts.force);
}
