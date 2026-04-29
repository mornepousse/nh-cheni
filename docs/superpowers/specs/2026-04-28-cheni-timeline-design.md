# `cheni timeline` — design

**Date** : 2026-04-28
**Statut** : design approved
**Cible release** : v0.7.0 (additive)
**Phase** : 3e (dernier de la roadmap management)

## Décision

Persistent operation log dans `~/.cache/cheni/timeline.jsonl`. Append-only, JSON Lines. `cheni timeline` lit + filtre.

## Format event

```json
{"ts": "2026-04-28T11:00:00Z", "kind": "pin", "package": "firefox", "details": {}}
{"ts": "2026-04-28T11:05:00Z", "kind": "promote", "package": "firefox", "details": {"from": "freeze", "to": "pin"}}
{"ts": "2026-04-28T11:10:00Z", "kind": "upgrade", "package": null, "details": {"outcome": "success", "duration_secs": 47}}
```

`kind` valeurs supportées (initiales) :
- `pin`, `unpin`, `freeze`, `unfreeze` — état pkg
- `promote`, `demote` — transitions
- `upgrade`, `build`, `rollback` — system ops
- `restore` — snapshot restore appliqué

Extensible : futurs `kind` possibles sans format_version bump (les anciens cheni ignorent les nouveaux kinds).

## CLI

```
cheni timeline                  # last 20 events, all kinds
cheni timeline --last 50        # last N events
cheni timeline --package firefox # filter by package
cheni timeline --since 7d       # filter by age (7 days)
cheni timeline --kind pin       # filter by event kind
cheni timeline --json           # raw JSONL pass-through
```

Filters combinent (AND).

## Architecture

```
src/nix/timeline.rs        # record() + read_events() + Event struct
src/nix/tests/timeline.rs  # sibling
src/cmd/timeline.rs        # cmd::run + filtering + rendering
src/cmd/tests/timeline.rs  # sibling
```

### Helper crate-wide

```rust
// src/nix/timeline.rs
pub struct Event {
    pub ts: String,
    pub kind: String,
    pub package: Option<String>,
    pub details: serde_json::Value,
}

/// Append an event to the timeline log. Best-effort: any IO error is
/// logged at debug level and swallowed — record() must never fail a
/// caller's main flow (timeline is observational, not authoritative).
pub fn record(kind: &str, package: Option<&str>, details: serde_json::Value);

/// Read events from disk. Returns empty Vec if the file doesn't exist.
pub fn read_events() -> Result<Vec<Event>>;
```

### Call sites instrumentés (initial)

- `cmd/pin.rs::pin_one` → record("pin", Some(name), {})
- `cmd/pin.rs::unpin_*` → record("unpin", Some(name), {})
- `cmd/freeze.rs::freeze_one` → record("freeze", Some(name), {"version": v})
- `cmd/unfreeze.rs::*` → record("unfreeze", Some(name), {})
- `cmd/lifecycle.rs::promote/demote` → record("promote"/"demote", Some(name), {"from": ..., "to": ...})
- `cmd/upgrade/mod.rs::run` → record("upgrade", None, {"outcome": ..., "duration_secs": ...})
- `cmd/build.rs::run` → record("build", None, {"outcome": ...})
- `cmd/rollback.rs::run` → record("rollback", None, {"to_gen": N})
- `cmd/snapshot.rs::restore` → record("restore", None, {"from": hostname, "n_pins": ..., "n_freezes": ...})

Pas tous au même commit — instrumenter les plus importants en premier (pin/unpin/freeze/unfreeze/promote/demote), reste en suivi.

### `cheni timeline` rendering

```
=== cheni timeline (last 20) ===

2026-04-28 11:00 UTC   pin     firefox
2026-04-28 11:05 UTC   promote firefox  (freeze → pin)
2026-04-28 11:10 UTC   upgrade          (success, 47s)
2026-04-28 11:15 UTC   build            (success)
...
```

Format : `<ts> <kind> <package?> <details_summary?>`. Lisible humain, pas tabulaire (les détails varient).

`--json` passe-through brut (la lecture est déjà JSONL, on filtre puis on relâche).

## Edge cases

| Situation | Comportement |
|---|---|
| `~/.cache/cheni/timeline.jsonl` absent | `cheni timeline` affiche "No events yet — operations will be logged from now on." |
| Fichier corrupt (JSONL invalide) | parse line-by-line, skip les invalides, log debug |
| `record()` échoue (disque plein) | log debug, swallow — n'interrompt pas l'op user |
| Fichier > 100 MiB | mention dans `cheni doctor` (pas dans ce ticket) |
| `--since` parse fail | bail "invalid duration: '<input>'. Try 7d, 1h, 30m." |

## Tests

Pure helpers seulement (l'append réel touche FS et est testé indirectement via tests/integration).

- `parse_since_duration_handles_d_h_m`
- `filter_events_by_package`
- `filter_events_by_kind`
- `filter_events_by_since`
- `filter_events_combined_AND`
- `event_serialise_round_trip`

## Out of scope

- Pas d'analytics / stats agrégées
- Pas de sync entre machines (timeline reste local par host)
- Pas de pruning auto (sera ajouté dans `cheni clean --cruft` plus tard si besoin)
- Pas de hook auto pour détecter et reconstruire des events historiques (le timeline démarre vide)
- Pas d'instrumentation de TOUS les call sites au premier commit — les events critiques en premier (pin/freeze/lifecycle/restore), upgrade/build/rollback en commits suivants

## Critères de succès

1. `cargo build && clippy && test && nix build` passent
2. `cheni timeline` affiche le message empty-state sur un nouveau host
3. `cheni pin firefox` puis `cheni timeline` montre l'event pin
4. `cheni timeline --kind pin --last 5` filtre correctement
5. `cheni timeline --json` produit du JSONL valide
6. `cheni unpin firefox --yes` n'échoue pas si le timeline a un IO error (best-effort)
