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
    let refs: Vec<&_> = changes.iter().collect();
    let header = aggregate_header(&refs);
    assert!(header.contains("1 major"));
    // "new-pkg" is a new install — it belongs in the "new" bucket,
    // not any of the diff buckets.
    assert!(header.contains("1 new"));
    assert!(!header.contains("0 "));
}

#[test]
fn aggregate_header_is_empty_when_nothing_changes() {
    let empty: Vec<crate::nix::store::StorePackage> = vec![];
    let changes = build_changes(&[], &empty);
    let refs: Vec<&_> = changes.iter().collect();
    assert_eq!(aggregate_header(&refs), "");
}

#[test]
fn system_artefact_exacts_are_collapsed() {
    // These are bare store names emitted by `nix build --dry-run`
    // for home-manager / nixos system closures. They're noise in
    // the preview — not things the user installed.
    for name in [
        "options.json",
        "man-cache",
        "man-paths",
        "etc",
        "boot.json",
        "firmware",
    ] {
        assert!(is_system_artefact_name(name), "{name} should classify as artefact");
    }
}

#[test]
fn system_artefact_prefixes_match_home_manager_and_nixos() {
    for name in [
        "hm_.manpath",
        "home-manager-path",
        "home-configuration-reference-manpage",
        "nixos-system-my-host",
        "system-path",
        "closure-info",
        "initrd-linux-6.12.1",
        "linux-6.12.1-modules",
        "user-environment",
    ] {
        assert!(is_system_artefact_name(name), "{name} should classify as artefact");
    }
}

#[test]
fn system_artefact_suffixes_cover_completions_and_manpages() {
    for name in [
        "foo-fish-completions",
        "bar-bash-completions",
        "baz-zsh-completions",
        "quux-completions",
        "something.manpath",
        "something.dirs",
        "something-manpage",
    ] {
        assert!(is_system_artefact_name(name), "{name} should classify as artefact");
    }
}

#[test]
fn real_packages_are_not_classified_as_artefacts() {
    // The whole point: real packages with normal names stay out of
    // the artefact bucket.
    for name in ["firefox", "openssl", "vivaldi", "kicad", "claude-code"] {
        assert!(!is_system_artefact_name(name), "{name} should NOT be an artefact");
    }
}

#[test]
fn aggregate_stats_separates_artefacts_from_packages() {
    // Mix of one real package, two artefacts. Stats should have
    // 1 minor (firefox) + 2 artefacts, and 0 in every other bucket.
    let installed = vec![pkg("firefox", "149.0.1")];
    let fetch_entries = vec!["firefox-149.0.2".to_string()];
    let build_entries = vec!["options.json".to_string(), "hm_.manpath".to_string()];
    let fetch = build_changes(&fetch_entries, &installed);
    let build = build_changes(&build_entries, &installed);
    let stats = aggregate_stats(&fetch, &build);

    assert_eq!(stats.minor, 1);
    assert_eq!(stats.artefacts, 2);
    assert_eq!(stats.major, 0);
    assert_eq!(stats.patch, 0);
    assert_eq!(stats.new, 0);
    assert_eq!(stats.total_packages(), 1);
}

#[test]
fn aggregate_stats_counts_only_artefacts_when_no_real_packages() {
    // The "home-manager refresh, nothing else" case — what triggered
    // the fix. All 19 entries bucketed as artefacts, zero packages.
    let installed: Vec<StorePackage> = vec![];
    let build_entries = vec![
        "options.json".to_string(),
        "home-manager-path".to_string(),
        "user-environment".to_string(),
    ];
    let stats = aggregate_stats(&[], &build_changes(&build_entries, &installed));

    assert_eq!(stats.total_packages(), 0);
    assert_eq!(stats.artefacts, 3);
}

