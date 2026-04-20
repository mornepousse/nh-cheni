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

#[test]
fn detects_unfree_package_refusal() {
    let log = "error: Package 'steam-1.0.0.82' has an unfree license ('unfree'), \
               refusing to evaluate.";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("unfree"));
}

#[test]
fn detects_broken_package_error() {
    let log = "error: Package 'discord-0.0.85' in /nix/store/.../default.nix:123 \
               is marked as broken, refusing to evaluate.";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("broken"));
}

#[test]
fn detects_package_collision() {
    let log = "error: collision between `/nix/store/AAA-foo-1.0/bin/foo' \
               and `/nix/store/BBB-bar-2.0/bin/foo'";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("collision"));
}

#[test]
fn detects_pure_eval_absolute_path_refusal() {
    let log = "error: access to absolute path '/home/user/secrets.nix' \
               is forbidden in pure eval mode (use '--impure' to override)";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("pure eval mode"));
}

#[test]
fn detects_file_not_in_git_tree() {
    let log = "warning: Git tree '/home/user/nixos-config' is dirty\n\
               error: path '/nix/store/...-source/modules/new.nix' \
               does not exist in the flake";
    let hits = find_issues(log);
    assert_eq!(hits.len(), 1);
    assert!(hits[0].title.contains("tracked by git"));
}
