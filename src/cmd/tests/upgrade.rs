use super::*;
use crate::nix::store::StorePackage;
use crate::version::compare::VersionDiff;

fn pkg(name: &str, version: &str) -> StorePackage {
    StorePackage {
        name: name.to_string(),
        version: version.to_string(),
    }
}

#[test]
fn marks_new_installs_when_the_package_is_absent_locally() {
    let installed: Vec<StorePackage> = vec![];
    let entries = vec!["chromium-151.0.0".to_string()];
    let changes = build_changes(&entries, &installed);

    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].name, "chromium");
    assert_eq!(changes[0].new, "151.0.0");
    assert!(changes[0].old.is_none());
}

#[test]
fn classifies_a_patch_bump() {
    let installed = vec![pkg("firefox", "149.0.1")];
    let entries = vec!["firefox-149.0.2".to_string()];
    let changes = build_changes(&entries, &installed);

    assert_eq!(changes[0].name, "firefox");
    assert_eq!(changes[0].old.as_deref(), Some("149.0.1"));
    assert_eq!(changes[0].new, "149.0.2");
    // compare_versions treats a single trailing bump with a matching
    // leading component as Minor; we display it as "patch" in the
    // tag mapping. The model stays honest with VersionDiff::Minor.
    assert_eq!(changes[0].diff, VersionDiff::Minor);
}

#[test]
fn classifies_a_major_bump() {
    let installed = vec![pkg("openssl", "3.0.7")];
    let entries = vec!["openssl-4.0.0".to_string()];
    let changes = build_changes(&entries, &installed);

    assert_eq!(changes[0].diff, VersionDiff::Major);
}

#[test]
fn classifies_a_downgrade_as_newer() {
    // `Newer` means "installed is newer than available" — a
    // downgrade from the user's perspective when it shows up in a
    // dry-run fetch list. The render layer paints this differently
    // so the user notices.
    let installed = vec![pkg("vivaldi", "7.9")];
    let entries = vec!["vivaldi-7.8".to_string()];
    let changes = build_changes(&entries, &installed);
    assert_eq!(changes[0].diff, VersionDiff::Newer);
}

#[test]
fn entries_with_unparseable_names_fall_back_cleanly() {
    // `split_name_version` returns None for things like
    // `some-package-name` (no trailing digits). We shouldn't drop
    // them — keep them in the list with an empty name so the user
    // still sees something.
    let installed: Vec<StorePackage> = vec![];
    let entries = vec!["some-package-name".to_string()];
    let changes = build_changes(&entries, &installed);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].name, "");
    assert_eq!(changes[0].new, "some-package-name");
    assert!(changes[0].old.is_none());
}

#[test]
fn aggregate_header_drops_zero_groups() {
    // Headers like "(3 major, 0 minor, 8 patch)" are noisy; we only
    // keep the non-zero slots.
    let installed = vec![
        pkg("major-pkg", "1.0"),
        pkg("patch-pkg", "1.2.3"),
    ];
    let entries = vec![
        "major-pkg-2.0".to_string(),
        "patch-pkg-1.2.4".to_string(),
        "new-pkg-9.9".to_string(),
    ];
    let changes = build_changes(&entries, &installed);
    let header = aggregate_header(&changes);
    assert!(header.contains("1 major"));
    // "new-pkg" is a new install — it belongs in the "new" bucket,
    // not any of the diff buckets.
    assert!(header.contains("1 new"));
    assert!(!header.contains("0 "));
}

#[test]
fn aggregate_header_is_empty_when_nothing_changes() {
    let empty: Vec<crate::nix::store::StorePackage> = vec![];
    assert_eq!(aggregate_header(&build_changes(&[], &empty)), "");
}

#[test]
fn parses_single_flake_input_update() {
    let stderr = "\
warning: Git tree '/home/user/nixos-config' is dirty
warning: updating lock file \"/home/user/nixos-config/flake.lock\":
• Updated input 'cheni':
    'gitlab:harrael/cheni/abc123?narHash=sha256-XXX=' (2026-04-19)
  → 'gitlab:harrael/cheni/def456?narHash=sha256-YYY=' (2026-04-20)
";
    let updates = parse_flake_update_events(stderr);
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].name, "cheni");
    assert_eq!(updates[0].old_date, "2026-04-19");
    assert_eq!(updates[0].new_date, "2026-04-20");
}

#[test]
fn parses_multiple_flake_input_updates() {
    let stderr = "\
• Updated input 'nixpkgs':
    'github:NixOS/nixpkgs/aaa?narHash=sha256-A=' (2026-04-10)
  → 'github:NixOS/nixpkgs/bbb?narHash=sha256-B=' (2026-04-20)
• Updated input 'home-manager':
    'github:nix-community/home-manager/ccc?narHash=sha256-C=' (2026-04-15)
  → 'github:nix-community/home-manager/ddd?narHash=sha256-D=' (2026-04-20)
";
    let updates = parse_flake_update_events(stderr);
    assert_eq!(updates.len(), 2);
    assert_eq!(updates[0].name, "nixpkgs");
    assert_eq!(updates[1].name, "home-manager");
}

#[test]
fn parses_no_updates_on_a_clean_run() {
    let stderr = "warning: Git tree is dirty\nno updates\n";
    assert!(parse_flake_update_events(stderr).is_empty());
}

#[test]
fn parses_skips_malformed_date_lines() {
    // If the new-line locator is garbage, the event is skipped rather
    // than producing a malformed `InputUpdate`.
    let stderr = "\
• Updated input 'weird':
    'url' (not-a-date)
  → 'url' (2026-04-20)
";
    assert!(parse_flake_update_events(stderr).is_empty());
}

#[test]
fn format_elapsed_under_a_minute() {
    assert_eq!(format_elapsed(std::time::Duration::from_secs(0)), "0s");
    assert_eq!(format_elapsed(std::time::Duration::from_secs(42)), "42s");
    assert_eq!(format_elapsed(std::time::Duration::from_secs(59)), "59s");
}

#[test]
fn format_elapsed_over_a_minute() {
    assert_eq!(format_elapsed(std::time::Duration::from_secs(60)), "1m00s");
    assert_eq!(format_elapsed(std::time::Duration::from_secs(125)), "2m05s");
    assert_eq!(format_elapsed(std::time::Duration::from_secs(3_600)), "60m00s");
}
