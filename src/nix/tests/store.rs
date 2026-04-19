use super::*;

#[test]
fn extract_name_from_store_path() {
    let path = "/nix/store/abc12345678901234567890123456789-legcord-1.5.4";
    assert_eq!(extract_store_name(path), Some("legcord-1.5.4"));
}

#[test]
fn split_simple() {
    assert_eq!(
        split_name_version("legcord-1.5.4"),
        Some(("legcord".into(), "1.5.4".into()))
    );
}

#[test]
fn split_with_plus() {
    assert_eq!(
        split_name_version("gtk+3-3.24.51"),
        Some(("gtk+3".into(), "3.24.51".into()))
    );
}

#[test]
fn split_complex_name() {
    assert_eq!(
        split_name_version("xdg-desktop-portal-1.15.1"),
        Some(("xdg-desktop-portal".into(), "1.15.1".into()))
    );
}

#[test]
fn split_terminfo_ignored() {
    assert_eq!(split_name_version("alacritty-0.17.0-terminfo"), None);
}

#[test]
fn split_source_archives_ignored() {
    // Source-file derivations land in the store alongside the real
    // package — must be filtered or they get parsed as bogus
    // versions ("620.zip") and shadow the real one.
    assert_eq!(split_name_version("displaylink-620.zip"), None);
    assert_eq!(split_name_version("foo-1.0.tar.gz"), None);
    assert_eq!(split_name_version("bar-2.3.tar.xz"), None);
    assert_eq!(split_name_version("baz-9.tgz"), None);
}

#[test]
fn split_platform_ignored() {
    assert_eq!(
        split_name_version("cargo-1.94.1-x86_64-unknown-linux-gnu"),
        None
    );
}

#[test]
fn split_no_version() {
    assert_eq!(split_name_version("some-package-name"), None);
}

#[test]
fn ignore_internal_packages() {
    assert!(is_ignored("libfoo"));
    assert!(is_ignored("gcc-13.2.0"));
    assert!(is_ignored("python3.11-pip"));
    assert!(is_ignored("nixos-rebuild"));
}

#[test]
fn keep_user_packages() {
    assert!(!is_ignored("firefox"));
    assert!(!is_ignored("legcord"));
    assert!(!is_ignored("kicad"));
    assert!(!is_ignored("alacritty"));
}

#[test]
fn pick_highest_version_empty_slice_returns_empty() {
    // Defensive guard — callers should never do this, but it mustn't panic.
    assert_eq!(pick_highest_version(&[]), "");
}

#[test]
fn pick_highest_version_picks_max() {
    // The mesa case that motivated the fix: a sub-output ("24.3.2-osmesa")
    // shouldn't shadow the real package version that's also in the store.
    assert_eq!(
        pick_highest_version(&["24.3.2-osmesa".into(), "26.0.4".into()]),
        "26.0.4"
    );
    assert_eq!(
        pick_highest_version(&["26.0.4".into(), "24.3.2-osmesa".into()]),
        "26.0.4"
    );
    assert_eq!(
        pick_highest_version(&["1.2.3".into(), "1.2.3".into()]),
        "1.2.3"
    );
    assert_eq!(
        pick_highest_version(&["3.12.8".into(), "3.13.12".into()]),
        "3.13.12"
    );
}
