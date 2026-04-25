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
fn test_parse_switch_inhibitor_extracts_change_line() {
    // Real activation refusal from `nh os switch` when dbus is
    // moving to dbus-broker. The detected change line must surface
    // verbatim in `what` so the user reads exactly which critical
    // bit is moving.
    let stderr = [
        "Checking switch inhibitors...",
        "There are changes to critical components of the system:",
        "",
        "dbus-implementation : dbus -> broker",
        "",
        "Switching into this system is not recommended.",
        "You probably want to run 'nixos-rebuild boot' and reboot your system instead.",
        "",
        "Pre-switch check 'switchInhibitors' failed",
        "Pre-switch checks failed",
    ]
    .join("\n");
    let errors = parse_errors(&stderr);
    assert!(
        errors
            .iter()
            .any(|e| e.category == "Pre-switch check"
                && e.what.contains("dbus-implementation")
                && e.what.contains("dbus -> broker")),
        "expected a Pre-switch check entry mentioning the dbus-implementation change, got: {:?}",
        errors
    );
    let entry = errors
        .iter()
        .find(|e| e.category == "Pre-switch check")
        .unwrap();
    assert!(
        entry.hint.as_deref().unwrap_or("").contains("nh os boot"),
        "hint should point at `nh os boot`, got: {:?}",
        entry.hint
    );
}

#[test]
fn test_parse_switch_inhibitor_falls_back_to_generic_label() {
    // When the change-line couldn't be parsed (truncated log, weird
    // formatting), the entry still fires with a placeholder `what`
    // — better than missing the diagnosis entirely.
    let stderr = [
        "some unrelated log line",
        "Pre-switch check 'someOtherCheck' failed",
        "Pre-switch checks failed",
    ]
    .join("\n");
    let errors = parse_errors(&stderr);
    let entry = errors
        .iter()
        .find(|e| e.category == "Pre-switch check");
    assert!(entry.is_some(), "expected a Pre-switch check entry, got: {:?}", errors);
    assert_eq!(entry.unwrap().what, "critical component change");
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
fn test_parse_insecure_single_line() {
    // Compact nixpkgs phrasing: package name + "marked as insecure"
    // on the same error line.
    let lines = [
        "error: Package 'qtwebengine-5.15.19' is marked as insecure, refusing to evaluate.",
    ];
    let errors = parse_errors(&lines.join("\n"));
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].category, "Insecure package");
    assert_eq!(errors[0].what, "qtwebengine-5.15.19");
    let hint = errors[0].hint.as_deref().unwrap_or("");
    assert!(hint.contains("permittedInsecurePackages"), "hint: {hint}");
    assert!(hint.contains("qtwebengine-5.15.19"), "hint: {hint}");
}

#[test]
fn test_parse_insecure_multiline_refusal() {
    // Exact phrasing from a real nh failure (v0.4.1 reproduction):
    // the "Refusing to evaluate package 'PKG'" line is a few lines
    // above the "marked as insecure" body.
    let stderr = "\
error: Refusing to evaluate package 'qtwebengine-5.15.19' in /nix/store/abc-source/pkgs/development/libraries/qt-5/modules/qtwebengine.nix:448 because it is marked as insecure

Known issues:
 - qt5 qtwebengine is unmaintained upstream since april 2025.";
    let errors = parse_errors(stderr);
    assert!(
        errors.iter().any(|e| e.category == "Insecure package"
            && e.what == "qtwebengine-5.15.19"),
        "expected insecure qtwebengine-5.15.19, got {:?}",
        errors
    );
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

// --- cap_names (active policy formatter) ---

fn names(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

#[test]
fn cap_names_returns_full_list_when_under_cap() {
    let v = names(&["a", "b", "c"]);
    assert_eq!(cap_names(&v, 5), "a, b, c");
}

#[test]
fn cap_names_returns_full_list_at_exactly_cap() {
    let v = names(&["a", "b", "c", "d", "e"]);
    assert_eq!(cap_names(&v, 5), "a, b, c, d, e");
}

#[test]
fn cap_names_truncates_with_overflow_marker_past_cap() {
    let v = names(&["a", "b", "c", "d", "e", "f", "g"]);
    assert_eq!(cap_names(&v, 3), "a, b, c (+4 more)");
}

#[test]
fn cap_names_handles_empty_slice() {
    let v: Vec<String> = Vec::new();
    assert_eq!(cap_names(&v, 5), "");
}
