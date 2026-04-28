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
