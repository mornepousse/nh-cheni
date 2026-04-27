//! Smoke tests d'intégration : lance le binaire `cheni` réel sur des
//! fixtures isolées.
//!
//! Objectif : attraper les régressions de type "la commande tourne
//! mais ne fait plus rien de correct" — le genre que 644 tests unitaires
//! peuvent manquer si aucun d'entre eux n'exerce le chemin d'exécution
//! complet (dispatch clap → run → output).
//!
//! Chaque test :
//!   - crée un `tempfile::tempdir()` propre (parallel-safe, nettoyage auto),
//!   - pointe `CHENI_CONFIG` dessus via `Command::env()` (jamais `set_var`),
//!   - n'exige aucun accès réseau ni au store Nix.
//!
//! Pour lancer : `cargo test --test smoke`

use std::process::Command;

// Chemin du binaire injecté par Cargo au moment de la compilation des
// tests. Si la variable n'existe pas (build manuel sans cargo test),
// on tombe sur le fallback release ci-dessous.
fn cheni_bin() -> String {
    // `env!` est résolu à la compilation — fonctionnera toujours quand
    // on passe par `cargo test`.
    option_env!("CARGO_BIN_EXE_cheni")
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            // Fallback : binary release dans target/. Utile pour un
            // `cargo build --release && ./tests/smoke` manuel.
            let manifest = env!("CARGO_MANIFEST_DIR");
            format!("{}/target/release/cheni", manifest)
        })
}

/// Crée un tempdir avec un `flake.nix` minimal qui satisfait
/// `config::detect()` via `CHENI_CONFIG` (la présence du fichier suffit,
/// pas besoin de `nixosConfigurations`).
///
/// Le flake est intentionnellement squelettique : pas de `nixpkgs-latest`,
/// pas de `package-pins.json`. C'est le point de départ "utilisateur
/// venant d'installer cheni, avant `cheni init`".
fn make_minimal_flake() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir creation failed");
    std::fs::write(
        dir.path().join("flake.nix"),
        r#"{
  description = "smoke-test fixture";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  outputs = { self, nixpkgs }: {
    nixosConfigurations = {};
  };
}
"#,
    )
    .expect("write flake.nix");
    dir
}

/// Crée un `Command` pré-configuré : binaire cheni, `CHENI_CONFIG`
/// pointant sur `dir`, couleurs désactivées (sortie ASCII pure pour les
/// assertions).
fn cmd_in(dir: &tempfile::TempDir) -> Command {
    let mut c = Command::new(cheni_bin());
    c.env("CHENI_CONFIG", dir.path())
        .env("NO_COLOR", "1")
        // Évite toute interaction TTY dans le sandbox CI.
        .env("TERM", "dumb");
    c
}

// ---------------------------------------------------------------------------
// Test 1 — `cheni --version`
// ---------------------------------------------------------------------------

#[test]
fn version_exits_zero_and_matches_semver() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = Command::new(cheni_bin())
        .arg("--version")
        .env("NO_COLOR", "1")
        .current_dir(dir.path())
        .output()
        .expect("failed to run cheni --version");

    assert!(
        out.status.success(),
        "cheni --version returned non-zero exit code: {:?}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    // La version est affichée sur stdout par clap. Format attendu : "cheni vX.Y.Z" ou
    // "cheni vX.Y.Z-N-gHASH" (git describe). On vérifie juste le préfixe `v\d`.
    assert!(
        stdout.contains('v') && stdout.chars().any(|c| c.is_ascii_digit()),
        "cheni --version output does not look like a version string: {:?}",
        stdout
    );
}

// ---------------------------------------------------------------------------
// Test 2 — `cheni --help`
// ---------------------------------------------------------------------------

#[test]
fn help_exits_zero_and_mentions_tagline() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = Command::new(cheni_bin())
        .arg("--help")
        .env("NO_COLOR", "1")
        .current_dir(dir.path())
        .output()
        .expect("failed to run cheni --help");

    assert!(
        out.status.success(),
        "cheni --help returned non-zero: {:?}",
        out.status
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("Granular package updates"),
        "cheni --help output does not contain expected tagline.\nGot: {}",
        stdout
    );
}

