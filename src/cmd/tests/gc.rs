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

#[test]
fn safety_guard_passes_above_floor() {
    let result = check_safety_guard(5, false);
    assert!(result.is_ok());
}

#[test]
fn safety_guard_passes_at_floor() {
    let result = check_safety_guard(MIN_SAFETY_FLOOR, false);
    assert!(result.is_ok());
}

#[test]
fn safety_guard_blocks_below_floor() {
    let result = check_safety_guard(2, false);
    let err = result.expect_err("should refuse below floor");
    let msg = format!("{err}");
    assert!(msg.contains("safety floor"));
    assert!(msg.contains("--force"));
}

#[test]
fn safety_guard_allows_below_floor_with_force() {
    let result = check_safety_guard(1, true);
    assert!(result.is_ok());
}

#[test]
fn safety_guard_blocks_zero_even_with_force() {
    let result = check_safety_guard(0, true);
    let err = result.expect_err("zero is always refused");
    let msg = format!("{err}");
    assert!(msg.contains("0 generations"));
}
