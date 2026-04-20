use super::*;
use crate::nix::freezes::{FreezeEntry, Freezes};

fn sample_entry(version: &str, date: &str) -> FreezeEntry {
    FreezeEntry {
        rev: "abcdef0123456789abcdef0123456789abcdef01".to_string(),
        nar_hash: "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string(),
        version: version.to_string(),
        frozen_at: date.to_string(),
    }
}

#[test]
fn split_out_frozen_removes_matching_packages() {
    // Two packages, one frozen — the frozen one is extracted into a row
    // with its installed version & freeze date, and is no longer in the
    // to-check list that goes to Repology.
    let mut packages = vec![
        ("firefox".to_string(), "127.0.1".to_string()),
        ("vivaldi".to_string(), "7.9".to_string()),
    ];
    let mut freezes = Freezes::new();
    freezes.insert("firefox".to_string(), sample_entry("127.0.1", "2026-04-20"));

    let rows = split_out_frozen(&mut packages, &freezes);

    assert_eq!(packages, vec![("vivaldi".to_string(), "7.9".to_string())]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "firefox");
    assert_eq!(rows[0].version, "127.0.1");
    assert_eq!(rows[0].frozen_at, "2026-04-20");
}

#[test]
fn split_out_frozen_empty_freezes_is_noop() {
    let mut packages = vec![
        ("firefox".to_string(), "127.0.1".to_string()),
        ("vivaldi".to_string(), "7.9".to_string()),
    ];
    let before = packages.clone();
    let freezes = Freezes::new();

    let rows = split_out_frozen(&mut packages, &freezes);

    assert!(rows.is_empty());
    assert_eq!(packages, before, "vector must be untouched when no freezes");
}

#[test]
fn split_out_frozen_preserves_order_of_non_frozen() {
    // The non-frozen packages keep their original relative order, which
    // matters because cheni check surfaces updates in the same order
    // they're scanned (broadly alphabetical, user expects stable output).
    let mut packages = vec![
        ("a".to_string(), "1".to_string()),
        ("b".to_string(), "2".to_string()),
        ("c".to_string(), "3".to_string()),
        ("d".to_string(), "4".to_string()),
    ];
    let mut freezes = Freezes::new();
    freezes.insert("b".to_string(), sample_entry("2", "2026-04-20"));
    freezes.insert("d".to_string(), sample_entry("4", "2026-04-20"));

    let _ = split_out_frozen(&mut packages, &freezes);
    let names: Vec<&str> = packages.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, vec!["a", "c"]);
}

#[test]
fn split_out_frozen_skips_freezes_not_in_check_list() {
    // A freeze exists for a package that isn't in the to-check set
    // (e.g. excluded because it wasn't in modules/). The row list must
    // not contain it — we only report what was going to be checked.
    let mut packages = vec![("firefox".to_string(), "127.0.1".to_string())];
    let mut freezes = Freezes::new();
    freezes.insert("kitty".to_string(), sample_entry("0.38", "2026-04-20"));

    let rows = split_out_frozen(&mut packages, &freezes);

    assert_eq!(packages, vec![("firefox".to_string(), "127.0.1".to_string())]);
    assert!(rows.is_empty());
}
