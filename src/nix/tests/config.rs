use super::*;

#[test]
fn nix_keywords_detected() {
    assert!(is_nix_keyword("enable"));
    assert!(is_nix_keyword("pkgs"));
    assert!(is_nix_keyword("mkDerivation"));
}

#[test]
fn package_names_not_keywords() {
    assert!(!is_nix_keyword("firefox"));
    assert!(!is_nix_keyword("legcord"));
    assert!(!is_nix_keyword("kicad"));
}
