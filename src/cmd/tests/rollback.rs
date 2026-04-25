use super::*;

/// Build a minimal Generation for tests. `is_current` defaults off so
/// the caller only has to mark the active one.
fn gen(number: u32, is_current: bool) -> Generation {
    Generation {
        number,
        date: format!("2026-04-{:02}", number.min(28)),
        mtime_secs: None,
        is_current,
        store_path: format!("/nix/store/xxx-nixos-system-test-{}", number),
        nixos_label: Some(format!("test-label-{}", number)),
    }
}

#[test]
fn explicit_target_resolves_when_it_exists() {
    let gens = vec![gen(10, false), gen(11, false), gen(12, true)];
    let current = &gens[2];
    let target = resolve_target(&gens, current, Some(10)).unwrap();
    assert_eq!(target.number, 10);
}

#[test]
fn explicit_target_errors_when_absent_from_the_listing() {
    // Target doesn't exist on disk — we'd rather bail upfront than
    // hand nix-env a bogus number and parse its cryptic error.
    let gens = vec![gen(10, false), gen(11, true)];
    let current = &gens[1];
    let err = resolve_target(&gens, current, Some(9999)).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("9999"));
    assert!(msg.contains("not found"));
}

#[test]
fn explicit_target_equal_to_current_is_rejected() {
    // Rolling back to the current generation is a no-op dressed up as
    // a destructive-looking prompt — catch it early.
    let gens = vec![gen(10, false), gen(11, true)];
    let current = &gens[1];
    let err = resolve_target(&gens, current, Some(11)).unwrap_err();
    assert!(err.to_string().contains("already active"));
}

#[test]
fn no_target_picks_the_previous_generation() {
    let gens = vec![gen(10, false), gen(11, false), gen(12, true)];
    let current = &gens[2];
    let target = resolve_target(&gens, current, None).unwrap();
    assert_eq!(target.number, 11);
}

#[test]
fn no_target_skips_gaps_left_by_prior_prunes() {
    // `cheni history --prune` can remove 11, leaving 10 and 12 with a
    // gap. `rollback` without a target should land on 10 (highest
    // strictly below current), not fall through.
    let gens = vec![gen(10, false), gen(12, true)];
    let current = &gens[1];
    let target = resolve_target(&gens, current, None).unwrap();
    assert_eq!(target.number, 10);
}

#[test]
fn no_target_errors_when_current_is_the_oldest() {
    // Fresh NixOS install with one generation, or an aggressively
    // pruned history. Either way, "previous" doesn't exist.
    let gens = vec![gen(1, true)];
    let current = &gens[0];
    let err = resolve_target(&gens, current, None).unwrap_err();
    assert!(err.to_string().contains("oldest"));
}
