use super::*;

#[test]
fn relevance_rank_orders_exact_prefix_substring_other() {
    // Lower number = more relevant. The four buckets must be strict
    // (0 < 1 < 2 < 3) so the sort in run() puts exact matches on top
    // and irrelevant description hits at the bottom.
    assert_eq!(relevance_rank("firefox", "firefox"), 0);
    assert_eq!(relevance_rank("firefox-esr", "firefox"), 1);
    assert_eq!(relevance_rank("mozfirefox", "firefox"), 2);
    assert_eq!(relevance_rank("chrome", "firefox"), 3);
}

#[test]
fn parse_and_sort_results_puts_exact_match_first() {
    // Build a nix-search-shaped JSON map with the exact match deliberately
    // last in iteration order — the sort must lift it to the top.
    let mut map = serde_json::Map::new();
    map.insert(
        "legacyPackages.x86_64-linux.firefox-esr".into(),
        serde_json::json!({ "version": "128.9.0", "description": "Firefox ESR" }),
    );
    map.insert(
        "legacyPackages.x86_64-linux.firefox".into(),
        serde_json::json!({ "version": "149.0.2", "description": "Mozilla Firefox" }),
    );

    let results = parse_and_sort_results(&map, "firefox");
    assert_eq!(results[0].0, "firefox");
    assert_eq!(results[0].1, "149.0.2");
    assert_eq!(results[1].0, "firefox-esr");
}

#[test]
fn parse_and_sort_results_tie_breaks_alphabetically() {
    // Two entries that share a bucket (both are prefix matches) must
    // fall back to alphabetical order for stable output.
    let mut map = serde_json::Map::new();
    map.insert(
        "legacyPackages.x86_64-linux.firefox-esr".into(),
        serde_json::json!({ "version": "1", "description": "" }),
    );
    map.insert(
        "legacyPackages.x86_64-linux.firefox-beta".into(),
        serde_json::json!({ "version": "2", "description": "" }),
    );
    let results = parse_and_sort_results(&map, "firefox-");
    assert_eq!(results[0].0, "firefox-beta");
    assert_eq!(results[1].0, "firefox-esr");
}

// --- pad_to (column alignment) ---

#[test]
fn pad_to_short_name_reaches_full_column_width() {
    assert_eq!(pad_to("abc", 10).chars().count(), 7);
}

#[test]
fn pad_to_at_or_over_column_returns_a_single_space() {
    // The original "{:<W}" format string would have produced zero
    // padding once the content reached W, visually merging two
    // adjacent columns. We always emit at least one separator.
    assert_eq!(pad_to("0123456789", 10), " ");
    assert_eq!(pad_to("01234567890123", 10), " ");
}

#[test]
fn pad_to_handles_empty_input() {
    assert_eq!(pad_to("", 5).chars().count(), 5);
}

#[test]
fn pad_to_counts_unicode_code_points_not_bytes() {
    // "café" is 4 chars, 5 bytes — the visible width is 4.
    assert_eq!(pad_to("café", 6).chars().count(), 2);
}

// --- LocalState badges ---

fn local_state(installed: &[&str], pinned: &[&str], frozen: &[(&str, &str)]) -> LocalState {
    LocalState {
        installed: installed.iter().map(|s| (*s).to_string()).collect(),
        pinned: pinned.iter().map(|s| (*s).to_string()).collect(),
        frozen: frozen
            .iter()
            .map(|(n, v)| ((*n).to_string(), (*v).to_string()))
            .collect(),
    }
}

#[test]
fn local_state_returns_none_when_package_is_unknown() {
    let s = local_state(&["alacritty"], &["vivaldi"], &[]);
    assert!(s.badges("firefox").is_none());
}

#[test]
fn local_state_pinned_renders_a_simple_marker() {
    let s = local_state(&[], &["vivaldi"], &[]);
    assert_eq!(s.badges("vivaldi").as_deref(), Some("pinned"));
}

#[test]
fn local_state_frozen_includes_version_when_present() {
    let s = local_state(&[], &[], &[("firefox", "140.2")]);
    assert_eq!(s.badges("firefox").as_deref(), Some("frozen@140.2"));
}

