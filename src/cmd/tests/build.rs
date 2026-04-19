use super::*;

#[test]
fn test_extract_hash() {
    assert_eq!(
        extract_hash("  got: sha256-abc123def456"),
        Some("sha256-abc123def456".to_string())
    );
    assert_eq!(
        extract_hash("no hash here"),
        None
    );
}

#[test]
fn test_extract_pkg_from_drv() {
    assert_eq!(
        extract_pkg_from_drv("builder for '/nix/store/abc12345678901234567890123456789-vivaldi-7.9.drv' failed"),
        Some("vivaldi-7.9".to_string())
    );
}

#[test]
fn test_parse_hash_mismatch() {
    let lines = [
        "error: hash mismatch in fixed-output derivation",
        "  specified: sha256-aaaa",
        "  got: sha256-bbbb",
    ];
    let errors = parse_errors(&lines.join("\n"));
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].category, "Hash mismatch");
}

#[test]
fn test_parse_unfree() {
    let lines = [
        "error: Package 'nvidia-x11' is not free and refused to install.",
    ];
    let errors = parse_errors(&lines.join("\n"));
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].category, "Unfree package");
    assert_eq!(errors[0].what, "nvidia-x11");
}

#[test]
fn test_parse_broken() {
    let lines = [
        "error: Package 'python3.11-some-pkg' is marked as broken.",
    ];
    let errors = parse_errors(&lines.join("\n"));
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].category, "Broken package");
}

#[test]
fn test_parse_undefined_var() {
    let lines = [
        "at /home/mae/nixos-config/modules/dev/test.nix:5:3:",
        "error: undefined variable 'pkgss'",
    ];
    let errors = parse_errors(&lines.join("\n"));
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].category, "Undefined variable");
    assert_eq!(errors[0].what, "pkgss");
}

#[test]
fn test_parse_cargohash_out_of_date() {
    // Real nh output when a Cargo dep is added without bumping cargoHash.
    // This one was the reason cheni itself failed to self-update once.
    let stderr = "\
cheni> ERROR: cargoHash or cargoSha256 is out of date
cheni> Cargo.lock is not the same in /build/cheni-0.1.0-vendor";
    let errors = parse_errors(stderr);
    assert!(
        errors.iter().any(|e| e.category == "Hash mismatch"),
        "expected Hash mismatch, got {:?}",
        errors
    );
}

#[test]
fn test_parse_python_interpreter_mismatch() {
    let stderr = "error: sphinx-9.1.0 not supported for interpreter python3.13";
    let errors = parse_errors(stderr);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].category, "Incompatible package");
    assert_eq!(errors[0].what, "sphinx-9.1.0");
}

#[test]
fn test_parse_python_mismatch_deduplicates() {
    // nh can repeat the same error across several builder retries —
    // the parser should only emit one entry.
    let stderr = "\
error: sphinx-9.1.0 not supported for interpreter python3.13
... some noise ...
error: sphinx-9.1.0 not supported for interpreter python3.13";
    let errors = parse_errors(stderr);
    let dups = errors
        .iter()
        .filter(|e| e.category == "Incompatible package" && e.what == "sphinx-9.1.0")
        .count();
    assert_eq!(dups, 1, "expected dedup, got {:?}", errors);
}

#[test]
fn test_parse_multiple_errors() {
    // A rebuild can surface several independent errors — we should
    // collect them all, not just the first.
    let stderr = "\
error: undefined variable 'pkgss'
at /file.nix:1:1
error: Package 'mesa' is marked as broken.";
    let errors = parse_errors(stderr);
    assert!(errors.len() >= 2, "expected >=2, got {:?}", errors);
}

#[test]
fn test_parse_empty_stderr() {
    // Truly empty stderr → no errors. Must not panic or return bogus
    // "error: " entries from the generic fallback.
    let errors = parse_errors("");
    assert!(errors.is_empty());
}

#[test]
fn test_parse_generic_error_fallback() {
    // An error pattern we don't specifically recognise still shows up
    // as a generic entry so the user sees *something*, not silence.
    let errors = parse_errors("error: some novel upstream message we don't know yet");
    assert_eq!(errors.len(), 1);
    assert!(errors[0].message.contains("some novel"));
}

#[test]
fn test_parse_infinite_recursion() {
    let lines = [
        "error: infinite recursion encountered",
    ];
    let errors = parse_errors(&lines.join("\n"));
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].category, "Infinite recursion");
}

#[test]
fn test_parse_path_not_found() {
    let lines = [
        "error: path '/nix/store/abc-source/modules/test.nix' does not exist",
    ];
    let errors = parse_errors(&lines.join("\n"));
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].category, "File not found");
}

#[test]
fn test_parse_cargo_hash() {
    let lines = [
        "ERROR: cargoHash or cargoSha256 is out of date",
    ];
    let errors = parse_errors(&lines.join("\n"));
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].category, "Hash mismatch");
}

#[test]
fn test_no_errors() {
    let errors = parse_errors("everything is fine");
    assert!(errors.is_empty());
}