// ---------------------------------------------------------------------------
// Test 3 — `cheni pin` (liste) sur flake non-initialisé
// Attendu : exit 0, le hint première-fois est affiché (pas un crash).
// Contexte : `config::is_initialized()` retourne false quand `nixpkgs-latest`
// est absent du flake → `cmd::pin::pin_one` affiche le hint et sort proprement.
// ---------------------------------------------------------------------------

#[test]
fn pin_list_on_uninitialised_flake_exits_zero() {
    // Le flake minimal de make_minimal_flake() n'a pas de nixpkgs-latest.
    let dir = make_minimal_flake();
    let out = cmd_in(&dir)
        .arg("pin")
        // Pas d'argument → list_pins() → detect() réussit, affiche les pins
        // (aucun) et sort proprement.
        .output()
        .expect("failed to run cheni pin");

    assert!(
        out.status.success(),
        "cheni pin (list) should exit 0 even without pins: {:?}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );

    // La sortie doit mentionner soit "no active pins", soit "Pin a package".
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("no active pins") || stdout.contains("pin"),
        "Expected a pin-list output.\nGot: {}",
        stdout
    );
}

// ---------------------------------------------------------------------------
// Test 4 — `cheni status` sur flake minimal (sans nixpkgs-latest, sans pins)
// Attendu : exit 0, sortie contient "no active pins".
// ---------------------------------------------------------------------------

#[test]
fn status_on_minimal_flake_shows_no_active_pins() {
    let dir = make_minimal_flake();
    let out = cmd_in(&dir)
        .arg("status")
        .output()
        .expect("failed to run cheni status");

    assert!(
        out.status.success(),
        "cheni status on minimal flake should exit 0: {:?}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("no active pins"),
        "Expected 'no active pins' in cheni status output.\nGot: {}",
        stdout
    );
}

// ---------------------------------------------------------------------------
// Test 5 — `cheni doctor` sur flake minimal
// Attendu : exit 0 (la commande finit proprement même avec des warnings).
// La sévérité maximale sur un flake vierge est Warning (nh manquant, pas de
// nixpkgs-latest) — jamais un panic ou un exit non-zero inattendu.
// ---------------------------------------------------------------------------

#[test]
fn doctor_on_minimal_flake_exits_zero() {
    let dir = make_minimal_flake();
    let out = cmd_in(&dir)
        .arg("doctor")
        .output()
        .expect("failed to run cheni doctor");

    assert!(
        out.status.success(),
        "cheni doctor should exit 0 (warnings are OK, errors too — the command itself must not crash): {:?}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );

    // La sortie doit contenir le header du rapport.
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("cheni doctor"),
        "Expected 'cheni doctor' header in output.\nGot: {}",
        stdout
    );
}

// ---------------------------------------------------------------------------
// Test 6 — `cheni completion zsh`
// Attendu : exit 0, sortie commence par `#compdef cheni`.
// ---------------------------------------------------------------------------

#[test]
fn completion_zsh_starts_with_compdef() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = Command::new(cheni_bin())
        .args(["completion", "zsh"])
        .env("NO_COLOR", "1")
        .current_dir(dir.path())
        .output()
        .expect("failed to run cheni completion zsh");

    assert!(
        out.status.success(),
        "cheni completion zsh returned non-zero: {:?}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.trim_start().starts_with("#compdef cheni"),
        "cheni completion zsh output should start with '#compdef cheni'.\nGot first 100 chars: {:?}",
        &stdout[..stdout.len().min(100)]
    );
}

// ---------------------------------------------------------------------------
// Test 7 — `cheni man`
// Attendu : exit 0, sortie contient `.TH` (en-tête roff man-page).
// ---------------------------------------------------------------------------

#[test]
fn man_page_starts_with_roff_header() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = Command::new(cheni_bin())
        .arg("man")
        .env("NO_COLOR", "1")
        .current_dir(dir.path())
        .output()
        .expect("failed to run cheni man");

    assert!(
        out.status.success(),
        "cheni man returned non-zero: {:?}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains(".TH"),
        "cheni man output should contain '.TH' (roff man header).\nGot first 200 chars: {:?}",
        &stdout[..stdout.len().min(200)]
    );
}

