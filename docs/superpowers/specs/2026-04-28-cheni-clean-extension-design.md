# `cheni clean` extension — design

**Date** : 2026-04-28
**Statut** : design approved, plan à écrire
**Cible release** : v0.6.x ou v0.7.0
**Phase** : 3b-β (second sous-projet de la phase maintenance)

## Contexte

`cheni clean` aujourd'hui drop les pins obsolètes (nixpkgs a rattrapé nixpkgs-latest). C'est la seule catégorie traitée. Trois autres sources de cruft accumulent dans une config NixOS active :

1. **Orphan pins** : un pin pour `firefox` reste dans `package-pins.json` après que l'utilisateur ait supprimé `firefox` de ses modules. Le pin n'a plus rien à router — il est inerte.
2. **Orphan freezes** : même chose pour `package-freezes.json`.
3. **Cruft cheni-specific** : `result*` symlinks oubliés (de `cheni build` / `nix build`) dans `flake_dir`, et version-cache (`~/.cache/cheni/version-cache.json`) qui peut grossir.

`cheni doctor` flagge déjà le version-cache > 10 MiB. `cheni clean` doit pouvoir agir sur ces signaux.

## Décision

**Étendre `cheni clean`** avec trois flags opt-in pour les nouvelles catégories, en gardant le default behaviour identique (compat).

### CLI

