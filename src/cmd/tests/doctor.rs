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

#[test]
fn is_hex_rev_accepts_full_and_short_revs() {
    assert!(is_hex_rev("abcdef0123456789abcdef0123456789abcdef01"));
    assert!(is_hex_rev("abcdef1")); // 7-char short rev (git's default minimum)
}

#[test]
fn is_hex_rev_rejects_non_hex_and_bad_lengths() {
    assert!(!is_hex_rev("abcdeXY")); // non-hex chars
    assert!(!is_hex_rev("abc")); // too short
    assert!(!is_hex_rev("")); // empty
    assert!(!is_hex_rev(&"a".repeat(65))); // too long
}

#[test]
fn is_sri_hash_accepts_sha256_and_sha512() {
    assert!(is_sri_hash(
        "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
    ));
    assert!(is_sri_hash(
        "sha512-BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=="
    ));
}

#[test]
fn is_sri_hash_rejects_non_sri_and_injection() {
    assert!(!is_sri_hash("abc")); // no sha prefix
    assert!(!is_sri_hash("md5-whatever")); // wrong alg
    assert!(!is_sri_hash("sha256-AAA\"BBB")); // quote injection
    assert!(!is_sri_hash("sha256-AAA\nBBB")); // control char
    assert!(!is_sri_hash(&format!("sha256-{}", "A".repeat(250)))); // way too long
}
