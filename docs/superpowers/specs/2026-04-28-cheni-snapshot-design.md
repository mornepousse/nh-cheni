# `cheni snapshot` + `cheni restore` — design

**Date** : 2026-04-28
**Statut** : design approved
**Cible release** : v0.7.0 (additive)
**Phase** : 3d (portabilité d'état)

## Décision

Deux nouvelles commandes pour porter l'état pins+freezes entre machines :

- `cheni snapshot [--out FILE]` : dump JSON sur stdout (ou fichier)
- `cheni restore <FILE>` : remplace l'état local par celui du fichier (avec confirmation)

## Format snapshot

```json
{
  "format_version": 1,
  "created_at": "2026-04-28T11:00:00Z",
  "hostname": "morthinkpad",
  "pins": ["firefox", "kicad"],
  "freezes": {
    "vivaldi": {
      "rev": "abc123...",
      "nar_hash": "sha256-...",
      "version": "7.9.3970.55",
      "frozen_at": "2026-04-15",
      "major_constraint": null
    }
  }
}
```

`format_version: 1` → futur changement de schéma incrémente. `restore` bail si version > supported.

## Behavior

### `snapshot`

1. Lit pins + freezes courants
2. Compose le `Snapshot { format_version: 1, created_at, hostname, pins, freezes }`
3. Sérialise en JSON pretty
4. Écrit sur stdout (default) ou `--out FILE` (atomic_write)
5. Affiche un résumé sur stderr ("Snapshotted N pins + M freezes")

### `restore`

1. Lit le fichier, parse le JSON
2. Vérifie `format_version <= 1`, sinon bail
3. Compute le diff vs état local : pins/freezes à ajouter, à retirer, à remplacer
4. Affiche le diff
5. Si rien à changer → "Already in sync" et sort
6. Confirmation **default-no** (destructif — remplace l'état)
7. Apply : `pins::clear` + `pins::add` + `freezes::clear` + `freezes::add` chaque entrée
8. Affiche le résultat

## Architecture

```
src/cmd/snapshot.rs        # snapshot + restore
src/cmd/tests/snapshot.rs  # sibling
```

Helpers `pub(crate)` :
- `compose_snapshot(pins, freezes, hostname) -> Snapshot` (pure)
- `compute_diff(current_pins, current_freezes, snapshot) -> RestoreDiff` (pure)
- `apply_restore(flake_dir, snapshot) -> Result<()>` (mutates state)

Réutilise `pins::{read, clear, add}` et `freezes::{read, clear, add}` existants.

## Tests

Sibling :
- `compose_snapshot_includes_all_pins`
- `compose_snapshot_includes_all_freezes`
- `compute_diff_detects_added_pin`
- `compute_diff_detects_removed_pin`
- `compute_diff_detects_added_freeze`
- `compute_diff_detects_changed_freeze` (same name, different version)
- `compute_diff_empty_when_identical`
- `restore_bails_on_unsupported_format_version`

## CLI

- `cheni snapshot [--out FILE]`
- `cheni restore <FILE> [--yes]`

## Edge cases

| Situation | Comportement |
|---|---|
| Snapshot file inexistant | bail "file not found" |
| JSON invalide | bail avec context |
| `format_version > 1` | bail "this snapshot uses format vN, this cheni supports v1. Update cheni." |
| Restore vers un host avec pins/freezes différents | diff explicit, confirmation requise |
| Restore identique au state local | "Already in sync" et exit 0 |
| `--out` vers un dir non-writable | propage IO error |

## Out of scope

- Pas de merge intelligent (snapshot = remplacement, pas fusion)
- Pas de signature/auth (canal de confiance)
- Pas de portabilité cross-arch (les freezes lockés peuvent être invalides ailleurs — `cheni doctor` flagge au prochain run)
- Pas de modules/, flake.nix, version-cache (seul l'état pin/freeze est portable)
