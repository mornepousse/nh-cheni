use super::*;

#[test]
fn strips_a_single_store_path_in_a_building_line() {
    assert_eq!(
        prettify_line("building '/nix/store/5fwyagyxlc0vpa3ps74lyjn5bqjqd6pg-linux-7.0-modules-shrunk.drv'..."),
        "building 'linux-7.0-modules-shrunk.drv'..."
    );
}

#[test]
fn strips_multiple_store_paths_in_the_same_line() {
    let raw = "copying from /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-src to /nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-out";
    assert_eq!(prettify_line(raw), "copying from src to out");
}

#[test]
fn strips_path_with_nested_subpath_keeping_the_suffix() {
    let raw = "copying dependency: /nix/store/wa6czixakx46s4babflfgfh2dcl72q0s-linux-7.0-modules/lib/modules/7.0.0/foo.ko.xz";
    assert_eq!(
        prettify_line(raw),
        "copying dependency: linux-7.0-modules/lib/modules/7.0.0/foo.ko.xz"
    );
}

#[test]
fn strips_error_line_referencing_a_store_path() {
    let raw = "error: Cannot build '/nix/store/5fwyagyxlc0vpa3ps74lyjn5bqjqd6pg-linux-7.0-modules-shrunk.drv'.";
    assert_eq!(
        prettify_line(raw),
        "error: Cannot build 'linux-7.0-modules-shrunk.drv'."
    );
}

#[test]
fn passes_through_lines_without_store_paths() {
    assert_eq!(prettify_line(""), "");
    assert_eq!(prettify_line("root module: aes"), "root module: aes");
    assert_eq!(
        prettify_line("modprobe: FATAL: Module aes_generic not found"),
        "modprobe: FATAL: Module aes_generic not found"
    );
}

#[test]
fn does_not_strip_shorter_hash_lookalikes() {
    // 16-char strings after /nix/store/ are too short to match — we
    // only strip the full 32-char nix hash shape.
    let raw = "/nix/store/abc123-short";
    assert_eq!(prettify_line(raw), raw);
}

#[test]
fn is_idempotent() {
    let raw = "building '/nix/store/5fwyagyxlc0vpa3ps74lyjn5bqjqd6pg-foo.drv'";
    let once = prettify_line(raw);
    let twice = prettify_line(&once);
    assert_eq!(once, twice);
}
