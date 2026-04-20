use super::*;

#[test]
fn classifies_enable_true_as_enabled() {
    assert_eq!(classify_match("programs.firefox.enable = true;"), Some("enabled"));
    assert_eq!(classify_match("  services.openssh.enable = true;"), Some("enabled"));
    // Missing spaces around `=` shouldn't change the verdict.
    assert_eq!(classify_match("programs.firefox.enable=true;"), Some("enabled"));
}

#[test]
fn classifies_enable_false_as_disabled() {
    assert_eq!(classify_match("programs.firefox.enable = false;"), Some("disabled"));
    assert_eq!(classify_match("services.openssh.enable=false;"), Some("disabled"));
}

#[test]
fn classifies_system_packages_as_system() {
    assert_eq!(
        classify_match("environment.systemPackages = [ pkgs.firefox ];"),
        Some("system")
    );
    // Short form (the prefix is the same so we still match).
    assert_eq!(
        classify_match("  systemPackages = with pkgs; [ firefox ];"),
        Some("system")
    );
}

#[test]
fn classifies_home_packages_as_home() {
    assert_eq!(
        classify_match("home.packages = with pkgs; [ firefox ];"),
        Some("home")
    );
    assert_eq!(
        classify_match("  home.packages = [ pkgs.firefox ];"),
        Some("home")
    );
}

#[test]
fn returns_none_for_bare_references() {
    // A generic attribute access doesn't fit any of the four roles —
    // we'd rather show no tag than misleading one.
    assert_eq!(classify_match("let ff = pkgs.firefox; in"), None);
    assert_eq!(classify_match("  firefox.override { ... }"), None);
    assert_eq!(classify_match(""), None);
}

#[test]
fn home_wins_over_system_when_both_look_like_candidates() {
    // `home.packages` contains the substring `packages` but not
    // `systemPackages`, so the order of checks matters. Both shapes
    // should classify correctly without stepping on each other.
    assert_eq!(
        classify_match("home.packages = [ pkgs.foo ];"),
        Some("home")
    );
    assert_eq!(
        classify_match("environment.systemPackages = [ pkgs.foo ];"),
        Some("system")
    );
}
