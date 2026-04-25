use super::*;

use crate::nix::freezes::{FreezeEntry, Freezes};

fn freeze(version: &str, rev: &str) -> FreezeEntry {
    FreezeEntry {
        rev: rev.to_string(),
        nar_hash: "sha256-deadbeef".to_string(),
        version: version.to_string(),
        frozen_at: "2026-04-25".to_string(),
        major_constraint: None,
    }
}

fn freezes(entries: &[(&str, FreezeEntry)]) -> Freezes {
    entries
        .iter()
        .map(|(name, e)| ((*name).to_string(), e.clone()))
        .collect()
}

fn pins(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn no_change_returns_none() {
    let p = pins(&["firefox", "vivaldi"]);
    let f = freezes(&[("mesa", freeze("24.0", "abcdef0"))]);
    assert!(compute_pin_freeze_delta(&p, &p, &f, &f).is_none());
}

#[test]
fn pin_addition_is_captured() {
    let prev = pins(&["firefox"]);
    let cur = pins(&["firefox", "vivaldi"]);
    let empty = freezes(&[]);
    let d = compute_pin_freeze_delta(&prev, &cur, &empty, &empty).unwrap();
    assert_eq!(d.pins_added, vec!["vivaldi".to_string()]);
    assert!(d.pins_removed.is_empty());
}

#[test]
fn pin_removal_is_captured() {
    let prev = pins(&["firefox", "vivaldi"]);
    let cur = pins(&["firefox"]);
    let empty = freezes(&[]);
    let d = compute_pin_freeze_delta(&prev, &cur, &empty, &empty).unwrap();
    assert_eq!(d.pins_removed, vec!["vivaldi".to_string()]);
    assert!(d.pins_added.is_empty());
}

#[test]
fn pin_swap_shows_both_sides() {
    // The whole set turning over still surfaces the symmetric diff
    // — important so users see "I dropped X and picked up Y on the
    // same generation".
    let prev = pins(&["firefox"]);
    let cur = pins(&["vivaldi"]);
    let empty = freezes(&[]);
    let d = compute_pin_freeze_delta(&prev, &cur, &empty, &empty).unwrap();
    assert_eq!(d.pins_added, vec!["vivaldi".to_string()]);
    assert_eq!(d.pins_removed, vec!["firefox".to_string()]);
}

#[test]
fn freeze_addition_keeps_the_version() {
    let empty_pins: Vec<String> = Vec::new();
    let prev = freezes(&[]);
    let cur = freezes(&[("firefox", freeze("140.2", "aaa1111"))]);
    let d = compute_pin_freeze_delta(&empty_pins, &empty_pins, &prev, &cur).unwrap();
    assert_eq!(
        d.freezes_added,
        vec![("firefox".to_string(), "140.2".to_string())]
    );
}

#[test]
fn freeze_rev_bump_shows_up_as_changed() {
    // `cheni upgrade` on a `--major N` freeze rewrites the rev (and
    // typically the version). That should be visible in history as
    // a change, not as add+remove.
    let empty_pins: Vec<String> = Vec::new();
    let prev = freezes(&[("firefox", freeze("140.1", "aaa1111"))]);
    let cur = freezes(&[("firefox", freeze("140.2", "bbb2222"))]);
    let d = compute_pin_freeze_delta(&empty_pins, &empty_pins, &prev, &cur).unwrap();
    assert_eq!(
        d.freezes_changed,
        vec![("firefox".to_string(), "140.1".to_string(), "140.2".to_string())]
    );
    assert!(d.freezes_added.is_empty());
    assert!(d.freezes_removed.is_empty());
}

#[test]
fn freeze_same_rev_is_not_flagged() {
    // Identical entries on either side must NOT register — otherwise
    // every generation between two real changes would carry a noise
    // line.
    let empty_pins: Vec<String> = Vec::new();
    let entry = freeze("140.2", "bbb2222");
    let prev = freezes(&[("firefox", entry.clone())]);
    let cur = freezes(&[("firefox", entry)]);
    assert!(compute_pin_freeze_delta(&empty_pins, &empty_pins, &prev, &cur).is_none());
}

#[test]
fn freeze_removal_is_captured() {
    let empty_pins: Vec<String> = Vec::new();
    let prev = freezes(&[("mesa", freeze("24.0", "ccc"))]);
    let cur = freezes(&[]);
    let d = compute_pin_freeze_delta(&empty_pins, &empty_pins, &prev, &cur).unwrap();
    assert_eq!(d.freezes_removed, vec!["mesa".to_string()]);
}

#[test]
fn format_combines_sections_with_middle_dot() {
    let d = PinFreezeDelta {
        pins_added: vec!["vivaldi".to_string()],
        pins_removed: vec![],
        freezes_added: vec![("firefox".to_string(), "140.2".to_string())],
        freezes_changed: vec![],
        freezes_removed: vec![],
    };
    assert_eq!(
        format_pin_freeze_delta(&d),
        "+pinned vivaldi · +frozen firefox@140.2"
    );
}

#[test]
fn format_lists_under_three_items_inline() {
    let d = PinFreezeDelta {
        pins_added: vec!["a".into(), "b".into()],
        pins_removed: vec![],
        freezes_added: vec![],
        freezes_changed: vec![],
        freezes_removed: vec![],
    };
    assert_eq!(format_pin_freeze_delta(&d), "+pinned a, b");
}

#[test]
fn format_collapses_long_lists_with_overflow_marker() {
    let d = PinFreezeDelta {
        pins_added: vec!["a".into(), "b".into(), "c".into(), "d".into(), "e".into()],
        pins_removed: vec![],
        freezes_added: vec![],
        freezes_changed: vec![],
        freezes_removed: vec![],
    };
    assert_eq!(format_pin_freeze_delta(&d), "+pinned a, b, c (+2 more)");
}

#[test]
fn format_freeze_change_uses_arrow_between_versions() {
    let d = PinFreezeDelta {
        pins_added: vec![],
        pins_removed: vec![],
        freezes_added: vec![],
        freezes_changed: vec![(
            "firefox".to_string(),
            "140.1".to_string(),
            "140.2".to_string(),
        )],
        freezes_removed: vec![],
    };
    assert_eq!(format_pin_freeze_delta(&d), "~frozen firefox 140.1→140.2");
}

#[test]
fn format_freeze_added_without_version_drops_the_at_suffix() {
    // Older freezes written by a pre-version-tracking cheni have an
    // empty `version` field. The annotation should fall back to just
    // the package name, not "firefox@".
    let d = PinFreezeDelta {
        pins_added: vec![],
        pins_removed: vec![],
        freezes_added: vec![("firefox".to_string(), "".to_string())],
        freezes_changed: vec![],
        freezes_removed: vec![],
    };
    assert_eq!(format_pin_freeze_delta(&d), "+frozen firefox");
}
