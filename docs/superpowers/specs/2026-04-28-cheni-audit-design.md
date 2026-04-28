# `cheni audit` — design

**Date** : 2026-04-28
**Statut** : design approved, plan à écrire
**Cible release** : v0.6.x ou v0.7.0 (additive feature, pas breaking)
**Phase** : 3a (premier des nouveaux commands de management)

## Contexte

Aujourd'hui, pour avoir le tableau complet de l'état d'un flake, l'utilisateur enchaîne à la main :

```bash
cheni doctor    # santé : lock dirty, inputs vieux, store size, etc.
cheni check     # mises à jour disponibles
cheni status    # pins, freezes, ages d'inputs
```

Trois passes, beaucoup de redondance affichée (les flake-inputs apparaissent dans `check` ET `status`), pas de hiérarchisation claire de ce qu'il faut traiter en premier.

`cheni audit` consolide ça en un seul rapport ordonné par priorité d'action.

## Décision

**Une nouvelle commande `cheni audit`** :

- Re-utilise les vérifications existantes (doctor, check, status)
- Présente l'info dans l'ordre qui maximise l'utilité : TL;DR → erreurs → updates → état pins/freezes → next-action
- Ajoute zéro nouvelle source de donnée (pas de tracking, pas de timeline — c'est Phase 3e)
- Mode `--brief` aligné sur le pattern existant (`doctor --brief`, `status --brief`, `check --brief`)
- Mode `--json` pour scripts

### Ce qu'on gagne

- Un seul appel à mémoriser pour le check de routine
- Hiérarchisation explicite : "voici ce qui doit te concerner, dans l'ordre"
- Une `next-action tip` finale qui pointe l'item le plus prioritaire (calculé une fois)
- Évite la double affiche : flake-inputs apparaît une seule fois (dans la section "updates"), pas en double

### Ce qu'on ne fait pas

- Pas de tracking historique (Phase 3e)
- Pas de cache : l'état change vite, audit re-run frais à chaque appel
- Pas de filtre `--category` (audit = vue flake entier ; pour catégorie, l'utilisateur reste sur `cheni check -c`)

## Architecture

### Nouveau module

```
src/cmd/audit.rs       # orchestrator (run, run_brief, format)
```

### Refactor minimum requis

Pour composer le rapport, `audit` a besoin que `doctor`, `check`, et `status` exposent leurs collectes en tant que **données structurées**, pas juste comme des sous-effets de leur `run()`. On extrait :

```rust
// src/cmd/doctor.rs
pub(crate) fn collect_health(nix_config: &NixConfig) -> HealthReport;

// src/cmd/check.rs
pub(crate) async fn collect_updates(nix_config: &NixConfig) -> Result<UpdatesReport>;

// src/cmd/status.rs
pub(crate) fn collect_state(nix_config: &NixConfig) -> StateReport;
```

`collect_updates` est `async` parce que la chaîne de check passe par `tokio::task::spawn_blocking` pour le `nix eval` ; les deux autres sont sync (lectures de fichiers + parsing).

Chaque `collect_*` renvoie un struct serializable (pour `--json`). Les `run()` existantes continuent à wrapper `collect_* + print_*` — compat préservée.

`audit::run` appelle les 3 collectes en parallèle (`tokio::join!` ou `tokio::task::spawn`), puis compose le rapport unifié.

### Schéma des structs

```rust
pub struct HealthReport {
    pub errors: Vec<HealthIssue>,
    pub warnings: Vec<HealthIssue>,
    pub passed: usize,
}

pub struct HealthIssue {
    pub name: String,
    pub message: String,
    pub hint: Option<String>,
}

pub struct UpdatesReport {
    pub up_to_date: usize,
    pub minor: usize,
    pub major: usize,
    pub newer: usize,
    pub unknown: usize,
    pub frozen: usize,
    pub flake_inputs_with_update: Vec<FlakeInputUpdate>,
}

pub struct StateReport {
    pub pins_count: usize,
    pub freezes_count: usize,
    pub flake_dir: PathBuf,
}

pub struct AuditReport {
    pub health: HealthReport,
    pub updates: UpdatesReport,
    pub state: StateReport,
    pub verdict: AuditVerdict,           // Clear / Warnings / Errors
    pub next_action: Option<String>,     // "Start with `cheni upgrade` ..."
}
```

### Output ordonné

```
=== cheni audit ===

✓ All clear                    # OR ⚠ 2 warnings | ✗ 1 error

Health (doctor):
  ⚠ flake.lock — uncommitted input changes
    → `git diff flake.lock` to inspect, `git checkout flake.lock` to discard
  ✓ 11 other checks passed (Pins, Freezes, …)

Updates available (check):
  Up to date: 116 | Minor: 0 | Major: 0 | Newer: 4 | Unknown: 4

Flake inputs needing update:
  claude-code         2.1.119  →  latest 2026-04-28
  rust-overlay        ?        →  latest 2026-04-28

State (status):
  3 pins · 1 freeze · config /home/mae/nixos-config

→ Next: run `cheni upgrade` to advance the floor before tackling pins.
```

### Verdict computation

`AuditVerdict` est dérivé :

```rust
pub enum AuditVerdict {
    Clear,        // 0 errors, 0 warnings, 0 actionable updates
    Warnings,     // health.warnings.len() > 0  OR  updates.minor + updates.major > 0
    Errors,       // health.errors.len() > 0
}
```

Couleurs : Clear=green, Warnings=yellow, Errors=red. La TL;DR line affiche cette verdict + les counts.

### Next-action heuristic

```rust
fn compute_next_action(report: &AuditReport) -> Option<String> {
    if !report.health.errors.is_empty() {
        return Some(format!("Address `{}` first — it blocks rebuild.",
            report.health.errors[0].name));
    }
    if report.updates.major > 0 {
        return Some("Run `cheni check --details` to see major updates, then `cheni upgrade` if you want to take them.".to_string());
    }
    if !report.updates.flake_inputs_with_update.is_empty() {
        return Some("Run `cheni upgrade` to take the flake-input updates listed above.".to_string());
    }
    if !report.health.warnings.is_empty() {
        return Some(format!("Optional: address `{}` (warning).",
            report.health.warnings[0].name));
    }
    None  // Clear → no next action needed
}
```

## Modes

### `cheni audit` (default)

Le rapport complet décrit ci-dessus.

### `cheni audit --brief`

```
✓ All clear
```

OU

```
⚠ 2 warnings
  · health: 1 warning (flake.lock dirty)
  · updates: 1 flake input (claude-code)
```

Une seule ligne par catégorie qui a un signal. Pas de listing détaillé.

### `cheni audit --json`

Sérialisation directe de `AuditReport`. Schéma stable pour scripts. `--brief` n'a aucun effet en mode JSON (la donnée est toujours complète, l'humain n'est pas le consommateur).

## Data flow

```
                ┌──────────────────┐
                │   audit::run()   │
                └────────┬─────────┘
                         │
         ┌───────────────┼───────────────┐
         ▼               ▼               ▼
  collect_health   collect_updates   collect_state
  (parallel)       (parallel)        (parallel)
         │               │               │
         └───────────────┼───────────────┘
                         ▼
                 compose_report
                         │
                         ▼
              ┌──────────┴──────────┐
              ▼                     ▼
         render_human          render_json
       (with --brief?)
```

## Edge cases

| Situation | Comportement |
|---|---|
| Flake pas init | Health rapporte l'erreur "not initialized", verdict = Errors, next-action = "run `cheni init`". Audit ne tente pas les autres collectes (court-circuit). |
| `nixpkgs-latest` absent | `collect_updates` traite tous les packages comme Unknown (déjà géré côté check). Pas une erreur d'audit. |
| Eval échoue sur un package | Inclus dans `unknown` count. Pas d'arrêt d'audit. |
| Aucune connectivité (flake-input probe fails) | `flake_inputs_with_update` reste vide. La section "Flake inputs" est skippée si vide. |

## Tests

- `src/cmd/tests/audit.rs` (sibling)
- Tests unitaires sur :
  - `compute_next_action` avec différents `AuditReport` (purs, parallel-safe)
  - `AuditVerdict` derivation
  - Sérialisation JSON (round-trip)
  - Format brief vs full (helper `format_brief()` testable)
- Pas de test bout-en-bout qui appelle nix — les `collect_*` sont mockables via fixtures struct.

## Critères de succès

1. `cargo build && cargo clippy && cargo test` passent
2. `nix build .#cheni` passe (gate de release per `feedback_release_sandbox_gate`)
3. `cheni audit` sur un flake sain affiche `✓ All clear` en < 5s quand le `version_cache` (utilisé par `check`) est warm
4. `cheni audit --brief` tient sur 1 ligne quand verdict = Clear
5. `cheni audit --json` valide via `jq` (parse OK)
6. La next-action tip pointe la bonne priorité dans 4 cas testés (clean, warning-only, major-update, error)
7. Pas de régression sur `cheni doctor`, `cheni check`, `cheni status` (les `run()` existantes restent inchangées en surface)

## Out of scope (explicitement)

- Pas de tracking persistant (Phase 3e)
- Pas de filtre `--category`
- Pas de cache d'audit (l'état change vite)
- Pas de sortie partielle quand une collecte échoue (si une collecte panique, l'audit panique — on remontera la stack)
- Pas de re-run automatique périodique (c'est ce que `cheni audit` est dans un cron/loop si l'utilisateur veut)
