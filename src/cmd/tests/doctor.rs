use super::*;

// ─── tally_severities ─────────────────────────────────────────────────────────

fn make_result(severity: Severity) -> CheckResult {
    CheckResult {
        severity,
        name: "test".to_string(),
        message: "msg".to_string(),
        hint: None,
    }
}

#[test]
fn tally_severities_counts_correctly() {
    let checks = vec![
        make_result(Severity::Ok),
        make_result(Severity::Ok),
        make_result(Severity::Warning),
        make_result(Severity::Error),
        make_result(Severity::Error),
        make_result(Severity::Error),
    ];
    let (ok, warn, err) = tally_severities(&checks);
    assert_eq!(ok, 2);
    assert_eq!(warn, 1);
    assert_eq!(err, 3);
}

#[test]
fn tally_severities_empty_slice_returns_zeros() {
    let (ok, warn, err) = tally_severities(&[]);
    assert_eq!((ok, warn, err), (0, 0, 0));
}

#[test]
fn tally_severities_all_ok() {
    let checks: Vec<CheckResult> = (0..5).map(|_| make_result(Severity::Ok)).collect();
    let (ok, warn, err) = tally_severities(&checks);
    assert_eq!((ok, warn, err), (5, 0, 0));
}

// ─── check_nixpkgs_latest_input ───────────────────────────────────────────────

#[test]
fn nixpkgs_latest_input_found_returns_ok() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("flake.nix"),
        r#"{ inputs.nixpkgs-latest.url = "github:NixOS/nixpkgs"; }"#,
    ).unwrap();
    let result = check_nixpkgs_latest_input(dir.path());
    assert_eq!(result.severity, Severity::Ok, "message: {}", result.message);
}

#[test]
fn nixpkgs_latest_input_missing_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("flake.nix"),
        r#"{ inputs.nixpkgs.url = "github:NixOS/nixpkgs"; }"#,
    ).unwrap();
    let result = check_nixpkgs_latest_input(dir.path());
    assert_eq!(result.severity, Severity::Error, "message: {}", result.message);
    assert!(result.hint.is_some(), "an actionable hint should be present");
}

#[test]
fn nixpkgs_latest_input_unreadable_flake_returns_error() {
    // Dossier sans flake.nix du tout → erreur de lecture.
    let dir = tempfile::tempdir().unwrap();
    let result = check_nixpkgs_latest_input(dir.path());
    assert_eq!(result.severity, Severity::Error, "message: {}", result.message);
}

// ─── check_nixpkgs_floor_age ──────────────────────────────────────────────────

/// Écrit un flake.lock minimal avec un `nixpkgs` dont le `lastModified`
/// est `now - age_days * 86400`. Permet de tester les seuils d'alerte
/// sans appels réseau ni effets de bord.
fn write_nixpkgs_lock(dir: &std::path::Path, age_days: u64) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let last_modified = now.saturating_sub(age_days * 86400);
    let lock = serde_json::json!({
        "nodes": {
            "root": {
                "inputs": { "nixpkgs": "nixpkgs" }
            },
            "nixpkgs": {
                "locked": {
                    "lastModified": last_modified,
                    "rev": "abcdef1234567890abcdef1234567890abcdef12",
                    "type": "github"
                },
                "original": {
                    "type": "github",
                    "owner": "NixOS",
                    "repo": "nixpkgs"
                }
            }
        },
        "root": "root",
        "version": 7
    });
    std::fs::write(dir.join("flake.lock"), lock.to_string()).unwrap();
}

#[test]
fn nixpkgs_floor_age_fresh_returns_ok() {
    // nixpkgs mis à jour il y a 1 jour → sous le seuil de 3j, résultat Ok.
    let dir = tempfile::tempdir().unwrap();
    write_nixpkgs_lock(dir.path(), 1);
    let result = check_nixpkgs_floor_age(dir.path());
    assert_eq!(result.severity, Severity::Ok, "message: {}", result.message);
}

