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

#[test]
fn create_freezes_file_writes_empty_object() {
    // First-run seed: a `{}` file so the Nix overlay finds it without
    // the `pathExists` branch having to kick in, and `cheni freeze`
    // doesn't have to create it on first invocation.
    let dir = tempfile::tempdir().unwrap();
    let created = create_freezes_file(dir.path()).unwrap();
    assert!(created);

    let contents = std::fs::read_to_string(dir.path().join("package-freezes.json")).unwrap();
    assert_eq!(contents, "{}\n");
}

#[test]
fn create_freezes_file_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    // Pre-existing file with user content must not be overwritten.
    std::fs::write(
        dir.path().join("package-freezes.json"),
        r#"{"firefox":{"rev":"abc"}}"#,
    )
    .unwrap();

    let created = create_freezes_file(dir.path()).unwrap();
    assert!(!created, "shouldn't recreate an existing file");

    let contents = std::fs::read_to_string(dir.path().join("package-freezes.json")).unwrap();
    assert!(
        contents.contains("firefox"),
        "user content must be preserved, got: {}",
        contents
    );
}

#[test]
fn add_freeze_overlay_inserts_after_overlay_bracket() {
    // Fixture flake with a minimal overlay list — `add_freeze_overlay`
    // should splice the freeze overlay right after the opening `[`.
    let flake = r#"{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };
  outputs = { self, nixpkgs }: {
    nixosConfigurations.demo = nixpkgs.lib.nixosSystem {
      modules = [
        ({ ... }: {
          nixpkgs.overlays = [
          ];
        })
      ];
    };
  };
}
"#;
    let dir = tempfile::tempdir().unwrap();
    let flake_path = dir.path().join("flake.nix");
    std::fs::write(&flake_path, flake).unwrap();

    add_freeze_overlay(&flake_path, flake).unwrap();

    let modified = std::fs::read_to_string(&flake_path).unwrap();
    assert!(
        modified.contains("package-freezes.json"),
        "marker must be present so re-run is idempotent"
    );
    assert!(
        modified.contains("builtins.fetchTree"),
        "overlay body must reference fetchTree: {}",
        modified
    );
    assert!(
        modified.contains("nixpkgs.overlays = ["),
        "original overlay-list bracket must still be there"
    );
}

#[test]
fn add_freeze_overlay_fails_without_overlay_list() {
    // A flake that doesn't declare `nixpkgs.overlays = [` can't be
    // auto-modified; the caller converts this into MANUAL instructions.
    let flake = r#"{
  outputs = { self, nixpkgs }: {
    nixosConfigurations.demo = { };
  };
}
"#;
    let dir = tempfile::tempdir().unwrap();
    let flake_path = dir.path().join("flake.nix");
    std::fs::write(&flake_path, flake).unwrap();

    let err = add_freeze_overlay(&flake_path, flake).unwrap_err();
    assert!(format!("{:#}", err).contains("nixpkgs.overlays"));
}
