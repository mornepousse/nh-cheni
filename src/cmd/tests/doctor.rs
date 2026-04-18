use super::*;

fn sev(size: &str) -> Severity {
    classify_store_size(size).severity
}

#[test]
fn classify_small_store() {
    assert_eq!(sev("5.2G"), Severity::Ok);
    assert_eq!(sev("48G"), Severity::Ok);
    assert_eq!(sev("800M"), Severity::Ok);
}

#[test]
fn classify_large_store() {
    assert_eq!(sev("76G"), Severity::Warning);
    assert_eq!(sev("51G"), Severity::Warning);
    assert_eq!(sev("1.2T"), Severity::Warning);
    // Case-insensitive unit suffix
    assert_eq!(sev("100g"), Severity::Warning);
}

#[test]
fn classify_unparseable() {
    // Unknown unit or garbage → Ok (no warning), caller just shows it raw.
    assert_eq!(sev("?"), Severity::Ok);
    assert_eq!(sev("unknown"), Severity::Ok);
}