#[test]
fn nixpkgs_floor_age_stale_returns_warning() {
    // nixpkgs mis à jour il y a 7 jours → dépasse le seuil de 3j.
    let dir = tempfile::tempdir().unwrap();
    write_nixpkgs_lock(dir.path(), 7);
    let result = check_nixpkgs_floor_age(dir.path());
    assert_eq!(result.severity, Severity::Warning, "message: {}", result.message);
    assert!(result.hint.is_some(), "a hint should suggest upgrading");
}

#[test]
fn nixpkgs_floor_age_missing_input_returns_warning() {
    // Pas de nixpkgs dans le lock (config atypique) → Warning avec hint.
    let dir = tempfile::tempdir().unwrap();
    let lock = serde_json::json!({
        "nodes": {
            "root": { "inputs": {} }
        },
        "root": "root",
        "version": 7
    });
    std::fs::write(dir.path().join("flake.lock"), lock.to_string()).unwrap();
    let result = check_nixpkgs_floor_age(dir.path());
    assert_eq!(result.severity, Severity::Warning, "message: {}", result.message);
}

// ─── check_dirty_lock ─────────────────────────────────────────────────────────

/// Initialise un dépôt git minimal dans `dir` avec un commit initial
/// contenant un `flake.lock` vide. Retourne le chemin du dépôt.
/// Nécessite que `git` soit disponible dans le PATH (c'est le cas sur NixOS).
fn init_git_repo_with_lock(dir: &std::path::Path) {
    let lock_path = dir.join("flake.lock");
    std::fs::write(&lock_path, "{}").unwrap();
    // Configuration locale minimale pour que git commit ne réclame pas
    // de user.email/user.name globaux (absents dans le sandbox Nix).
    for (cmd, args) in [
        ("init", vec![]),
        ("config", vec!["user.email", "test@test"]),
        ("config", vec!["user.name", "Test"]),
    ] {
        std::process::Command::new("git")
            .arg("-C").arg(dir)
            .arg(cmd)
            .args(&args)
            .output()
            .unwrap();
    }
    std::process::Command::new("git")
        .arg("-C").arg(dir)
        .args(["add", "flake.lock"])
        .output().unwrap();
    std::process::Command::new("git")
        .arg("-C").arg(dir)
        .args(["commit", "-m", "init"])
        .output().unwrap();
}

#[test]
fn check_dirty_lock_clean_repo_returns_ok() {
    // flake.lock commité et non modifié → Ok, pas d'avertissement.
    let dir = tempfile::tempdir().unwrap();
    init_git_repo_with_lock(dir.path());
    let result = check_dirty_lock(dir.path());
    assert_eq!(result.severity, Severity::Ok, "message: {}", result.message);
}

