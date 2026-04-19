//! Tests for the Repology entry-picker disambiguation logic.
//!
//! Each test case is a small fixture mirroring real data observed from
//! https://repology.org/api/v1/project/<name>. They guard against
//! regressions in the version-aware selection that fixed firefox,
//! breeze-icons, and exo phantom "Newer than nixpkgs" reports.

use super::*;

fn entry(repo: &str, src: Option<&str>, ver: &str) -> RepologyEntry {
    RepologyEntry {
        repo: repo.to_string(),
        version: Some(ver.to_string()),
        summary: None,
        binname: None,
        srcname: src.map(String::from),
        visiblename: src.map(String::from),
    }
}

fn entry_with_visible(
    repo: &str,
    src: Option<&str>,
    visible: Option<&str>,
    ver: &str,
) -> RepologyEntry {
    RepologyEntry {
        repo: repo.to_string(),
        version: Some(ver.to_string()),
        summary: None,
        binname: None,
        srcname: src.map(String::from),
        visiblename: visible.map(String::from),
    }
}

#[test]
fn parse_major_handles_simple_versions() {
    assert_eq!(parse_major("3.14.3"), Some(3));
    assert_eq!(parse_major("149.0.2"), Some(149));
    assert_eq!(parse_major("0"), Some(0));
}

#[test]
fn parse_major_handles_garbage() {
    assert_eq!(parse_major(""), None);
    assert_eq!(parse_major("not.a.version"), None);
    assert_eq!(parse_major("a.b.c"), None);
}

#[test]
fn parse_major_handles_calver() {
    // "2026.04.01" still has a numeric major. Filtering it semantically
    // is out of scope for this helper.
    assert_eq!(parse_major("2026.04.01"), Some(2026));
}

#[test]
fn field_matches_case_insensitive() {
    assert!(field_matches(&Some("Firefox".to_string()), "firefox"));
    assert!(field_matches(&Some("firefox".to_string()), "Firefox".to_lowercase().as_str()));
    assert!(!field_matches(&Some("firefox-esr".to_string()), "firefox"));
    assert!(!field_matches(&None, "firefox"));
}

#[test]
fn pick_returns_none_when_no_entries() {
    let result = pick_nix_entry(&[], &["firefox"], "nix_unstable", None);
    assert!(result.is_none());
}

#[test]
fn pick_returns_first_when_no_name_match_no_hint() {
    // Last-resort fallback: no name in the needles matches any entry.
    let entries = vec![
        entry("nix_unstable", Some("totally-different"), "1.0"),
        entry("nix_unstable", Some("also-different"), "2.0"),
    ];
    let picked = pick_nix_entry(&entries, &["banana"], "nix_unstable", None);
    assert_eq!(picked.unwrap().version.as_deref(), Some("1.0"));
}

#[test]
fn pick_firefox_with_installed_picks_real_firefox_not_esr() {
    // The actual case from production: firefox vs firefox-esr both have
    // visiblename "firefox", but only srcname=firefox is the right one
    // for a user on Firefox release.
    let entries = vec![
        entry_with_visible("nix_unstable", Some("firefox-esr"), Some("firefox"), "140.9.1"),
        entry_with_visible("nix_unstable", Some("firefox-mobile"), Some("firefox"), "149.0.2"),
        entry_with_visible("nix_unstable", Some("firefox"), Some("firefox"), "149.0.2"),
        entry_with_visible("nix_unstable", Some("firefox-bin"), Some("firefox-bin"), "149.0.2"),
    ];
    let picked = pick_nix_entry(
        &entries,
        &["firefox"],
        "nix_unstable",
        Some("149.0.2"),
    )
    .unwrap();
    // Name match (srcname=firefox) AND version match (149.0.2) — strategy 1.
    assert_eq!(picked.srcname.as_deref(), Some("firefox"));
    assert_eq!(picked.version.as_deref(), Some("149.0.2"));
}

#[test]
fn pick_breeze_icons_uses_version_when_name_doesnt_match() {
    // Real case: srcnames are namespaced (kdePackages.* / libsForQt5.*)
    // so the bare "breeze-icons" needle never matches. We must fall
    // through to version matching (strategy 2/3) to pick the KDE 6 entry.
    let entries = vec![
        entry("nix_unstable", Some("libsForQt5.breeze-icons"), "5.116.0"),
        entry("nix_unstable", Some("kdePackages.breeze-icons"), "6.20.0"),
    ];
    let picked = pick_nix_entry(
        &entries,
        &["breeze-icons"],
        "nix_unstable",
        Some("6.25.0"),  // user has KDE 6, no exact version match in repo
    )
    .unwrap();
    // Major version match (strategy 3): 6.x picks kdePackages.
    assert_eq!(picked.srcname.as_deref(), Some("kdePackages.breeze-icons"));
}

#[test]
fn pick_exo_disambiguates_unrelated_packages_by_version() {
    // exo (LLM tool, ~1.x) and xfce4-exo (Xfce file manager, 4.x) share
    // the same Repology project. Without a version hint, srcname=exo
    // matches the LLM tool. With installed=4.20.0 we should pick xfce.
    let entries = vec![
        entry("nix_unstable", Some("exo"), "1.0.69"),
        entry("nix_unstable", Some("xfce4-exo"), "4.20.0"),
    ];

    // Without hint: srcname match → wrong package (LLM exo).
    let picked_blind = pick_nix_entry(&entries, &["exo"], "nix_unstable", None).unwrap();
    assert_eq!(picked_blind.srcname.as_deref(), Some("exo"));

    // With version hint: exact version match → right package.
    let picked_hinted =
        pick_nix_entry(&entries, &["exo"], "nix_unstable", Some("4.20.0")).unwrap();
    assert_eq!(picked_hinted.srcname.as_deref(), Some("xfce4-exo"));
}

