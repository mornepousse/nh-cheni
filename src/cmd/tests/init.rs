use super::*;

#[test]
fn find_nixpkgs_insert_line_single_line_form() {
    // Classic one-line nixpkgs input — the function returns that line
    // itself, since the declaration ends on the same row.
    let content = r#"{
  description = "demo";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    home-manager.url = "github:nix-community/home-manager";
  };
}
"#;
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(find_nixpkgs_insert_line(&lines), Some(3));
}

#[test]
fn find_nixpkgs_insert_line_multi_line_form() {
    // Multi-line block — returns the closing `};` line so the new input
    // lands *after* the whole nested declaration, never inside it.
    let content = r#"{
  inputs = {
    nixpkgs = {
      url = "github:NixOS/nixpkgs/nixos-unstable";
    };
  };
}
"#;
    let lines: Vec<&str> = content.lines().collect();
    // Line 4 (0-indexed) is the `};` that closes the nixpkgs block.
    assert_eq!(find_nixpkgs_insert_line(&lines), Some(4));
}

#[test]
fn find_nixpkgs_insert_line_no_nixpkgs_returns_none() {
    // A flake without any nixpkgs input — caller converts this to a
    // MANUAL fallback with printed instructions.
    let content = r#"{
  inputs = {
    home-manager.url = "github:nix-community/home-manager";
  };
}
"#;
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(find_nixpkgs_insert_line(&lines), None);
}

#[test]
fn build_content_with_latest_input_inserts_after_line() {
    let lines = vec!["before", "TARGET", "after"];
    let out = build_content_with_latest_input(&lines, 1);
    // The new input is inserted after line 1 (0-indexed), with a blank
    // spacer line, then comment, then the url line.
    let got: Vec<&str> = out.lines().collect();
    assert_eq!(got[0], "before");
    assert_eq!(got[1], "TARGET");
    assert_eq!(got[2], ""); // blank spacer
    assert!(got[3].contains("# nixpkgs-latest"));
    assert!(got[4].contains("nixpkgs-latest.url"));
    assert_eq!(got[5], "after");
}

#[test]
fn build_content_with_latest_input_preserves_trailing_newline() {
    let lines = vec!["a", "b"];
    let out = build_content_with_latest_input(&lines, 0);
    assert!(out.ends_with('\n'), "must end with a newline (POSIX text)");
}
