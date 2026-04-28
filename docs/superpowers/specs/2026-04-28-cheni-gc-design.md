# `cheni gc` — design

**Date** : 2026-04-28
**Statut** : design approved, plan à écrire
**Cible release** : v0.6.x ou v0.7.0
**Phase** : 3b-α (premier des deux sous-projets de la phase maintenance)

## Contexte

Le store nix de l'utilisateur fait actuellement ~110 GB. `cheni doctor` flagge cette taille et pointe sur `cheni history --keep 20 --gc` comme remède. Mais cette voie a deux problèmes :

1. **Discoverability** : pour faire un nettoyage disque, l'utilisateur doit savoir que ça passe par `cheni history` avec les bons flags. C'est non-évident.
2. **Pas de safety guard** : `cheni history --keep 0 --gc` accepterait sans broncher, laissant l'utilisateur sans capacité de rollback.

`cheni gc` extrait ce flow dans une commande dédiée, ajoute du preview structuré et des safety guards, sans réinventer ce qui existe.

## Décision

**Une nouvelle commande `cheni gc`** qui orchestre le nettoyage disque en quatre phases (audit → safety → preview → apply), réutilisant les helpers de `history.rs` et `nix::gc`.

### Ce qu'on gagne

- Un nom de commande explicite pour la maintenance disque
- Safety guards : refuse de descendre sous N générations sans `--force`
- Preview structuré : nb de générations supprimées, nb de paths libérés (la taille en GB n'est pas accessible facilement via `nix store gc --dry-run` — voir Edge cases)
- Flag `--dry-run` qui audit + preview sans toucher quoi que ce soit
- Cohérence avec les autres commandes destructives (`pin`, `freeze`) — confirmation par défaut, `--yes` pour skipper

### Ce qu'on ne fait pas (Phase 3b-β)

- Pas d'orphan pin/freeze detection ici (c'est state hygiene, pas disk space)
- Pas de cleanup `result/` symlinks ni de version-cache pruning ici
- Ces deux items sont Phase 3b-β (extension de `cheni clean`)

## Architecture

### Nouveau module

```
src/cmd/gc.rs           # orchestrator (run, GcOptions, render)
src/cmd/tests/gc.rs     # sibling tests
```

### Refactor minimum requis

Le code de prune existe déjà dans `src/cmd/history.rs::run_delete` (ligne 488) et `run_gc` (ligne 695). On extrait les briques en helpers `pub(crate)` réutilisables :

```rust
// src/cmd/history.rs
pub(crate) struct PrunePlan {
    /// Generation IDs that would be deleted.
    pub deleted_ids: Vec<u32>,
    /// Generation IDs kept (the most recent N).
    pub kept_ids: Vec<u32>,
    /// Sum of `kept_ids.len()` (convenience).
    pub kept_count: usize,
}

pub(crate) fn plan_prune_keep_n(
    generations: &[Generation],
    keep: usize,
) -> PrunePlan;

pub(crate) fn apply_prune(plan: &PrunePlan) -> Result<()>;
```

Le helper `crate::nix::gc::preview(&[])` existe déjà, on le réutilise sans le toucher.

### `GcOptions` et `run`

```rust
pub struct GcOptions {
    /// Number of recent generations to keep (default 10).
    pub keep: usize,
    /// Audit + preview, do not delete anything.
    pub dry_run: bool,
    /// Skip confirmation prompt.
    pub yes: bool,
    /// Brief output (numbers only).
    pub brief: bool,
    /// Override the safety floor (allow keep < min_safety_floor).
    pub force: bool,
}

pub fn run(opts: GcOptions) -> Result<()>;
```

### Constantes

```rust
/// Refuse to gc if the user would keep fewer than this — without `--force`.
const MIN_SAFETY_FLOOR: usize = 3;
```

### Schéma du flow

```
                ┌────────────────┐
                │   gc::run()    │
                └────────┬───────┘
                         │
         ┌───────────────┴─────────────────┐
         │ Phase 1 — Audit                  │
         │ - read_generations()             │
         │ - plan_prune_keep_n(opts.keep)   │
         └───────────────┬─────────────────┘
                         │
                         ▼
         ┌─────────────────────────────────┐
         │ Phase 2 — Safety guards          │
         │ - kept_count >= MIN_SAFETY_FLOOR │
         │   OR opts.force                  │
         └───────────────┬─────────────────┘
                         │
                         ▼
         ┌─────────────────────────────────┐
         │ Phase 3 — Preview                │
         │ - nix::gc::preview() for path #  │
         │ - render summary                 │
         └───────────────┬─────────────────┘
                         │
                  ┌──────┴──────┐
                  │ dry_run?    │
                  └─┬─────────┬─┘
                yes │         │ no
                    ▼         ▼
                  exit    confirm (unless --yes)
                                │
                                ▼
                  ┌─────────────────────────┐
                  │ Phase 4 — Apply          │
                  │ - apply_prune(plan)      │
                  │ - nix-collect-garbage    │
                  │ - render results         │
                  └──────────────────────────┘
```

