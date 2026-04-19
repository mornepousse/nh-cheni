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