| Invocation | Action |
|---|---|
| `cheni clean` (default) | Drop obsolète UNIQUEMENT (compat) |
| `cheni clean --orphans` | Drop pins/freezes orphelins |
| `cheni clean --cruft` | Drop result symlinks + truncate version-cache si > 10 MiB |
| `cheni clean --all` | Les trois catégories |
| `cheni clean --yes` | Skip confirmation prompts (s'applique à toutes les catégories activées) |

Les flags peuvent se combiner : `cheni clean --orphans --cruft` ≡ `cheni clean --all`. C'est OK, on documente `--all` comme raccourci.

### Confirmation

Chaque catégorie traitée demande sa propre confirmation (sauf si `--yes`). C'est cohérent avec `cheni gc` (Phase 3b-α) et `cheni pin`. Si l'utilisateur dit non à orphans, on continue sur cruft, etc.

## Architecture

### Modifications

```
src/cmd/clean.rs                # étendu : ajout des phases orphans + cruft
src/cmd/tests/clean.rs          # nouveau (l'ancien test inline si présent migre)
```

### Nouvelles fonctions `pub(crate)` dans `clean.rs`

```rust
/// Returns the list of pin names that no active module declares.
pub(crate) fn find_orphan_pins(
    pins: &[String],
    declared_packages: &HashSet<String>,
) -> Vec<String>;

/// Returns the list of freeze names that no active module declares.
pub(crate) fn find_orphan_freezes(
    freezes: &Freezes,
    declared_packages: &HashSet<String>,
) -> Vec<String>;

/// Returns the paths of `result*` symlinks in `flake_dir`.
pub(crate) fn find_result_symlinks(flake_dir: &Path) -> Vec<PathBuf>;

/// Returns the size in bytes of the version cache, or 0 if missing.
pub(crate) fn version_cache_size_bytes() -> u64;
```

### Nouveaux helpers d'apply

```rust
/// Removes the listed orphan pins from `package-pins.json`.
fn apply_remove_orphan_pins(flake_dir: &Path, names: &[String]) -> Result<()>;

/// Removes the listed orphan freezes from `package-freezes.json`.
fn apply_remove_orphan_freezes(flake_dir: &Path, names: &[String]) -> Result<()>;

/// Deletes the `result*` symlinks from `flake_dir`.
fn apply_remove_result_symlinks(paths: &[PathBuf]) -> Result<usize>;

/// Truncates the version cache (deletes the file).
fn apply_truncate_version_cache() -> Result<()>;
```

### `CleanOptions`

```rust
pub struct CleanOptions {
    pub orphans: bool,
    pub cruft: bool,
    pub yes: bool,
}

impl CleanOptions {
    pub fn all(&self) -> Self { ... }  // helper for --all flag
    pub fn anything_explicit(&self) -> bool {
        self.orphans || self.cruft
    }
}
```

### Nouveau `run`

```rust
pub fn run(opts: CleanOptions) -> Result<()> {
    // Phase 1: obsolete pins (always — backwards compat)
    run_obsolete_phase(...)?;

    if opts.orphans {
        run_orphans_phase(...)?;
    }
    if opts.cruft {
        run_cruft_phase(...)?;
    }
    Ok(())
}
```

Chaque `run_*_phase` :
1. Détecte la cruft de sa catégorie
2. Print le résumé
3. Si rien à faire, log "nothing to clean" et return
4. Sinon prompt confirm (sauf --yes), apply, print result

### Threshold pour le version cache

```rust
const VERSION_CACHE_TRUNCATE_THRESHOLD: u64 = 10 * 1024 * 1024; // 10 MiB
```

C'est le même seuil que `doctor`'s warning. Ainsi `cheni clean --cruft` n'agit que sur les caches problématiques. Un cache de 100 KB est laissé tranquille.

## Output examples

### `cheni clean --all` (avec orphans + cruft à nettoyer)

```
=== cheni clean ===

Obsolete pins:
  ✓ Removed 2 obsolete pins. nixpkgs has caught up with nixpkgs-latest.

Orphan pins:
  Found 1 pin(s) declared by no module:
    · firefox

Remove these orphan pins? (y/N) y
  ✓ Removed 1 orphan pin

Orphan freezes:
  ✓ No orphan freezes found.

Cruft:
  Found 2 result symlink(s):
    · /home/mae/nixos-config/result
    · /home/mae/nixos-config/result-1
  Version cache: 4 MiB (below 10 MiB threshold, kept)

Remove the result symlinks? (y/N) y
  ✓ Removed 2 result symlinks
```

### `cheni clean` (default — obsolete only, current behavior)

Identical to today's output.

## Tests

`src/cmd/tests/clean.rs` (nouveau ou étendu) :

- `find_orphan_pins_returns_pins_not_in_declared` — pure
- `find_orphan_pins_handles_empty_pins` — pure
- `find_orphan_pins_handles_all_declared` — pure (returns empty)
- `find_orphan_freezes_returns_freezes_not_in_declared` — pure
- `find_result_symlinks_in_tempdir` — uses `tempfile::TempDir`, creates result/result-1, asserts both detected
- `find_result_symlinks_ignores_non_results` — only `result*` symlinks, not regular files
- `version_cache_size_bytes_returns_zero_for_missing` — pure (mocks via injecting a path? or uses default path which is OK if no cache exists in CI)

Pas de tests pour les `apply_*` (touchent le système). Le run() lui-même est testable indirectement via les phases.

## Edge cases

| Situation | Comportement |
|---|---|
| `package-pins.json` n'existe pas (fresh flake) | `pins::read` renvoie vec vide; tout phase no-op |
| Pas de modules actifs détectés | `extract_package_names` renvoie liste vide → tous les pins sont "orphans". On bail proactivement avec un message explicatif AVANT de lister les "orphans" pour éviter le faux positif. |
| `flake_dir` est read-only | `apply_*` retournent IO error, propagée. |
| User Ctrl-C entre phases | Phases déjà appliquées restent (pas de rollback). C'est cohérent avec `cheni gc` et acceptable. |
| `--orphans` + flake non-init | `cheni clean` n'init pas, on bail avec hint vers `cheni init`. |

## Critères de succès

1. `cargo build && cargo clippy && cargo test` passent
2. `nix build .#cheni` passe
3. `cheni clean` (no flags) produit la même sortie qu'avant la PR — backwards compat
4. `cheni clean --all` détecte + propose suppression des trois catégories
5. `cheni clean --orphans` sur un pin orphelin connu propose suppression et le retire après confirm
6. `cheni clean --cruft` sur un `flake_dir` avec un `result` symlink le détecte et le supprime après confirm
7. `cheni clean --yes --all` skip toutes les confirmations
8. Pas de régression sur les autres commandes qui touchent `pins::read`/`freezes::read`/`extract_package_names`

## Out of scope

- Pas d'auto-cleanup périodique (l'user lance quand il veut)
- Pas de mode dry-run dédié — l'user peut lancer `cheni clean --orphans` et répondre N à la confirmation
- Pas de nettoyage de generations (c'est le rôle de `cheni gc`)
- Pas de nettoyage des inputs flake stales (`cheni doctor` les flagge, l'user agit via `nix flake update`)
- Pas de migration auto si un pin a été renommé (pin name → new name) — out of scope, would need policy
