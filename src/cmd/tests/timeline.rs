//! Tests for `cmd::timeline`.

use super::*;
use crate::nix::timeline::Event;

fn make_event(kind: &str, package: Option<&str>) -> Event {
    Event {
        ts: "2026-04-28T11:00:00Z".to_string(),
        kind: kind.to_string(),
        package: package.map(|s| s.to_string()),
        details: serde_json::json!({}),
    }
}

#[test]
fn parse_since_handles_d_h_m() {
    assert_eq!(parse_since_duration_secs("7d").unwrap(), 7 * 86_400);
    assert_eq!(parse_since_duration_secs("1h").unwrap(), 3600);
    assert_eq!(parse_since_duration_secs("30m").unwrap(), 1800);
}

#[test]
fn parse_since_bails_on_missing_unit() {
    let err = parse_since_duration_secs("7").expect_err("missing unit");
    assert!(format!("{err}").contains("Need a unit"));
}

#[test]
fn parse_since_bails_on_unknown_unit() {
    let err = parse_since_duration_secs("7y").expect_err("unknown unit");
    assert!(format!("{err}").contains("unknown duration unit"));
}

#[test]
fn match_filters_passes_when_no_filters() {
    let e = make_event("pin", Some("firefox"));
    assert!(match_filters(&e, None, None, None));
}

#[test]
fn match_filters_filters_by_package() {
    let e = make_event("pin", Some("firefox"));
    assert!(match_filters(&e, Some("firefox"), None, None));
    assert!(!match_filters(&e, Some("kicad"), None, None));
}

#[test]
fn match_filters_filters_by_kind() {
    let e = make_event("pin", Some("firefox"));
    assert!(match_filters(&e, None, Some("pin"), None));
    assert!(!match_filters(&e, None, Some("freeze"), None));
}

#[test]
fn match_filters_combines_filters_with_and() {
    let e = make_event("pin", Some("firefox"));
    // matches pkg AND kind
    assert!(match_filters(&e, Some("firefox"), Some("pin"), None));
    // pkg matches, kind doesn't
    assert!(!match_filters(&e, Some("firefox"), Some("freeze"), None));
}
