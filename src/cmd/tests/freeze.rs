use super::*;

// Note: `short_rev`, `civil_from_days` and `epoch_to_iso_date` were
// folded into shared helpers (`crate::nix::flake::short_hash` and
// `crate::util::format_ymd`). Their behaviour is now tested in
// `src/nix/tests/flake.rs` and `src/tests/util.rs` respectively.

#[test]
fn short_nar_hash_returns_input_when_short() {
    // Shorter than the head+tail width → return as-is, no ellipsis.
    assert_eq!(short_nar_hash("sha256-abc"), "sha256-abc");
}

#[test]
fn short_nar_hash_elides_middle_when_long() {
    let full = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    let short = short_nar_hash(full);
    assert!(short.starts_with("sha256-"), "got: {}", short);
    assert!(short.contains('…'), "got: {}", short);
    assert!(short.ends_with("AAAAA="), "got: {}", short);
    assert!(short.len() < full.len(), "got: {}", short);
}

#[test]
fn today_iso_is_well_formed() {
    // Smoke test: today_iso() must produce a YYYY-MM-DD string.
    let t = today_iso();
    assert_eq!(t.len(), 10);
    assert_eq!(&t[4..5], "-");
    assert_eq!(&t[7..8], "-");
    let year: u32 = t[..4].parse().expect("year is numeric");
    assert!((2020..=2200).contains(&year), "got {}", year);
}

#[test]
fn refresh_noop_when_no_constrained_freezes() {
    // Freezes without a constraint are strict locks — refresh must
    // leave them alone and report no outcomes.
    let dir = tempfile::tempdir().unwrap();
    // No file at all, yields an empty freezes map.
    let out = refresh_constrained_freezes(dir.path()).unwrap();
    assert!(out.is_empty());

    // A single strict-lock freeze should still yield no outcomes
    // (the refresh pass only touches entries with `major_constraint`).
    let mut freezes = crate::nix::freezes::Freezes::new();
    freezes.insert(
        "firefox".to_string(),
        crate::nix::freezes::FreezeEntry {
            rev: "abcdef0123456789abcdef0123456789abcdef01".to_string(),
            nar_hash: "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string(),
            version: "149.0.2".to_string(),
            frozen_at: "2026-04-20".to_string(),
            major_constraint: None,
        },
    );
    crate::nix::freezes::write(dir.path(), &freezes).unwrap();
    let out = refresh_constrained_freezes(dir.path()).unwrap();
    assert!(out.is_empty());
}

#[test]
fn refresh_outcome_held_renders_without_panic() {
    // Smoke test on the renderer — ensures the Held branch composes a
    // well-formed line (we only check it doesn't panic; the terminal
    // coloring is a display concern, not a behaviour concern).
    let outcomes = vec![(
        "kicad".to_string(),
        RefreshOutcome::Held {
            frozen_version: "9.0.2".to_string(),
            upstream_version: "10.0.0".to_string(),
            tracked_major: 9,
        },
    )];
    print_refresh_summary(&outcomes); // no panic = pass
}

#[test]
fn refresh_outcome_uptodate_block_is_silent() {
    // When every constrained freeze is already at the latest matching
    // version, there's nothing interesting to print — the renderer
    // should skip the header block entirely (no empty "Freeze refresh:"
    // line). Hard to assert visually; at minimum must not panic.
    let outcomes = vec![(
        "kicad".to_string(),
        RefreshOutcome::UpToDate { version: "9.0.2".to_string() },
    )];
    print_refresh_summary(&outcomes);
}
