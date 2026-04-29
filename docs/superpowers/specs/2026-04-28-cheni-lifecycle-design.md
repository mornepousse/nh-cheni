# `cheni promote` + `cheni demote` — design

**Date** : 2026-04-28
**Statut** : design approved
**Cible release** : v0.7.0 (additive, pas breaking)
**Phase** : 3c

## Décision

Deux nouvelles commandes pour flipper l'état pins↔freezes sans devoir faire `unfreeze` puis `pin` (ou inverse) à la main.

| Commande | Direction | Sémantique |
|---|---|---|
| `cheni promote <pkg>` | freeze → pin | "ce package gelé, je veux reprendre les updates via nixpkgs-latest" |
| `cheni demote <pkg>` | pin → freeze | "ce pin, je veux le figer à la version actuelle" |

Verbes explicites (pas de `flip` bidirectionnel ambigu).

## Flow

### `promote <pkg>`

1. Lire freezes; si `pkg` absent → bail "use `cheni freeze <pkg>` first"
2. Lire pins; si `pkg` présent → bail "inconsistent state, run `cheni doctor`"
3. Confirmation default-yes (sauf `--yes`)
4. `freezes::remove(flake_dir, &[pkg])` puis `pins::add(flake_dir, &[pkg])`
5. Affiche "✓ Promoted pkg from freeze to pin. Next rebuild will route via nixpkgs-latest."

### `demote <pkg>`

1. Lire pins; si `pkg` absent → bail "use `cheni pin <pkg>` first"
2. Lire freezes; si `pkg` présent → bail "inconsistent state"
3. `store::find_by_name(pkg)` pour la version installée; si absent → bail "can't demote a pin you don't have installed"
4. `flake::read_input_locked(flake_dir, "nixpkgs-latest")` pour `(rev, nar_hash)`; si absent → bail "no nixpkgs-latest input — run `cheni init`"
5. Confirmation default-yes (sauf `--yes`)
6. Construit `FreezeEntry { rev, nar_hash, version, frozen_at: today_iso(), major_constraint: None }`
7. `pins::remove` puis `freezes::add`
8. Affiche "✓ Demoted pkg from pin to freeze at <version>. Next rebuild will hold this version."

## Architecture

```
src/cmd/lifecycle.rs       # nouveau : promote() + demote() + helpers DRY
src/cmd/tests/lifecycle.rs # sibling
```

`pub fn promote(name: &str, yes: bool) -> Result<()>` et `pub fn demote(name: &str, yes: bool) -> Result<()>`.

Réutilise:
- `crate::nix::pins::{read, add, remove}`
- `crate::nix::freezes::{read, add, remove, FreezeEntry}`
- `crate::nix::store::find_by_name`
- `crate::nix::flake::read_input_locked`
- `crate::cmd::freeze::today_iso` (à faire `pub(crate)` si privée)

## Tests

Pure helpers de validation — bail messages :
- `validate_promote_preconditions` retourne `Result<()>` ou message d'erreur attendu
- `validate_demote_preconditions` idem

Tests sur les 4 chemins d'erreur de chaque commande (absent / inconsistent / no install / no input).

Pas de tests d'apply (touchent fichiers — couverts indirectement par tests de pins/freezes existants).

## Critères de succès

1. `cargo build && clippy && test && nix build` passent
2. `cheni promote firefox` (firefox dans freezes) → ajoute à pins, retire des freezes, succès
3. `cheni demote firefox` (firefox dans pins) → fige à la version installée, retire des pins
4. Bails ciblés sur les 4 erreurs par commande
5. Pas de régression sur `cheni pin` / `cheni freeze` / `cheni unfreeze` / `cheni unpin`

## Out of scope

- Pas de `--all` (pas de demande, YAGNI)
- Pas de `cheni regroup` (Phase 3c-bis si besoin)
- Pas de batch sur catégorie (nécessite tags qui n'existent pas)
- Pas de detection auto de la direction (D explicite > C ambigu)