### Output (default mode)

```
=== cheni gc ===

Audit:
  19 generation(s), kept: 10 most recent (gen 446..455)
  9 generation(s) to remove (gen 437..445, 5d-30d old)

Preview:
  Currently dead: 12,847 store path(s) (lower bound — generations not yet released).

Proceed with gc? (y/N) y

Pruning generations...
  ✓ 9 generations removed
Running garbage collection...
  ✓ 12,847 paths reclaimed

Done in 47s.
```

### Output (--brief)

```
gc: 9 gens, 12847 paths reclaimed in 47s
```

### Output (--dry-run)

Comme default mais skippe la confirmation et l'apply, exit après preview.

## Edge cases

| Situation | Comportement |
|---|---|
| `keep >= total_generations` | Audit dit "nothing to remove", exit clean |
| `keep < MIN_SAFETY_FLOOR` sans `--force` | bail avec hint : "would keep only N gen(s) — below the safety floor of 3. Use --force to override." |
| `keep == 0` même avec `--force` | bail toujours : "keeping 0 generations would leave you unable to rollback. Refusing." |
| `nix-env --delete-generations` échoue partiellement | Continue vers `nix-collect-garbage` quand même (le store gc nettoie ce qui est devenu unréférencé). Log la failure. |
| `nix-collect-garbage` échoue | bail, suggère `nix-store --gc --print-roots` comme déjà dans history.rs |
| Pas de `sudo` disponible | tool_error("sudo", e) — pattern existant |
| User Ctrl-C pendant l'apply | nh/nix s'occupent — pas notre problème, on ne fait pas de rollback partiel |

**Note sur la taille en GB** : `nix store gc --dry-run` renvoie un nombre de paths, pas une taille agrégée. Pour avoir la taille réelle, il faudrait stat chaque path (coûteux : 12k stats). On choisit de NE PAS afficher d'estimate en GB pour le preview, juste le path count. Le user voit la vraie taille reclaimed après l'apply via le output de nix-collect-garbage.

**Note sur la précision du path count** : le preview est calculé AVANT la suppression des générations. Il représente seulement les paths actuellement morts, pas ceux qui le deviendront après l'apply (la suppression de N générations va libérer N closures supplémentaires). Le path count affiché est donc un **lower bound**, jamais un upper bound. Le spec rend cette nuance explicite dans l'output.

## Tests

Sibling `src/cmd/tests/gc.rs` :

- `plan_prune_keep_n_keeps_most_recent` : génère 10 generations, keep=3, vérifie que les 3 plus récents sont kept
- `plan_prune_keep_n_returns_empty_when_keep_exceeds_total` : keep=20 sur 10 gens → deleted_ids vide
- `plan_prune_keep_n_zero_keeps_nothing` : keep=0 → all deleted (mais c'est bloqué par le safety guard, testé séparément)
- `safety_guard_blocks_below_floor` : test pure sur `check_safety_guard(plan, force=false)` — refuses si kept_count < 3
- `safety_guard_allows_with_force` : `force=true` permet kept_count < 3 (sauf 0)
- `safety_guard_blocks_zero_even_with_force` : `kept_count == 0` est toujours bloqué

Pas de tests d'apply (subprocess sudo). Les invocations sont réutilisées des chemins déjà testés via `cheni history --gc`.

## Critères de succès

1. `cargo build && cargo clippy && cargo test` passent
2. `nix build .#cheni` passe
3. `cheni gc --dry-run` ne touche rien et affiche le preview
4. `cheni gc --keep 5` (sur le host de l'utilisateur) confirme + execute, libère les paths attendus
5. `cheni gc --keep 1` sans `--force` bail avec le message d'aide
6. `cheni gc --keep 0` même avec `--force` bail (toujours refusé)
7. `cheni history --keep N --gc` continue à fonctionner sans régression (le code partagé n'a pas changé de surface)

## Out of scope (explicitement)

- Pas d'orphan pin/freeze detection (Phase 3b-β)
- Pas de cleanup `result/` symlinks (Phase 3b-β)
- Pas de version-cache pruning (Phase 3b-β — d'ailleurs le cache n'a pas de TTL temporel par design, c'est l'invalidation par rev qui le keep small)
- Pas de scheduling intégré (c'est ce que `/loop /schedule` font)
- Pas d'estimate de taille en GB pour le preview (coûteux, peu de valeur ajoutée)
- Pas de mode `--auto` qui ferait du gc régulièrement sans intervention (trop risqué pour cheni qui est un outil interactif desktop)
