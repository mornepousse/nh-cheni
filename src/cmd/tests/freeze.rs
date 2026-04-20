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