// ---------------------------------------------------------------------------
// Test 8 — `cheni history` (liste, read-only) avec accès aux profils
// Attendu : exit 0, la commande affiche le header "cheni history" sans
// paniquer. L'accès à /nix/var/nix/profiles/ est en lecture seule, donc
// ce test ne requiert pas sudo et fonctionne en sandbox comme en local.
// ---------------------------------------------------------------------------

#[test]
fn history_list_exits_zero_and_shows_header() {
    let dir = make_minimal_flake();
    let out = cmd_in(&dir)
        .arg("history")
        .output()
        .expect("failed to run cheni history");

    assert!(
        out.status.success(),
        "cheni history (list) should exit 0: {:?}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );

    // Qu'il y ait des générations ou non, la sortie doit contenir
    // "cheni history" (header) ou "No generations found" (sandbox vide).
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("cheni history") || stdout.contains("No generations found"),
        "Expected 'cheni history' header or 'No generations found'.\nGot: {}",
        stdout
    );
}

// ---------------------------------------------------------------------------
// Test 9 — `cheni unpin foobar` sur flake non-initialisé
// `pin_one` affiche un hint première-fois et sort exit 0 (comportement
// intentionnel). Pour tester l'exit non-zero sur commande mal formée on
// utilise `cheni unpin` sans argument — clap/dispatch doit retourner une
// erreur.
// ---------------------------------------------------------------------------

#[test]
fn unpin_without_args_exits_nonzero_with_usage_hint() {
    let dir = make_minimal_flake();
    let out = cmd_in(&dir)
        .arg("unpin")
        .output()
        .expect("failed to run cheni unpin");

    assert!(
        !out.status.success(),
        "cheni unpin with no args should exit non-zero.\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // Le message d'erreur doit mentionner comment utiliser la commande.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("unpin") || stderr.contains("--all") || stderr.contains("Usage"),
        "Expected a usage hint in error output.\nGot: {}",
        stderr
    );
}

// ---------------------------------------------------------------------------
// Test 10 — `cheni unfreeze foobar` sans flake initialisé
// Même logique : dispatch_unfreeze avec un nom valide mais no flake init.
// Comportement attendu : `config::detect()` réussit (flake.nix existe),
// puis `cmd::unfreeze::unfreeze_one` dit que le paquet n'est pas freezé.
// Exit 0 (information gracieuse) ou non-zero (erreur attendue) — on teste
// juste l'absence de panic.
// ---------------------------------------------------------------------------

#[test]
fn unfreeze_unknown_package_does_not_panic() {
    let dir = make_minimal_flake();
    let out = cmd_in(&dir)
        .args(["unfreeze", "nonexistentpackage123"])
        .output()
        .expect("failed to run cheni unfreeze");

    // La commande ne doit PAS se terminer par un signal (panic → SIGABRT).
    // Un exit code non-zero est acceptable, un signal ne l'est pas.
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert!(
            out.status.signal().is_none(),
            "cheni unfreeze panicked (terminated by signal {:?})",
            out.status.signal()
        );
    }

    // Pas de trace de panic dans stderr.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("panicked at"),
        "cheni unfreeze produced a panic message:\n{}",
        stderr
    );
}

// ---------------------------------------------------------------------------
// Test 11 — `cheni search firefox` (conditionnel : seulement si `nix` est
// dans le PATH du sandbox).
// Ce test est le canari originel de la regression : `nix search` était
// silencieusement cassé. Il est skip quand `nix` n'est pas accessible pour
// rester network-free en CI pur.
// ---------------------------------------------------------------------------

#[test]
fn search_firefox_exits_zero_when_nix_available() {
    // Vérifier que `nix` est dans le PATH avant de tenter quoi que ce soit.
    let nix_check = Command::new("nix").arg("--version").output();
    if nix_check.is_err() || !nix_check.unwrap().status.success() {
        // `nix` absent ou cassé : skip silencieux.
        eprintln!("[skip] nix binary not available — skipping cheni search smoke test");
        return;
    }

    let dir = make_minimal_flake();
    let out = cmd_in(&dir)
        .args(["search", "firefox"])
        // Timeout implicite : cargo test tue le processus après 60s.
        .output()
        .expect("failed to run cheni search firefox");

    // `nix search` peut être lent — on vérifie juste que ça n'a pas crashé.
    assert!(
        out.status.success(),
        "cheni search firefox failed when nix is available: {:?}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
