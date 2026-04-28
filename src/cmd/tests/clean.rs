//! Tests for `cmd::clean`.

#![allow(unused_imports)]

use super::*;
use std::collections::HashSet;

fn declared(names: &[&str]) -> HashSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

#[test]
fn find_orphan_pins_returns_pins_not_in_declared() {
    let pins = vec!["firefox".to_string(), "kicad".to_string()];
    let decl = declared(&["kicad"]);
    let orphans = find_orphan_pins(&pins, &decl);
    assert_eq!(orphans, vec!["firefox".to_string()]);
}

#[test]
fn find_orphan_pins_handles_empty_pins() {
    let decl = declared(&["firefox"]);
    let orphans = find_orphan_pins(&[], &decl);
    assert!(orphans.is_empty());
}

#[test]
fn find_orphan_pins_handles_all_declared() {
    let pins = vec!["firefox".to_string(), "kicad".to_string()];
    let decl = declared(&["firefox", "kicad", "vivaldi"]);
    let orphans = find_orphan_pins(&pins, &decl);
    assert!(orphans.is_empty());
}

#[test]
fn find_orphan_pins_when_no_modules_returns_all_as_orphans() {
    let pins = vec!["firefox".to_string()];
    let decl = HashSet::new();
    let orphans = find_orphan_pins(&pins, &decl);
    assert_eq!(orphans, vec!["firefox".to_string()]);
}
