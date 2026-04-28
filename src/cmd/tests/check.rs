use super::*;
use crate::nix::freezes::{FreezeEntry, Freezes};

// ─── strip_inputs_block ───────────────────────────────────────────────────────

#[test]
fn strip_inputs_block_no_inputs_key_returns_original() {
    // Flake nix sans bloc inputs= du tout → le texte est retourné intact.
    let text = "{ outputs = { nixpkgs, ... }: { }; }";
    assert_eq!(strip_inputs_block(text), text);
}

#[test]
fn strip_inputs_block_removes_single_flat_inputs_block() {
    // Un bloc inputs = { nixpkgs.url = "…"; }; — le contenu doit
    // disparaître et la mention `inputs.nixpkgs` dans outputs doit
    // subsister pour que grep puisse la détecter.
    let text = r#"{
  inputs = { nixpkgs.url = "github:NixOS/nixpkgs"; };
  outputs = { inputs, nixpkgs, ... }: inputs.nixpkgs.lib.nixosSystem {};
}"#;
    let stripped = strip_inputs_block(text);
    // Le bloc d'inputs est retiré…
    assert!(!stripped.contains("nixpkgs.url"), "url declaration should be stripped");
    // …mais la référence en outputs reste intacte.
    assert!(stripped.contains("inputs.nixpkgs.lib"), "real usage must survive");
}

#[test]
fn strip_inputs_block_handles_nested_attrsets_inside_inputs() {
    // inputs contient lui-même des accolades imbriquées (e.g. pour
    // `follows`). Le compteur de profondeur doit matcher la bonne
    // accolade fermante.
    let text = r#"{
  inputs = {
    nixpkgs = { url = "github:NixOS/nixpkgs"; };
    home-manager = {
      url = "github:nix-community/home-manager";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
  outputs = { inputs, ... }: inputs.home-manager.homeManagerModules.default;
}"#;
    let stripped = strip_inputs_block(text);
    assert!(!stripped.contains("url ="), "url declarations should be stripped");
    assert!(!stripped.contains("follows"), "follows should be stripped");
    assert!(stripped.contains("inputs.home-manager.homeManagerModules"), "real usage must survive");
}

#[test]
fn strip_inputs_block_handles_multiline_with_comments() {
    // Commentaires à l'intérieur du bloc inputs= — ne doivent pas
    // perturber le comptage des accolades.
    let text = r#"{
  inputs = {
    # Primary nixpkgs channel
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # Latest unstable for pinned packages
    nixpkgs-latest.url = "github:NixOS/nixpkgs";
  };
  outputs = { inputs, ... }: inputs.nixpkgs-latest.legacyPackages;
}"#;
    let stripped = strip_inputs_block(text);
    assert!(!stripped.contains("Primary nixpkgs channel"), "comment inside inputs should be stripped");
    assert!(!stripped.contains("nixpkgs.url"), "url inside inputs should be stripped");
    assert!(stripped.contains("inputs.nixpkgs-latest.legacyPackages"), "outputs reference must survive");
}

#[test]
fn strip_inputs_block_matching_braces_leaves_rest_intact() {
    // Le texte après le bloc inputs= ne doit pas être tronqué.
    // Vérifie que la concaténation [avant] + [après] est correcte.
    let text = "{ inputs = { a = 1; }; outputs = { x = 42; }; }";
    let stripped = strip_inputs_block(text);
    assert!(stripped.contains("outputs"), "outputs block must remain");
    assert!(stripped.contains("x = 42"), "outputs content must remain");
    assert!(!stripped.contains("a = 1"), "inputs content must be stripped");
}

fn sample_entry(version: &str, date: &str) -> FreezeEntry {
    FreezeEntry {
        rev: "abcdef0123456789abcdef0123456789abcdef01".to_string(),
        nar_hash: "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string(),
        version: version.to_string(),
        frozen_at: date.to_string(),
        major_constraint: None,
    }
}

#[test]
fn split_out_frozen_removes_matching_packages() {
    // Two packages, one frozen — the frozen one is extracted into a row
    // with its installed version & freeze date, and is no longer in the
    // to-check list that goes to nix eval.
    let mut packages = vec![
        ("firefox".to_string(), "127.0.1".to_string()),
        ("vivaldi".to_string(), "7.9".to_string()),
    ];
    let mut freezes = Freezes::new();
    freezes.insert("firefox".to_string(), sample_entry("127.0.1", "2026-04-20"));

    let rows = split_out_frozen(&mut packages, &freezes);

    assert_eq!(packages, vec![("vivaldi".to_string(), "7.9".to_string())]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "firefox");
    assert_eq!(rows[0].version, "127.0.1");
    assert_eq!(rows[0].frozen_at, "2026-04-20");
}

