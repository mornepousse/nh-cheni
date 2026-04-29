//! Tests for `nix::timeline`.

use super::*;

#[test]
fn event_serialises_to_jsonl() {
    let event = Event {
        ts: "2026-04-28T11:00:00Z".to_string(),
        kind: "pin".to_string(),
        package: Some("firefox".to_string()),
        details: serde_json::json!({}),
    };
    let line = serde_json::to_string(&event).expect("serialise");
    assert!(line.contains("\"kind\":\"pin\""));
    assert!(line.contains("\"package\":\"firefox\""));
    assert!(line.contains("\"ts\":\"2026-04-28T11:00:00Z\""));
}

#[test]
fn event_round_trips() {
    let event = Event {
        ts: "2026-04-28T11:00:00Z".to_string(),
        kind: "promote".to_string(),
        package: Some("kicad".to_string()),
        details: serde_json::json!({"from": "freeze", "to": "pin"}),
    };
    let line = serde_json::to_string(&event).expect("serialise");
    let parsed: Event = serde_json::from_str(&line).expect("parse");
    assert_eq!(parsed.ts, event.ts);
    assert_eq!(parsed.kind, event.kind);
    assert_eq!(parsed.package, event.package);
    assert_eq!(parsed.details, event.details);
}

#[test]
fn event_with_no_package_round_trips() {
    let event = Event {
        ts: "2026-04-28T11:00:00Z".to_string(),
        kind: "upgrade".to_string(),
        package: None,
        details: serde_json::json!({"outcome": "success"}),
    };
    let line = serde_json::to_string(&event).expect("serialise");
    let parsed: Event = serde_json::from_str(&line).expect("parse");
    assert!(parsed.package.is_none());
}

#[test]
fn format_rfc3339_epoch_is_1970() {
    assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
}

#[test]
fn format_rfc3339_known_date() {
    // 2026-04-28T11:00:00Z = 1777374000
    assert_eq!(format_rfc3339(1777374000), "2026-04-28T11:00:00Z");
}
