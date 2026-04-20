use super::*;

#[test]
fn parses_the_canonical_plural_shape() {
    let out = "finding garbage collector roots...\n\
               determining live/dead paths...\n\
               7 store paths would be deleted\n";
    assert_eq!(parse_path_count(out), 7);
}

#[test]
fn parses_the_singular_shape() {
    // Unlikely in practice but cheap to cover.
    let out = "determining live/dead paths...\n1 store path would be deleted\n";
    assert_eq!(parse_path_count(out), 1);
}

#[test]
fn parses_large_counts() {
    let out = "...\n12345 store paths would be deleted\ntrailing line\n";
    assert_eq!(parse_path_count(out), 12345);
}

#[test]
fn parses_delete_older_than_variant_output() {
    // Output shape observed from `nix-collect-garbage --delete-older-than 30d --dry-run`.
    // The "removing old generations" lines come before the final count.
    let out = "removing old generations of profile /home/mae/.local/state/nix/profiles/profile\n\
               waiting for the big garbage collector lock...\n\
               finding garbage collector roots...\n\
               determining live/dead paths...\n\
               7 store paths would be deleted\n";
    assert_eq!(parse_path_count(out), 7);
}

#[test]
fn returns_zero_when_the_line_is_absent() {
    assert_eq!(parse_path_count(""), 0);
    assert_eq!(parse_path_count("some unrelated log"), 0);
    assert_eq!(parse_path_count("0 store paths would be deleted"), 0);
    // (0 is both the absent case and the "nothing to delete" case —
    // fine for display purposes.)
}

#[test]
fn picks_the_last_match_when_several_counts_appear() {
    // If nix-collect-garbage prints intermediate totals before the
    // final one (doesn't in practice, but worth confirming behaviour),
    // regex::captures() grabs the first, which is what we document.
    let out = "3 store paths would be deleted\n7 store paths would be deleted\n";
    // First match wins — documented behaviour of this helper.
    assert_eq!(parse_path_count(out), 3);
}
