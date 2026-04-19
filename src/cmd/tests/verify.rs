use super::*;

#[test]
fn explicit_tag_wins_over_installed() {
    let got = resolve_tag(Some("v9.9.9"), "v0.1.0-beta-5-gabcdef").unwrap();
    assert_eq!(got, "v9.9.9");
}

#[test]
fn falls_back_to_installed_describe_stripped() {
    assert_eq!(resolve_tag(None, "v0.1.0-beta").unwrap(), "v0.1.0-beta");
    assert_eq!(
        resolve_tag(None, "v0.1.0-beta-5-gabcdef0").unwrap(),
        "v0.1.0-beta"
    );
    assert_eq!(
        resolve_tag(None, "v0.1.0-beta-5-gabcdef0-dirty").unwrap(),
        "v0.1.0-beta"
    );
}

#[test]
fn errors_when_installed_is_unknown() {
    // Builds with no embedded git metadata can't point at any release.
    let err = resolve_tag(None, "unknown").unwrap_err().to_string();
    assert!(err.contains("GIT_DESCRIBE=unknown"));
    assert!(err.contains("--tag"));
}

#[test]
fn explicit_tag_wins_even_when_installed_is_unknown() {
    // User override sidesteps the unknown-build error.
    let got = resolve_tag(Some("v0.2.0"), "unknown").unwrap();
    assert_eq!(got, "v0.2.0");
}
