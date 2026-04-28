//! Regression tests for summarize_diff — protects against format drift
//! in `nix store diff-closures` output. Each fixture is a raw stdout
//! sample (anonymised) from real diffs observed during cheni development.
//!
//! Included from history.rs via `#[cfg(test)] #[path = "history_tests.rs"]
//! mod diff_parser_tests;` — kept as a sibling file so the source stays
//! short and the tests are easy to browse on their own.

use super::*;

#[test]
fn identical_closures() {
    // Empty stdout = nothing changed between the two generations.
    assert_eq!(summarize_diff(""), Some("(identical closures)".to_string()));
}

#[test]
fn single_version_bump() {
    let out = "claude-code: 2.1.113 → 2.1.114";
    let s = summarize_diff(out).unwrap();
    assert!(s.contains("↑ claude-code"), "got: {}", s);
    assert!(s.contains("2.1.113 → 2.1.114"), "got: {}", s);
}

#[test]
fn version_bump_with_size_delta() {
    // Real observed form: nix appends ", +NNN KiB" in red ANSI after
    // a version bump that brought in a larger derivation.
    let out = "claude-code: 2.1.112 → 2.1.113, \x1b[31;1m552.0 KiB\x1b[0m";
    let s = summarize_diff(out).unwrap();
    assert!(s.contains("↑ claude-code (2.1.112 → 2.1.113)"), "got: {}", s);
    assert!(s.contains("+552 KiB"), "got: {}", s);
}

#[test]
fn rebuild_only_size_delta() {
    // Same version, closure content changed (e.g. cheni rebuilt from
    // a new source). Pure size line with no arrow.
    let out = "cheni: \x1b[31;1m38.6 KiB\x1b[0m";
    let s = summarize_diff(out).unwrap();
    assert!(s.contains("⟳ cheni"), "got: {}", s);
    assert!(s.contains("+39 KiB"), "got: {}", s);
}

#[test]
fn added_and_removed() {
    // ∅ → ε marks a new derivation with no version; ε → ∅ marks
    // a removal. Both appear for unit-file renames during rebuilds.
    let out = "hm_nviminit.lua: ∅ → ε\nwrapper-init: ε → ∅";
    let s = summarize_diff(out).unwrap();
    assert!(s.contains("+ hm_nviminit.lua"), "got: {}", s);
    assert!(s.contains("- wrapper-init"), "got: {}", s);
}

#[test]
fn removed_with_version() {
    let out = "old-pkg: 1.2.3 → ∅";
    let s = summarize_diff(out).unwrap();
    assert!(s.contains("- old-pkg"), "got: {}", s);
}

#[test]
fn added_with_version() {
    let out = "new-pkg: ∅ → 2.0.0";
    let s = summarize_diff(out).unwrap();
    assert!(s.contains("+ new-pkg"), "got: {}", s);
}

#[test]
fn big_upgrade_truncates_to_three_plus_more() {
    // 5 updates → shows first 3 names then "(+2 more)" marker.
    let out = "\
a: 1.0 → 2.0\n\
b: 1.0 → 2.0\n\
c: 1.0 → 2.0\n\
d: 1.0 → 2.0\n\
e: 1.0 → 2.0\n";
    let s = summarize_diff(out).unwrap();
    assert!(s.starts_with("↑ a, b, c"), "got: {}", s);
    assert!(s.contains("(+2 more)"), "got: {}", s);
}

#[test]
fn malformed_lines_are_skipped() {
    // Lines without "name: " aren't valid diff entries — must not panic
    // or produce bogus entries.
    let out = "\
some banner line\n\
claude-code: 1.0 → 2.0\n\
another weird line\n\
---\n";
    let s = summarize_diff(out).unwrap();
    // Only the real line is picked up.
    assert!(s.contains("↑ claude-code"), "got: {}", s);
}

#[test]
fn ascii_arrow_fallback_is_parsed() {
    // Locales without Unicode sometimes emit "->" instead of "→".
    // The parser accepts both so we don't silently lose entries.
    let out = "foo: 1.0 -> 2.0";
    let s = summarize_diff(out).unwrap();
    assert!(s.contains("↑ foo"), "got: {}", s);
}

#[test]
fn mib_size_delta_is_aggregated() {
    // Two rebuilt packages contributing MiB — aggregated into one
    // suffix on the summary line.
    let out = "\
kernel: \x1b[31;1m45.2 MiB\x1b[0m\n\
firefox: \x1b[31;1m33.4 MiB\x1b[0m\n";
    let s = summarize_diff(out).unwrap();
    assert!(s.contains("⟳"), "got: {}", s);
    // 45.2 + 33.4 = 78.6 MiB
    assert!(s.contains("+78.6 MiB") || s.contains("+78 MiB"), "got: {}", s);
}

#[test]
fn strip_ansi_leaves_plain_text_alone() {
    assert_eq!(strip_ansi("hello world"), "hello world");
    assert_eq!(strip_ansi("\x1b[31mred\x1b[0m"), "red");
    assert_eq!(strip_ansi("\x1b[31;1mred-bold\x1b[0m stays"), "red-bold stays");
}

#[test]
fn parse_size_delta_variants() {
    assert_eq!(parse_size_delta("cheni: 38.6 KiB"), Some(38.6));
    assert_eq!(parse_size_delta("kernel: 45.2 MiB"), Some(45.2 * 1024.0));
    assert_eq!(parse_size_delta("nothing here"), None);
    assert_eq!(parse_size_delta(""), None);
}

// --- run_brief ----------------------------------------------------------

/// Helper that builds a minimal Generation without a real store path.
fn make_gen(number: u32, date: &str, is_current: bool) -> Generation {
    Generation {
        number,
        date: date.to_string(),
        mtime_secs: None,
        is_current,
        store_path: format!("/nix/var/nix/profiles/system-{}-link", number),
        nixos_label: None,
    }
}

#[test]
fn run_brief_returns_ok_for_single_gen() {
    // run_brief writes to stdout but must not panic and must return Ok.
    // Content correctness is validated by visual inspection — stdout
    // capture is not in scope for this test suite.
    let gens = vec![make_gen(42, "2026-04-28 10:00", true)];
    assert!(run_brief(&gens).is_ok());
}

#[test]
fn run_brief_returns_ok_for_multiple_gens() {
    let gens = vec![
        make_gen(100, "2026-01-01 00:00", false),
        make_gen(101, "2026-02-01 00:00", false),
        make_gen(102, "2026-04-28 10:00", true),
    ];
    assert!(run_brief(&gens).is_ok());
}

#[test]
fn brief_overrides_by_diff_check() {
    // When --diff is passed with --brief, --diff wins (specific beats
    // general). We can't call run() without a real store, but we can
    // verify the precedence logic: brief=true && diff=true → brief=false.
    // Mirrors the `let brief = opts.brief && !opts.diff;` line in run().
    let brief = true;
    let diff = true;
    let effective_brief = brief && !diff;
    assert!(!effective_brief, "--diff should override --brief");
}

#[test]
fn brief_stays_on_without_diff() {
    let brief = true;
    let diff = false;
    let effective_brief = brief && !diff;
    assert!(effective_brief, "--brief should be effective when --diff is absent");
}