#[test]
fn artefact_fallback_covers_versionless_unparseable_entries() {
    // When `split_name_version` fails and we keep the raw entry as
    // `new` with an empty `name`, `is_system_artefact` should still
    // route home-manager-ish fallouts (no trailing digit) to the
    // artefact bucket rather than printing them as "packages".
    let installed: Vec<crate::nix::store::StorePackage> = vec![];
    let entries = vec!["hm_.manpath".to_string(), "options.json".to_string()];
    let changes = build_changes(&entries, &installed);
    for c in &changes {
        assert!(is_system_artefact(c), "{} should be an artefact via fallback", c.new);
    }
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
fn detects_dirty_tree_warning_from_nix_stderr() {
    let stderr = "warning: Git tree '/home/mae/nixos-config' is dirty\n";
    assert!(detect_dirty_tree_warning(stderr));
}

#[test]
fn detects_dirty_tree_warning_older_nix_phrasing() {
    // Older nix wrote it as "dirty Git tree '…'" — just in case
    // the user pins a stale nix version somewhere.
    let stderr = "warning: dirty Git tree '/home/mae/nixos-config'\n";
    assert!(detect_dirty_tree_warning(stderr));
}

#[test]
fn detects_dirty_tree_warning_absent() {
    let stderr = "no updates\n";
    assert!(!detect_dirty_tree_warning(stderr));
}

#[test]
fn summary_collapses_to_nothing_changed_when_artefacts_are_fully_explained() {
    // Inputs unchanged + dirty tree → the 19 artefacts are pure
    // re-eval noise. Headline stays "nothing changed", follow-up
    // line explains why.
    let stats = UpgradeStats { artefacts: 19, ..UpgradeStats::default() };
    let ctx = UpgradeContext { inputs_updated: 0, git_tree_dirty: true };
    let headline = render_summary_headline(&stats, &ctx);
    assert_eq!(headline, "nothing changed");

    let reason = explain_no_op_rebuild(&stats, &ctx).expect("should explain");
    assert!(reason.contains("dirty"), "reason was: {reason}");
    assert!(reason.contains("19 system artefact"), "reason was: {reason}");
}

#[test]
fn summary_mentions_reeval_when_inputs_unchanged_and_tree_clean() {
    let stats = UpgradeStats { artefacts: 5, ..UpgradeStats::default() };
    let ctx = UpgradeContext { inputs_updated: 0, git_tree_dirty: false };
    assert_eq!(render_summary_headline(&stats, &ctx), "nothing changed");

    let reason = explain_no_op_rebuild(&stats, &ctx).expect("should explain");
    assert!(reason.contains("home-manager"), "reason was: {reason}");
}

#[test]
fn summary_keeps_package_headline_when_real_packages_changed() {
    // Real packages changed → headline reports them, no follow-up.
    let stats = UpgradeStats {
        minor: 1, artefacts: 17, ..UpgradeStats::default()
    };
    let ctx = UpgradeContext { inputs_updated: 3, git_tree_dirty: false };
    let headline = render_summary_headline(&stats, &ctx);
    assert!(headline.contains("1 package"), "headline: {headline}");
    assert!(headline.contains("17 system artefact"), "headline: {headline}");
    assert!(explain_no_op_rebuild(&stats, &ctx).is_none());
}

#[test]
fn preview_warns_before_rebuild_when_tree_dirty_and_only_artefacts() {
    let stats = UpgradeStats { artefacts: 19, ..UpgradeStats::default() };
    let ctx = UpgradeContext { inputs_updated: 0, git_tree_dirty: true };
    let warning = preview_noop_warning(&stats, &ctx).expect("should warn");
    assert!(warning.contains("dirty"), "warning: {warning}");
    assert!(warning.contains("commit or stash"), "warning: {warning}");
    assert!(warning.contains("No package will change"), "warning: {warning}");
}

#[test]
fn preview_warns_when_tree_clean_but_only_artefacts() {
    let stats = UpgradeStats { artefacts: 3, ..UpgradeStats::default() };
    let ctx = UpgradeContext { inputs_updated: 0, git_tree_dirty: false };
    let warning = preview_noop_warning(&stats, &ctx).expect("should warn");
    assert!(warning.contains("home-manager internals"), "warning: {warning}");
    assert!(warning.contains("safe to skip"), "warning: {warning}");
}

#[test]
fn preview_stays_silent_when_inputs_moved() {
    // Inputs moved → the rebuild has a real cause even if only
    // artefacts show in the preview. No spurious warning.
    let stats = UpgradeStats { artefacts: 5, ..UpgradeStats::default() };
    let ctx = UpgradeContext { inputs_updated: 2, git_tree_dirty: false };
    assert!(preview_noop_warning(&stats, &ctx).is_none());
}

#[test]
fn preview_stays_silent_when_real_packages_change() {
    // Real package bump → no "no-op" warning even if the tree is dirty.
    let stats = UpgradeStats {
        minor: 1, artefacts: 10, ..UpgradeStats::default()
    };
    let ctx = UpgradeContext { inputs_updated: 0, git_tree_dirty: true };
    assert!(preview_noop_warning(&stats, &ctx).is_none());
}

#[test]
fn summary_no_follow_up_when_inputs_moved() {
    // Inputs moved but only artefacts got rebuilt — the cause is
    // obvious (inputs moved), no need for a dedicated explanation.
    let stats = UpgradeStats { artefacts: 3, ..UpgradeStats::default() };
    let ctx = UpgradeContext { inputs_updated: 1, git_tree_dirty: false };
    assert!(explain_no_op_rebuild(&stats, &ctx).is_none());
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
