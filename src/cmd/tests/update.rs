use super::*;

#[test]
fn noop_warning_fires_when_nothing_moved_and_pins_exist() {
    let ctx = UpdateContext { nixpkgs_latest_moved: false };
    let pins = vec!["firefox".to_string(), "chromium".to_string()];
    let warning = preview_noop_warning(&ctx, &pins, false).expect("should warn");
    assert!(warning.contains("nixpkgs-latest did not move"), "warning: {warning}");
    assert!(warning.contains("2 pins are"), "warning: {warning}");
}

#[test]
fn noop_warning_singular_for_one_pin() {
    let ctx = UpdateContext { nixpkgs_latest_moved: false };
    let pins = vec!["firefox".to_string()];
    let warning = preview_noop_warning(&ctx, &pins, false).expect("should warn");
    assert!(warning.contains("1 pin is"), "warning: {warning}");
}

#[test]
fn noop_warning_silent_when_lock_is_dirty() {
    let ctx = UpdateContext { nixpkgs_latest_moved: false };
    let pins = vec!["firefox".to_string()];
    // Dirty lock = flake inputs pending from outside cheni. Real cause
    // to rebuild, no warning.
    assert!(preview_noop_warning(&ctx, &pins, true).is_none());
}

#[test]
fn noop_warning_silent_when_latest_moved() {
    let ctx = UpdateContext { nixpkgs_latest_moved: true };
    let pins = vec!["firefox".to_string()];
    assert!(preview_noop_warning(&ctx, &pins, false).is_none());
}

#[test]
fn noop_warning_silent_when_no_pins() {
    let ctx = UpdateContext { nixpkgs_latest_moved: false };
    // No pins → the command was called for its rebuild-the-flake-lock
    // side, which is always non-no-op by construction (otherwise we'd
    // have short-circuited earlier in run()).
    assert!(preview_noop_warning(&ctx, &[], true).is_none());
}

#[test]
fn format_elapsed_under_a_minute() {
    assert_eq!(format_elapsed(std::time::Duration::from_secs(0)), "0s");
    assert_eq!(format_elapsed(std::time::Duration::from_secs(42)), "42s");
}

#[test]
fn format_elapsed_over_a_minute() {
    assert_eq!(format_elapsed(std::time::Duration::from_secs(60)), "1m00s");
    assert_eq!(format_elapsed(std::time::Duration::from_secs(125)), "2m05s");
}