#[test]
fn local_state_frozen_falls_back_to_bare_marker_when_version_absent() {
    // Older freezes files have an empty version field — render the
    // marker without a trailing "@".
    let s = local_state(&[], &[], &[("firefox", "")]);
    assert_eq!(s.badges("firefox").as_deref(), Some("frozen"));
}

#[test]
fn local_state_combines_markers_in_canonical_order() {
    // The order is frozen, pinned, installed — mirrors the policy
    // hierarchy from most-specific (locked rev) to most-general
    // (declared in modules), so the eye lands on the strongest
    // commitment first.
    let s = local_state(&["firefox"], &["firefox"], &[("firefox", "140.2")]);
    assert_eq!(
        s.badges("firefox").as_deref(),
        Some("frozen@140.2 · pinned · installed")
    );
}

// --- repology_differs ---

#[test]
fn repology_differs_swallows_placeholder_versions() {
    // "?" is the substitute we use when nix search has no version field.
    // Treating that as a delta would fire for every weird entry.
    assert!(!repology_differs("?", "1.2.3"));
    assert!(!repology_differs("1.2.3", "?"));
    assert!(!repology_differs("", "1.2.3"));
}

#[test]
fn repology_differs_treats_missing_trailing_zero_as_equal() {
    // 1.2 and 1.2.0 must not produce a "→ 1.2.0 upstream" annotation.
    assert!(!repology_differs("1.2", "1.2.0"));
    assert!(!repology_differs("1.2.0", "1.2"));
}

#[test]
fn repology_differs_flags_real_version_drift() {
    assert!(repology_differs("1.2.3", "1.2.4"));
    assert!(repology_differs("1.2.3", "2.0.0"));
}

#[test]
fn repology_differs_compares_strings_when_parser_yields_nothing() {
    // Hash-only "versions" can come out of nixpkgs-unstable for some
    // git-pinned derivations. Fall back to byte equality so we don't
    // wrongly flag identical strings as a drift.
    assert!(!repology_differs("git-abc123", "git-abc123"));
    assert!(repology_differs("git-abc123", "git-def456"));
}

// --- build_annotation ---

#[test]
fn build_annotation_returns_none_for_an_uninteresting_row() {
    let upstream = std::collections::HashMap::new();
    let local = LocalState::empty();
    assert!(build_annotation("firefox", "1.0.0", &upstream, &local).is_none());
}

#[test]
fn build_annotation_returns_only_upstream_when_no_local_state() {
    let mut upstream = std::collections::HashMap::new();
    upstream.insert("firefox".to_string(), "2.0.0".to_string());
    let local = LocalState::empty();
    assert_eq!(
        build_annotation("firefox", "1.0.0", &upstream, &local).as_deref(),
        Some("→ 2.0.0 upstream")
    );
}

#[test]
fn build_annotation_returns_only_local_when_repology_matches_nixpkgs() {
    let mut upstream = std::collections::HashMap::new();
    upstream.insert("firefox".to_string(), "1.0.0".to_string());
    let local = local_state(&[], &["firefox"], &[]);
    assert_eq!(
        build_annotation("firefox", "1.0.0", &upstream, &local).as_deref(),
        Some("pinned")
    );
}

#[test]
fn build_annotation_combines_upstream_and_local_with_middle_dot() {
    let mut upstream = std::collections::HashMap::new();
    upstream.insert("firefox".to_string(), "2.0.0".to_string());
    let local = local_state(&[], &[], &[("firefox", "1.0.0")]);
    assert_eq!(
        build_annotation("firefox", "1.0.0", &upstream, &local).as_deref(),
        Some("→ 2.0.0 upstream · frozen@1.0.0")
    );
}

#[test]
fn parse_and_sort_results_handles_missing_fields() {
    // Real-world: some `nix search` entries have no description and/or
    // no version — substitute "" and "?" rather than panic.
    let mut map = serde_json::Map::new();
    map.insert(
        "legacyPackages.x86_64-linux.ghost".into(),
        serde_json::json!({}),
    );
    let results = parse_and_sort_results(&map, "ghost");
    assert_eq!(results[0].0, "ghost");
    assert_eq!(results[0].1, "?");
    assert_eq!(results[0].2, "");
}