#[test]
fn check_dirty_lock_dirty_lock_returns_warning() {
    // flake.lock modifié après le commit → Warning avec hint git.
    let dir = tempfile::tempdir().unwrap();
    init_git_repo_with_lock(dir.path());
    // Modifier flake.lock sans le commiter pour le rendre dirty.
    std::fs::write(dir.path().join("flake.lock"), r#"{"modified": true}"#).unwrap();
    let result = check_dirty_lock(dir.path());
    assert_eq!(result.severity, Severity::Warning, "message: {}", result.message);
    let hint = result.hint.as_deref().unwrap_or("");
    assert!(hint.contains("git diff") || hint.contains("git checkout"),
        "hint should mention git commands, got: {hint}");
}

#[test]
fn check_dirty_lock_non_git_dir_returns_ok_skipped() {
    // Dossier sans git → is_repo() retourne false, check skipped → Ok.
    let dir = tempfile::tempdir().unwrap();
    // Aucun git init → pas un repo.
    let result = check_dirty_lock(dir.path());
    assert_eq!(result.severity, Severity::Ok, "message: {}", result.message);
    assert!(result.message.contains("skipped") || result.message.contains("not a git"),
        "message should indicate skip, got: {}", result.message);
}

// ─── classify_store_size (tests existants, inchangés) ─────────────────────────

fn sev(size: &str) -> Severity {
    classify_store_size(size).severity
}

#[test]
fn classify_small_store() {
    assert_eq!(sev("5.2G"), Severity::Ok);
    assert_eq!(sev("48G"), Severity::Ok);
    assert_eq!(sev("800M"), Severity::Ok);
}

#[test]
fn classify_large_store() {
    assert_eq!(sev("76G"), Severity::Warning);
    assert_eq!(sev("51G"), Severity::Warning);
    assert_eq!(sev("1.2T"), Severity::Warning);
    // Case-insensitive unit suffix
    assert_eq!(sev("100g"), Severity::Warning);
}

#[test]
fn classify_unparseable() {
    // Unknown unit or garbage → Ok (no warning), caller just shows it raw.
    assert_eq!(sev("?"), Severity::Ok);
    assert_eq!(sev("unknown"), Severity::Ok);
}

#[test]
fn is_hex_rev_accepts_full_and_short_revs() {
    assert!(is_hex_rev("abcdef0123456789abcdef0123456789abcdef01"));
    assert!(is_hex_rev("abcdef1")); // 7-char short rev (git's default minimum)
}

#[test]
fn is_hex_rev_rejects_non_hex_and_bad_lengths() {
    assert!(!is_hex_rev("abcdeXY")); // non-hex chars
    assert!(!is_hex_rev("abc")); // too short
    assert!(!is_hex_rev("")); // empty
    assert!(!is_hex_rev(&"a".repeat(65))); // too long
}

#[test]
fn is_sri_hash_accepts_sha256_and_sha512() {
    assert!(is_sri_hash(
        "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
    ));
    assert!(is_sri_hash(
        "sha512-BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB=="
    ));
}

#[test]
fn is_sri_hash_rejects_non_sri_and_injection() {
    assert!(!is_sri_hash("abc")); // no sha prefix
    assert!(!is_sri_hash("md5-whatever")); // wrong alg
    assert!(!is_sri_hash("sha256-AAA\"BBB")); // quote injection
    assert!(!is_sri_hash("sha256-AAA\nBBB")); // control char
    assert!(!is_sri_hash(&format!("sha256-{}", "A".repeat(250)))); // way too long
}

// ─── check_pin_freeze_conflict ────────────────────────────────────────────────

#[test]
fn pin_freeze_conflict_returns_ok_when_disjoint() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("package-pins.json"),
        r#"["firefox", "vivaldi"]"#,
    ).unwrap();
    std::fs::write(
        dir.path().join("package-freezes.json"),
        r#"{"alacritty": {"version": "0.13", "rev": "deadbeef", "narHash": "sha256-AAA="}}"#,
    ).unwrap();
    let result = check_pin_freeze_conflict(dir.path()).unwrap();
    assert_eq!(result.severity, Severity::Ok);
}

#[test]
fn pin_freeze_conflict_returns_ok_when_both_empty() {
    let dir = tempfile::tempdir().unwrap();
    let result = check_pin_freeze_conflict(dir.path()).unwrap();
    assert_eq!(result.severity, Severity::Ok);
}

#[test]
fn pin_freeze_conflict_flags_overlap_as_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("package-pins.json"),
        r#"["firefox", "vivaldi"]"#,
    ).unwrap();
    std::fs::write(
        dir.path().join("package-freezes.json"),
        r#"{"firefox": {"version": "140.0", "rev": "deadbeef", "narHash": "sha256-AAA="}}"#,
    ).unwrap();
    let result = check_pin_freeze_conflict(dir.path()).unwrap();
    assert_eq!(result.severity, Severity::Error);
    assert!(result.message.contains("firefox"));
    assert!(result.hint.is_some(), "must point at the manual edit needed");
}
