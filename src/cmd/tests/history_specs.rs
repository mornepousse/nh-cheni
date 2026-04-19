//! Tests for the small parsers that back `cheni history --delete` /
//! `--older-than`. They're easy to break with bad input and the
//! consequences (deleting too many generations) are not recoverable
//! by `cheni rollback`, so the bar is high.

use super::*;

const GENS: &[u32] = &[400, 401, 402, 403, 404, 405];

// ── parse_target_spec ─────────────────────────────────────────

#[test]
fn target_single_existing_returns_one() {
    assert_eq!(parse_target_spec("402", GENS).unwrap(), vec![402]);
}

#[test]
fn target_single_missing_errors() {
    let err = parse_target_spec("999", GENS).unwrap_err();
    assert!(format!("{:#}", err).contains("does not exist"));
}

#[test]
fn target_range_inclusive() {
    assert_eq!(
        parse_target_spec("401..403", GENS).unwrap(),
        vec![401, 402, 403]
    );
}

#[test]
fn target_range_reverse_is_normalised() {
    // Same as 401..403 — user wrote the bounds in the other order.
    assert_eq!(
        parse_target_spec("403..401", GENS).unwrap(),
        vec![401, 402, 403]
    );
}

#[test]
fn target_empty_string_errors() {
    let err = parse_target_spec("", GENS).unwrap_err();
    let msg = format!("{:#}", err);
    assert!(msg.contains("Empty"), "got: {}", msg);
}

#[test]
fn target_whitespace_only_errors() {
    let err = parse_target_spec("   ", GENS).unwrap_err();
    assert!(format!("{:#}", err).contains("Empty"));
}

#[test]
fn target_range_missing_lower_bound_errors() {
    let err = parse_target_spec("..405", GENS).unwrap_err();
    assert!(format!("{:#}", err).contains("missing one bound"));
}

#[test]
fn target_range_missing_upper_bound_errors() {
    let err = parse_target_spec("400..", GENS).unwrap_err();
    assert!(format!("{:#}", err).contains("missing one bound"));
}

#[test]
fn target_range_with_no_match_errors() {
    // Previously silently returned empty Vec — now flags as error so
    // the user sees what happened.
    let err = parse_target_spec("100..200", GENS).unwrap_err();
    assert!(format!("{:#}", err).contains("matches no existing generation"));
}

#[test]
fn target_garbage_errors() {
    let err = parse_target_spec("not a number", GENS).unwrap_err();
    assert!(format!("{:#}", err).contains("Invalid generation number"));
}

#[test]
fn target_range_with_negative_errors() {
    // u32 can't be negative — minus sign means parse fails.
    let err = parse_target_spec("-5..5", GENS).unwrap_err();
    assert!(format!("{:#}", err).contains("Invalid range"));
}

// ── parse_duration_days ───────────────────────────────────────

#[test]
fn duration_days_default_unit() {
    assert_eq!(parse_duration_days("30").unwrap(), 30);
    assert_eq!(parse_duration_days("30d").unwrap(), 30);
}

#[test]
fn duration_weeks_months_years() {
    assert_eq!(parse_duration_days("2w").unwrap(), 14);
    assert_eq!(parse_duration_days("6m").unwrap(), 180);
    assert_eq!(parse_duration_days("1y").unwrap(), 365);
}

#[test]
fn duration_zero_rejected() {
    // Critical safeguard: 0d would match every generation.
    let err = parse_duration_days("0d").unwrap_err();
    let msg = format!("{:#}", err);
    assert!(msg.contains("zero duration"), "got: {}", msg);
    assert!(msg.contains("--keep"), "should suggest the alternative");
}

#[test]
fn duration_zero_no_unit_rejected() {
    let err = parse_duration_days("0").unwrap_err();
    assert!(format!("{:#}", err).contains("zero duration"));
}

#[test]
fn duration_empty_rejected() {
    let err = parse_duration_days("").unwrap_err();
    assert!(format!("{:#}", err).contains("Empty"));
}

#[test]
fn duration_unknown_unit_rejected() {
    let err = parse_duration_days("5x").unwrap_err();
    assert!(format!("{:#}", err).contains("Unknown time unit"));
}

#[test]
fn duration_garbage_rejected() {
    let err = parse_duration_days("banana").unwrap_err();
    assert!(format!("{:#}", err).contains("Expected a number"));
}

#[test]
fn duration_whitespace_trimmed() {
    assert_eq!(parse_duration_days(" 30d ").unwrap(), 30);
}

// ── pick_oldest_beyond ────────────────────────────────────────

#[test]
fn pick_oldest_keeps_n_most_recent() {
    // Input is sorted ascending; keep 3 most recent → drop the oldest 3.
    assert_eq!(pick_oldest_beyond(GENS, 3), vec![400, 401, 402]);
}

#[test]
fn pick_oldest_keep_all_returns_empty() {
    assert!(pick_oldest_beyond(GENS, GENS.len()).is_empty());
    assert!(pick_oldest_beyond(GENS, GENS.len() + 1).is_empty());
}

#[test]
fn pick_oldest_keep_zero_drops_all() {
    assert_eq!(pick_oldest_beyond(GENS, 0).len(), GENS.len());
}