#[test]
fn pick_falls_back_to_name_when_version_matches_nothing() {
    // Installed version is wildly out of range — no exact, no major match.
    // Should fall through to name-match logic (strategy 4).
    let entries = vec![
        entry("nix_unstable", Some("foo"), "1.0"),
        entry("nix_unstable", Some("bar"), "2.0"),
    ];
    let picked =
        pick_nix_entry(&entries, &["bar"], "nix_unstable", Some("99.99.99")).unwrap();
    assert_eq!(picked.srcname.as_deref(), Some("bar"));
}

#[test]
fn pick_ignores_entries_from_other_repos() {
    let entries = vec![
        entry("debian", Some("firefox"), "999.0"),       // wrong repo
        entry("nix_stable_25_05", Some("firefox"), "146.0.1"),
        entry("nix_unstable", Some("firefox"), "149.0.2"),
    ];
    let picked = pick_nix_entry(&entries, &["firefox"], "nix_unstable", None).unwrap();
    assert_eq!(picked.version.as_deref(), Some("149.0.2"));
    assert_eq!(picked.repo, "nix_unstable");
}

#[test]
fn pick_uses_visiblename_when_srcname_misses() {
    // Older Repology data sometimes only populates visiblename.
    let entries = vec![
        entry_with_visible("nix_unstable", None, Some("foo"), "1.0"),
    ];
    let picked = pick_nix_entry(&entries, &["foo"], "nix_unstable", None).unwrap();
    assert_eq!(picked.version.as_deref(), Some("1.0"));
}

#[test]
fn split_cache_hits_partitions_correctly() {
    let mut cache = cache::Cache::default();
    cache.entries.insert(
        "firefox".to_string(),
        cache::CachedPackage {
            version: Some("149.0.2".to_string()),
            description: Some("Mozilla Firefox".to_string()),
        },
    );
    let packages = vec![
        ("firefox".to_string(), Some("149.0.2".to_string())),
        ("kicad".to_string(), Some("8.0.5".to_string())),
        ("alacritty".to_string(), None),
    ];
    let (hits, misses) = split_cache_hits(&packages, &cache);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "firefox");
    assert_eq!(hits[0].version.as_deref(), Some("149.0.2"));

    assert_eq!(misses.len(), 2);
    let miss_names: Vec<&str> = misses.iter().map(|(n, _)| n.as_str()).collect();
    assert!(miss_names.contains(&"kicad"));
    assert!(miss_names.contains(&"alacritty"));
}

#[test]
fn split_cache_hits_all_miss_on_empty_cache() {
    let cache = cache::Cache::default();
    let packages = vec![
        ("a".to_string(), None),
        ("b".to_string(), Some("1.0".to_string())),
    ];
    let (hits, misses) = split_cache_hits(&packages, &cache);
    assert_eq!(hits.len(), 0);
    assert_eq!(misses.len(), 2);
}

#[test]
fn split_cache_hits_preserves_installed_hint_on_miss() {
    // The installed-version hint is carried forward to the API layer so
    // pick_nix_entry can disambiguate Repology projects downstream.
    let cache = cache::Cache::default();
    let packages = vec![("firefox".to_string(), Some("149.0.2".to_string()))];
    let (_hits, misses) = split_cache_hits(&packages, &cache);
    assert_eq!(misses[0].1.as_deref(), Some("149.0.2"));
}

#[test]
fn retry_after_honors_server_seconds() {
    assert_eq!(parse_retry_after(Some("5")), 5);
    assert_eq!(parse_retry_after(Some("  12  ")), 12);
}

#[test]
fn retry_after_caps_at_max() {
    // Beyond the cap we fall back to the default — we'd rather return
    // "unknown" than block a user command for a full minute.
    assert_eq!(parse_retry_after(Some("60")), RATE_LIMIT_RETRY_SECS);
    assert_eq!(
        parse_retry_after(Some(&(RATE_LIMIT_MAX_WAIT_SECS + 1).to_string())),
        RATE_LIMIT_RETRY_SECS
    );
}

#[test]
fn retry_after_accepts_boundary() {
    assert_eq!(parse_retry_after(Some("1")), 1);
    assert_eq!(
        parse_retry_after(Some(&RATE_LIMIT_MAX_WAIT_SECS.to_string())),
        RATE_LIMIT_MAX_WAIT_SECS
    );
}

#[test]
fn retry_after_falls_back_on_missing_or_invalid() {
    assert_eq!(parse_retry_after(None), RATE_LIMIT_RETRY_SECS);
    assert_eq!(parse_retry_after(Some("")), RATE_LIMIT_RETRY_SECS);
    assert_eq!(parse_retry_after(Some("0")), RATE_LIMIT_RETRY_SECS);
    // HTTP-date form — not parsed, falls back.
    assert_eq!(
        parse_retry_after(Some("Wed, 21 Oct 2026 07:28:00 GMT")),
        RATE_LIMIT_RETRY_SECS
    );
    assert_eq!(parse_retry_after(Some("soon")), RATE_LIMIT_RETRY_SECS);
}
