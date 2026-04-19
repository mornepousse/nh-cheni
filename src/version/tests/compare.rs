use super::*;

#[test]
fn equal_versions() {
    assert_eq!(compare_versions(&[1, 2, 3], &[1, 2, 3]), VersionDiff::Equal);
}

#[test]
fn equal_with_trailing_zeros() {
    assert_eq!(compare_versions(&[1, 2], &[1, 2, 0]), VersionDiff::Equal);
}

#[test]
fn minor_patch_update() {
    assert_eq!(compare_versions(&[1, 2, 0], &[1, 2, 1]), VersionDiff::Minor);
}

#[test]
fn minor_version_update() {
    assert_eq!(compare_versions(&[1, 2, 0], &[1, 3, 0]), VersionDiff::Minor);
}

#[test]
fn major_update() {
    assert_eq!(compare_versions(&[9, 0, 2], &[10, 0, 1]), VersionDiff::Major);
}

#[test]
fn major_update_single_digit() {
    assert_eq!(compare_versions(&[1], &[2]), VersionDiff::Major);
}

#[test]
fn newer_than_available() {
    assert_eq!(compare_versions(&[2, 0, 0], &[1, 9, 0]), VersionDiff::Newer);
}

#[test]
fn newer_minor() {
    assert_eq!(compare_versions(&[1, 5, 0], &[1, 4, 0]), VersionDiff::Newer);
}

#[test]
fn empty_versions() {
    assert_eq!(compare_versions(&[], &[]), VersionDiff::Equal);
}

#[test]
fn empty_vs_zero() {
    assert_eq!(compare_versions(&[], &[0]), VersionDiff::Equal);
}

// Calendar versioning (calver) — should NOT be major
#[test]
fn calver_noto_fonts() {
    // noto-fonts: 2026.03.01 → 2026.04.01 (same year, minor)
    assert_eq!(compare_versions(&[2026, 3, 1], &[2026, 4, 1]), VersionDiff::Minor);
}

#[test]
fn calver_cross_year() {
    // 2.004 → 2026.04.01 looks like major but it's calver
    assert_eq!(compare_versions(&[2, 4], &[2026, 4, 1]), VersionDiff::Minor);
}

#[test]
fn calver_mesa() {
    // mesa: 24.3.2 → 26.0.4 — major is < 2000, so it's a real major
    assert_eq!(compare_versions(&[24, 3, 2], &[26, 0, 4]), VersionDiff::Major);
}

#[test]
fn calver_detection() {
    assert!(is_calver(2026));
    assert!(is_calver(2000));
    assert!(!is_calver(1999));
    assert!(!is_calver(26));
    assert!(!is_calver(0));
}
