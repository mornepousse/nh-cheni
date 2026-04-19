use super::*;

#[test]
fn simple_version() {
    assert_eq!(parse_version("1.2.3"), vec![1, 2, 3]);
}

#[test]
fn two_part_version() {
    assert_eq!(parse_version("3.28"), vec![3, 28]);
}

#[test]
fn single_number() {
    assert_eq!(parse_version("42"), vec![42]);
}

#[test]
fn version_with_suffix() {
    assert_eq!(parse_version("1.94.1-x86_64-unknown-linux-gnu"), vec![1, 94, 1]);
}

#[test]
fn version_with_pre_release() {
    assert_eq!(parse_version("2.0.0-beta1"), vec![2, 0, 0]);
}

#[test]
fn version_with_unstable_suffix() {
    assert_eq!(parse_version("0.17.0-unstable"), vec![0, 17, 0]);
}

#[test]
fn empty_string() {
    assert_eq!(parse_version(""), Vec::<u64>::new());
}

#[test]
fn no_digits() {
    assert_eq!(parse_version("alpha"), Vec::<u64>::new());
}

#[test]
fn detects_pep440_alpha() {
    // The python case that motivated the helper: 3.15.0a7 must NOT
    // appear as a stable update for a user on 3.14.3.
    assert!(is_prerelease("3.15.0a7"));
    assert!(is_prerelease("2.0b1"));
    assert!(is_prerelease("1.0rc3"));
}

#[test]
fn detects_dash_suffixes() {
    assert!(is_prerelease("2.0.0-beta1"));
    assert!(is_prerelease("1.0-rc2"));
    assert!(is_prerelease("0.17.0-unstable"));
    assert!(is_prerelease("4.5-pre"));
    assert!(is_prerelease("0.1-dev"));
}

#[test]
fn stable_versions_not_flagged() {
    assert!(!is_prerelease("3.14.3"));
    assert!(!is_prerelease("1.0.0"));
    // Calver dates must not trip the heuristic.
    assert!(!is_prerelease("2026.04.01"));
    assert!(!is_prerelease("20240301"));
    // Words containing 'a' or 'b' but not as a pre-release marker.
    assert!(!is_prerelease("1.0-build42"));
    assert!(!is_prerelease("alacritty-0.17.0"));
}
