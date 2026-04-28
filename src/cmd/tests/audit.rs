//! Tests for `cmd::audit`.

use super::*;
use std::path::PathBuf;

fn empty_report() -> AuditReport {
    AuditReport {
        health: HealthReport::default(),
        updates: UpdatesReport::default(),
        state: StateReport {
            pins_count: 0,
            freezes_count: 0,
            flake_dir: PathBuf::from("/tmp/fake-flake"),
        },
        verdict: AuditVerdict::Clear,
        next_action: None,
    }
}

#[test]
fn audit_report_serialises_to_json() {
    let report = empty_report();
    let json = serde_json::to_string(&report).expect("serialise");
    assert!(json.contains("\"verdict\":\"clear\""));
    assert!(json.contains("\"passed\":0"));
}

#[test]
fn verdict_clear_when_no_issues_and_no_updates() {
    let report = empty_report();
    assert_eq!(compute_verdict(&report.health, &report.updates), AuditVerdict::Clear);
}

#[test]
fn verdict_warnings_on_minor_update() {
    let mut report = empty_report();
    report.updates.minor = 1;
    assert_eq!(compute_verdict(&report.health, &report.updates), AuditVerdict::Warnings);
}

#[test]
fn verdict_warnings_on_health_warning() {
    let mut report = empty_report();
    report.health.warnings.push(HealthIssue {
        name: "stale input".into(),
        message: "...".into(),
        hint: None,
    });
    assert_eq!(compute_verdict(&report.health, &report.updates), AuditVerdict::Warnings);
}

#[test]
fn verdict_errors_on_health_error() {
    let mut report = empty_report();
    report.health.errors.push(HealthIssue {
        name: "not init".into(),
        message: "...".into(),
        hint: None,
    });
    assert_eq!(compute_verdict(&report.health, &report.updates), AuditVerdict::Errors);
}

#[test]
fn next_action_points_at_health_error_first() {
    let mut report = empty_report();
    report.health.errors.push(HealthIssue {
        name: "flake.lock dirty".into(),
        message: "...".into(),
        hint: None,
    });
    report.verdict = AuditVerdict::Errors;
    let action = compute_next_action(&report);
    assert!(action.unwrap().contains("flake.lock dirty"));
}

#[test]
fn next_action_suggests_upgrade_on_flake_input_update() {
    let mut report = empty_report();
    report.updates.flake_inputs_with_update.push(FlakeInputUpdate {
        name: "claude-code".into(),
        current: Some("2.1.119".into()),
        latest_remote_date: Some("2026-04-28".into()),
    });
    report.verdict = AuditVerdict::Warnings;
    let action = compute_next_action(&report);
    assert!(action.unwrap().contains("cheni upgrade"));
}

#[test]
fn next_action_none_on_clear() {
    let report = empty_report();
    assert!(compute_next_action(&report).is_none());
}
