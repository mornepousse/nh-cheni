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
    // NB: kernel-modules / -shrunk classification is no longer driven
    // by `is_system_artefact_name` — it lives in
    // `has_kernel_artefact_version_suffix` because after
    // `split_name_version` the modules marker ends up in the version
    // segment, not the name. Tests for that path live alongside
    // `linux_modules_artefact_still_classified_as_artefact` below.
    for name in [
        "hm_.manpath",
        "home-manager-path",
        "home-configuration-reference-manpage",
        "nixos-system-my-host",
        "system-path",
        "closure-info",
        "initrd-linux-6.12.1",
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

// --- detect_critical_component_changes ---

fn change(name: &str, new: &str) -> PackageChange {
    PackageChange {
        name: name.to_string(),
        old: None,
        new: new.to_string(),
        diff: crate::version::compare::VersionDiff::Equal,
    }
}

#[test]
fn detect_critical_flags_dbus_broker_landing() {
    // The classic case: dbus-broker shows up in either bucket
    // (download or build). Anywhere is enough to trigger the
    // pre-switch refusal at activation time.
    let fetch = vec![change("dbus-broker", "37")];
    let build: Vec<PackageChange> = vec![];
    let critical = detect_critical_component_changes(&fetch, &build);
    assert_eq!(critical.len(), 1);
    assert!(critical[0].contains("dbus-broker"));
}

#[test]
fn detect_critical_handles_dbus_broker_in_build_section_too() {
    let fetch: Vec<PackageChange> = vec![];
    let build = vec![change("dbus-broker", "37")];
    let critical = detect_critical_component_changes(&fetch, &build);
    assert_eq!(critical.len(), 1);
}

#[test]
fn detect_critical_returns_empty_when_no_trigger_present() {
    // Routine upgrade — no critical-swap signal. The detector must
    // stay silent so common-case upgrades aren't cluttered with the
    // boot-mode prompt.
    let fetch = vec![change("firefox", "150.0"), change("kicad", "10.0.1")];
    let build = vec![change("teamspeak6-client", "6.0.0")];
    assert!(detect_critical_component_changes(&fetch, &build).is_empty());
}

#[test]
fn detect_critical_does_not_match_unrelated_dbus_packages() {
    // A package that merely mentions "dbus" in its name (dbus-1,
    // dbus-glib, python-dbus, …) must not falsely trigger. The
    // signal we care about is exactly `dbus-broker`.
    let fetch = vec![
        change("dbus-glib", "0.112"),
        change("python3.13-dbus", "1.4.0"),
    ];
    let build: Vec<PackageChange> = vec![];
    assert!(detect_critical_component_changes(&fetch, &build).is_empty());
}

// --- has_kernel_artefact_version_suffix + kernel classification ---

#[test]
fn kernel_artefact_suffix_flags_modules_and_shrunk() {
    assert!(has_kernel_artefact_version_suffix("6.19.12-modules"));
    assert!(has_kernel_artefact_version_suffix("6.19.12-shrunk"));
    assert!(has_kernel_artefact_version_suffix("6.19.12-modules-shrunk"));
}

#[test]
fn kernel_artefact_suffix_does_not_flag_bare_versions() {
    // The bare kernel (`linux-zen-6.19.12`) splits into name="linux-zen"
    // and version="6.19.12" — the suffix check must say "real package"
    // so the user actually sees the kernel bump in the preview.
    assert!(!has_kernel_artefact_version_suffix("6.19.12"));
    assert!(!has_kernel_artefact_version_suffix("6.19.12-rc1"));
    assert!(!has_kernel_artefact_version_suffix("0-unstable-2026-04-22"));
    assert!(!has_kernel_artefact_version_suffix("20240909"));
}

#[test]
fn bare_kernel_classified_as_real_package() {
    // The motivation for the whole filter refinement: `linux-zen`
    // was being eaten by the artefact bucket, so kernel updates
    // never showed up as user-visible changes in the preview.
    let kernel = change("linux-zen", "6.19.12");
    assert!(!is_system_artefact(&kernel));
}

#[test]
fn linux_modules_artefact_still_classified_as_artefact() {
    // The `-modules` suffix lives in the version after split_name_version
    // — the discriminant has to look there, not at the name.
    let modules = change("linux-zen", "6.19.12-modules");
    assert!(is_system_artefact(&modules));
    let shrunk = change("linux-zen", "6.19.12-modules-shrunk");
    assert!(is_system_artefact(&shrunk));
}

#[test]
fn linux_firmware_classified_as_real_package() {
    // Now that the blanket `linux-` prefix is gone, firmware blobs
    // surface as user-visible too — they're updated in lockstep with
    // many kernel bumps and can introduce real behaviour changes.
    let firmware = change("linux-firmware", "20240909");
    assert!(!is_system_artefact(&firmware));
}

#[test]
fn linux_pam_classified_as_real_package() {
    // Ironic miss of the old blanket prefix: linux-pam is just an
    // auth library, has nothing to do with the kernel.
    let pam = change("linux-pam", "1.5.3");
    assert!(!is_system_artefact(&pam));
}

#[test]
fn initrd_linux_still_classified_as_artefact() {
    // initrd-linux-zen-6.19.12 → split: name="initrd-linux-zen",
    // version="6.19.12". The `initrd-linux-` prefix is still in
    // PREFIXES because it really is a build artefact (the initrd
    // is generated from kernel modules + userspace tools, not a
    // package the user installs).
    let initrd = change("initrd-linux-zen", "6.19.12");
    assert!(is_system_artefact(&initrd));
}

// --- extract_store_name ---

#[test]
fn extract_store_name_strips_hash_prefix() {
    // Standard /nix/store/<32-char-hash>-<name> path.
    let path = "/nix/store/abcdefghijklmnopqrstuvwxyz012345-vivaldi-7.9";
    assert_eq!(extract_store_name(path), Some("vivaldi-7.9".to_string()));
}

#[test]
fn extract_store_name_strips_drv_suffix() {
    let path = "/nix/store/abcdefghijklmnopqrstuvwxyz012345-firefox-150.0.drv";
    assert_eq!(extract_store_name(path), Some("firefox-150.0".to_string()));
}

#[test]
fn extract_store_name_returns_none_for_short_path() {
    // Less than 33 chars after the /nix/store/ prefix → can't slice.
    let path = "/nix/store/tooshort";
    assert_eq!(extract_store_name(path), None);
}

#[test]
fn extract_store_name_returns_none_without_nix_store_prefix() {
    assert_eq!(extract_store_name("/usr/local/bin/something"), None);
}

// --- parse_dry_run_summary ---

#[test]
fn parse_dry_run_summary_empty_on_no_changes() {
    let stderr = "nothing to do\n";
    let (build, fetch) = parse_dry_run_summary(stderr);
    assert!(build.is_empty());
    assert!(fetch.is_empty());
}

#[test]
fn parse_dry_run_summary_parses_build_section() {
    let hash = "a".repeat(32);
    let stderr = format!(
        "these 2 derivations will be built:\n  /nix/store/{hash}-openssl-3.5.0.drv\n  /nix/store/{hash}-python3-3.12.0.drv\n",
        hash = hash
    );
    let (build, fetch) = parse_dry_run_summary(&stderr);
    assert_eq!(build, vec!["openssl-3.5.0", "python3-3.12.0"]);
    assert!(fetch.is_empty());
}

#[test]
fn parse_dry_run_summary_parses_fetch_section() {
    let hash = "b".repeat(32);
    let stderr = format!(
        "these 3 paths will be fetched (45.0 MiB download, 120 MiB unpacked):\n  /nix/store/{hash}-firefox-150.0\n  /nix/store/{hash}-kicad-9.0.1\n  /nix/store/{hash}-vivaldi-7.9\n",
        hash = hash
    );
    let (build, fetch) = parse_dry_run_summary(&stderr);
    assert!(build.is_empty());
    assert_eq!(fetch, vec!["firefox-150.0", "kicad-9.0.1", "vivaldi-7.9"]);
}

#[test]
fn parse_dry_run_summary_handles_both_sections() {
    let hash = "c".repeat(32);
    let stderr = format!(
        "these 1 derivations will be built:\n  /nix/store/{h}-openssl-3.5.0.drv\nthese 2 paths will be fetched (10 MiB download, 30 MiB unpacked):\n  /nix/store/{h}-firefox-150.0\n  /nix/store/{h}-chromium-130.0\n",
        h = hash
    );
    let (build, fetch) = parse_dry_run_summary(&stderr);
    assert_eq!(build, vec!["openssl-3.5.0"]);
    assert_eq!(fetch, vec!["firefox-150.0", "chromium-130.0"]);
}

#[test]
fn parse_dry_run_summary_singular_headers_are_recognised() {
    // `nix build --dry-run` uses the singular form when exactly one
    // derivation / path is involved.
    let hash = "d".repeat(32);
    let stderr = format!(
        "this derivation will be built:\n  /nix/store/{h}-git-2.49.0.drv\nthis path will be fetched (5 MiB download, 18 MiB unpacked):\n  /nix/store/{h}-neovim-0.11.0\n",
        h = hash
    );
    let (build, fetch) = parse_dry_run_summary(&stderr);
    assert_eq!(build, vec!["git-2.49.0"]);
    assert_eq!(fetch, vec!["neovim-0.11.0"]);
}

#[test]
fn parse_dry_run_summary_section_ends_at_non_store_line() {
    // A non-/nix/store/ line resets the active section, so entries
    // that appear after an unrelated line are not mistakenly bucketed.
    let hash = "e".repeat(32);
    let stderr = format!(
        "these 1 derivations will be built:\n  /nix/store/{h}-openssl-3.5.0.drv\nsome other output line\n  /nix/store/{h}-unrelated-1.0.drv\n",
        h = hash
    );
    let (build, _) = parse_dry_run_summary(&stderr);
    // Only the first entry, before the reset line, is captured.
    assert_eq!(build, vec!["openssl-3.5.0"]);
}
