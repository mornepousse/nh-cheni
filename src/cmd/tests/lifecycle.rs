//! Tests for `cmd::lifecycle`.
//!
//! Tests only pure validation helpers — nothing that touches the
//! filesystem or calls nix/store. Those paths are covered indirectly by
//! the existing pins and freezes tests.

use std::collections::BTreeMap;

use crate::nix::freezes::FreezeEntry;
use crate::cmd::lifecycle::{validate_promote_preconditions, validate_demote_preconditions};

// --- helpers ------------------------------------------------------------

fn make_freeze_entry() -> FreezeEntry {
    FreezeEntry {
        rev: "abcdef1234567890abcdef1234567890abcdef12".to_string(),
        nar_hash: "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string(),
        version: "1.0.0".to_string(),
        frozen_at: "2026-04-29".to_string(),
        major_constraint: None,
    }
}

// --- validate_promote_preconditions -------------------------------------

#[test]
fn promote_ok_when_frozen_and_not_pinned() {
    let mut freezes = BTreeMap::new();
    freezes.insert("firefox".to_string(), make_freeze_entry());
    let pins: Vec<String> = vec![];

    let result = validate_promote_preconditions("firefox", &freezes, &pins);
    assert!(result.is_ok(), "expected ok, got: {:?}", result);
}

#[test]
fn promote_bails_when_not_frozen() {
    let freezes = BTreeMap::new();
    let pins: Vec<String> = vec![];

    let err = validate_promote_preconditions("firefox", &freezes, &pins)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("not currently frozen"),
        "expected 'not currently frozen' in: {err}"
    );
    assert!(
        err.contains("cheni freeze firefox"),
        "expected hint 'cheni freeze firefox' in: {err}"
    );
}

#[test]
fn promote_bails_on_inconsistent_state() {
    let mut freezes = BTreeMap::new();
    freezes.insert("firefox".to_string(), make_freeze_entry());
    let pins = vec!["firefox".to_string()];

    let err = validate_promote_preconditions("firefox", &freezes, &pins)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("inconsistent state"),
        "expected 'inconsistent state' in: {err}"
    );
    assert!(
        err.contains("cheni doctor"),
        "expected 'cheni doctor' in: {err}"
    );
}

// --- validate_demote_preconditions --------------------------------------

#[test]
fn demote_ok_when_pinned_and_not_frozen() {
    let pins = vec!["firefox".to_string()];
    let freezes = BTreeMap::new();

    let result = validate_demote_preconditions("firefox", &pins, &freezes);
    assert!(result.is_ok(), "expected ok, got: {:?}", result);
}

#[test]
fn demote_bails_when_not_pinned() {
    let pins: Vec<String> = vec![];
    let freezes = BTreeMap::new();

    let err = validate_demote_preconditions("firefox", &pins, &freezes)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("not currently pinned"),
        "expected 'not currently pinned' in: {err}"
    );
    assert!(
        err.contains("cheni pin firefox"),
        "expected hint 'cheni pin firefox' in: {err}"
    );
}

#[test]
fn demote_bails_on_inconsistent_state() {
    let pins = vec!["firefox".to_string()];
    let mut freezes = BTreeMap::new();
    freezes.insert("firefox".to_string(), make_freeze_entry());

    let err = validate_demote_preconditions("firefox", &pins, &freezes)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("inconsistent state"),
        "expected 'inconsistent state' in: {err}"
    );
    assert!(
        err.contains("cheni doctor"),
        "expected 'cheni doctor' in: {err}"
    );
}

#[test]
fn promote_bails_does_not_affect_other_packages() {
    // "vivaldi" is frozen, "firefox" is not — promote("firefox") should bail
    let mut freezes = BTreeMap::new();
    freezes.insert("vivaldi".to_string(), make_freeze_entry());
    let pins: Vec<String> = vec![];

    let err = validate_promote_preconditions("firefox", &freezes, &pins)
        .unwrap_err()
        .to_string();
    assert!(err.contains("not currently frozen"), "{err}");
}

#[test]
fn demote_bails_does_not_affect_other_packages() {
    // "vivaldi" is pinned, "firefox" is not — demote("firefox") should bail
    let pins = vec!["vivaldi".to_string()];
    let freezes = BTreeMap::new();

    let err = validate_demote_preconditions("firefox", &pins, &freezes)
        .unwrap_err()
        .to_string();
    assert!(err.contains("not currently pinned"), "{err}");
}