#[test]
fn split_out_frozen_empty_freezes_is_noop() {
    let mut packages = vec![
        ("firefox".to_string(), "127.0.1".to_string()),
        ("vivaldi".to_string(), "7.9".to_string()),
    ];
    let before = packages.clone();
    let freezes = Freezes::new();

    let rows = split_out_frozen(&mut packages, &freezes);

    assert!(rows.is_empty());
    assert_eq!(packages, before, "vector must be untouched when no freezes");
}

#[test]
fn split_out_frozen_preserves_order_of_non_frozen() {
    // The non-frozen packages keep their original relative order, which
    // matters because cheni check surfaces updates in the same order
    // they're scanned (broadly alphabetical, user expects stable output).
    let mut packages = vec![
        ("a".to_string(), "1".to_string()),
        ("b".to_string(), "2".to_string()),
        ("c".to_string(), "3".to_string()),
        ("d".to_string(), "4".to_string()),
    ];
    let mut freezes = Freezes::new();
    freezes.insert("b".to_string(), sample_entry("2", "2026-04-20"));
    freezes.insert("d".to_string(), sample_entry("4", "2026-04-20"));

    let _ = split_out_frozen(&mut packages, &freezes);
    let names: Vec<&str> = packages.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, vec!["a", "c"]);
}

#[test]
fn split_out_frozen_skips_freezes_not_in_check_list() {
    // A freeze exists for a package that isn't in the to-check set
    // (e.g. excluded because it wasn't in modules/). The row list must
    // not contain it — we only report what was going to be checked.
    let mut packages = vec![("firefox".to_string(), "127.0.1".to_string())];
    let mut freezes = Freezes::new();
    freezes.insert("kitty".to_string(), sample_entry("0.38", "2026-04-20"));

    let rows = split_out_frozen(&mut packages, &freezes);

    assert_eq!(packages, vec![("firefox".to_string(), "127.0.1".to_string())]);
    assert!(rows.is_empty());
}

// --- suspicious_eval_silence ---

fn classification(up_to_date: usize, unknown: usize) -> Classification {
    Classification {
        minor: Vec::new(),
        major: Vec::new(),
        newer: Vec::new(),
        unknown: (0..unknown).map(|i| format!("pkg-{}", i)).collect(),
        up_to_date,
    }
}

#[test]
fn silence_warning_fires_when_everything_is_unknown() {
    // Zero classified, many Unknown — the signature of a missing
    // nixpkgs-latest input or a systemic nix eval failure. The warning
    // must surface so the user doesn't quietly rely on a broken report.
    let c = classification(0, 123);
    assert!(suspicious_eval_silence(&c).is_some());
}

#[test]
fn silence_warning_silent_for_a_normal_run() {
    // Most packages classified, a handful Unknown — the legitimate
    // "nixpkgs doesn't have a .version for these specific packages" outcome.
    let c = classification(100, 19);
    assert!(suspicious_eval_silence(&c).is_none());
}

#[test]
fn silence_warning_silent_for_a_tiny_config() {
    // < 10 packages: the all-Unknown outcome is plausibly real
    // (think: a config with only obscure self-built flakes). We'd
    // rather miss the false-alarm than nag every minimal setup.
    let c = classification(0, 5);
    assert!(suspicious_eval_silence(&c).is_none());
}

#[test]
fn silence_warning_silent_when_one_classification_lands() {
    // Even a single Up-to-date / Minor / Major / Newer hit means
    // nix eval returned a version for at least one package — eval
    // is working. Don't fire on that.
    let c = classification(1, 50);
    assert!(suspicious_eval_silence(&c).is_none());
}

// --- format_local_age ---

#[test]
fn local_age_today_for_zero_days() {
    assert_eq!(format_local_age(0), "today");
}

#[test]
fn local_age_singular_for_one_day() {
    assert_eq!(format_local_age(1), "1d ago");
}

#[test]
fn local_age_plural_for_more_than_one_day() {
    assert_eq!(format_local_age(2), "2d ago");
    assert_eq!(format_local_age(45), "45d ago");
}
