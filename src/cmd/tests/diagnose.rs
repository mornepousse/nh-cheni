use super::*;

#[test]
fn empty_log_yields_no_findings() {
    assert!(find_issues("").is_empty());
}

#[test]
fn unrelated_log_yields_no_findings() {
    let log = "Configuration built successfully.\nActivating...\nDone.";
    assert!(find_issues(log).is_empty());
}

#[test]
fn detects_aes_generic_kernel_issue() {
    let log = "root module: aes_generic\n\
               modprobe: FATAL: Module aes_generic not found in directory /nix/store/.../7.0.0\n\
               error: Failed to build linux-7.0-modules-shrunk.drv";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("aes_generic"));
}

#[test]
fn detects_hash_mismatch() {
    let log = "error: hash mismatch in fixed-output derivation \
               '/nix/store/...drv':\n  expected: sha256-AAA=\n  got: sha256-BBB=";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("hash mismatch"));
}

#[test]
fn detects_disk_full() {
    let log = "error: writing to file: No space left on device";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("disk full"));
}

#[test]
fn detects_missing_flake_attribute() {
    let log = "error: flake 'gitlab:foo/bar' does not provide attribute \
               'packages.x86_64-linux.default'";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("attribute"));
}

#[test]
fn detects_infinite_recursion() {
    let log = "error: infinite recursion encountered\n  \
               at /nix/store/.../default.nix:12:5";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("infinite recursion"));
}

#[test]
fn multiple_issues_in_one_log_all_reported() {
    let log = "error: hash mismatch in fixed-output derivation '/nix/store/...drv'\n\
               ... later ...\n\
               error: writing to file: No space left on device";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 2);
}

#[test]
fn match_is_case_insensitive() {
    let log = "NO SPACE LEFT ON DEVICE somewhere in loud output";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
}

#[test]
fn duplicate_pattern_in_log_reports_once() {
    // A pattern can appear many times in a long log (e.g. retries).
    // We only care that at least one occurrence is surfaced.
    let log = "aes_generic not found\naes_generic not found\naes_generic not found";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
}
