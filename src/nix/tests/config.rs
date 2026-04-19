use super::*;

#[test]
fn nix_keywords_detected() {
    assert!(is_nix_keyword("enable"));
    assert!(is_nix_keyword("pkgs"));
    assert!(is_nix_keyword("mkDerivation"));
}

#[test]
fn package_names_not_keywords() {
    assert!(!is_nix_keyword("firefox"));
    assert!(!is_nix_keyword("legcord"));
    assert!(!is_nix_keyword("kicad"));
}

#[test]
fn parse_imports_single_block() {
    let content = r#"{
  imports = [
    ./modules/desktop/hyprland.nix
    ./modules/dev/stm32.nix
    ./hosts/morthinkpad/default.nix
  ];
}"#;
    let imports = parse_imports(content);
    assert_eq!(imports.len(), 3);
    assert!(imports.contains(&"./modules/desktop/hyprland.nix".to_string()));
    assert!(imports.contains(&"./modules/dev/stm32.nix".to_string()));
    assert!(imports.contains(&"./hosts/morthinkpad/default.nix".to_string()));
}

#[test]
fn parse_imports_strips_inline_comments() {
    // Inline `# foo` after the path must not be captured as part of the
    // path token — the trim happens before the ./-prefix check.
    let content = r#"{
  imports = [
    ./a.nix           # module A
    ./b.nix  # with a trailing comment
  ];
}"#;
    let imports = parse_imports(content);
    assert_eq!(imports, vec!["./a.nix", "./b.nix"]);
}

#[test]
fn parse_imports_ignores_inputs_and_bare_identifiers() {
    // Only `./`/`../` tokens are relative paths; bare identifiers
    // (e.g. `inputs.cheni.nixosModules.default`) must be skipped.
    let content = r#"{
  imports = [
    inputs.cheni.nixosModules.default
    ./local.nix
    ../shared.nix
  ];
}"#;
    let imports = parse_imports(content);
    assert!(imports.contains(&"./local.nix".to_string()));
    assert!(imports.contains(&"../shared.nix".to_string()));
    assert!(!imports.iter().any(|s| s.contains("inputs")));
}

#[test]
fn parse_imports_multiple_blocks() {
    // Flakes with specialisations sometimes have two imports lists.
    let content = r#"{
  imports = [ ./a.nix ];
  specialisations.foo = {
    imports = [ ./b.nix ];
  };
}"#;
    let imports = parse_imports(content);
    assert_eq!(imports.len(), 2);
    assert!(imports.contains(&"./a.nix".to_string()));
    assert!(imports.contains(&"./b.nix".to_string()));
}

#[test]
fn parse_imports_handles_with_lib_prefix() {
    // `imports = with lib; [...]` is a less common but valid pattern.
    let content = r#"{
  imports = with lib; [
    ./first.nix
    ./second.nix
  ];
}"#;
    let imports = parse_imports(content);
    assert_eq!(imports, vec!["./first.nix", "./second.nix"]);
}

#[test]
fn parse_imports_empty_when_no_block() {
    let content = r#"{ config, pkgs, ... }: {
  environment.systemPackages = [ pkgs.firefox ];
}"#;
    assert!(parse_imports(content).is_empty());
}

#[test]
fn parse_imports_strips_trailing_semicolons() {
    // Users sometimes put the `;` on the same line as the last path —
    // the token comes in as "./foo.nix;" which must lose the semicolon.
    let content = r#"{
  imports = [ ./foo.nix ];
}"#;
    let imports = parse_imports(content);
    assert_eq!(imports, vec!["./foo.nix"]);
}
