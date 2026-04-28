# Quitter Repology — design

**Date** : 2026-04-28
**Statut** : design approved, plan à écrire
**Cible release** : v0.6.0 (breaking sémantique sur le delta)

## Contexte

Repology est aujourd'hui la source de vérité de cheni pour répondre à
"upstream a-t-il publié plus neuf que nixpkgs ?". Cette dépendance
pose plusieurs problèmes :

- 429 fréquents (rate limit), retry obligatoire
- Blocklist sur User-Agent récent (commit 6b2ec9a, v0.5.10) — première
  rotation forcée du UA
- ~1200 LOC entre client (`src/api/repology.rs`, 624L), cache
  (`src/api/cache.rs`, 186L) et tests (383L)
- Surface qui grossit malgré nous (changements uncommitted en cours sur
  `repology.rs` au moment du design)
- Friction qui pousse cheni hors de son scope d'outil personnel — la
  feature "tracker l'écosystème upstream mondial" n'a pas vraiment sa
  place dans un wrapper nh

## Décision

**Cheni redéfinit "upstream" = "ce que nixpkgs sait faire de plus
neuf", pas "ce que les devs amont ont publié".**

On supprime intégralement la dépendance Repology et on remplace par une
comparaison entre l'input nixpkgs courant et l'input nixpkgs-latest
déjà utilisé par le mécanisme de pins.

### Ce qu'on perd

- Le signal "upstream a sorti X mais nixpkgs n'a pas suivi". C'est
  acceptable : ce signal était inactionable côté cheni (on ne va pas
  patcher nixpkgs pour l'utilisateur).

### Ce qu'on gagne

- Zéro service tiers, zéro futur blocklist, zéro 429
- Sémantique alignée : le delta affiché correspond exactement à ce
  qu'un `cheni pin <pkg>` ou `cheni upgrade` peut concrètement faire
- ~1200 LOC supprimées
- Cohérence : `cheni pin` route déjà vers nixpkgs-latest, donc l'info
  "version cible" provient désormais de la même source que ce qui sera
  effectivement installé

## Architecture

### Modules supprimés

```
src/api/repology.rs       (624 LOC)
src/api/cache.rs          (186 LOC)
src/api/tests/repology.rs (383 LOC)
```

`src/api/mod.rs` devient quasi-vide (peut être supprimé ou conservé en
placeholder selon ce qui reste après nettoyage).

`src/http.rs` reste — utilisé par `src/release.rs` (self-update).

### Modules ajoutés

**`src/nix/eval.rs`** — wrapper `nix eval`

```rust
/// Evalue `<input>#<attr>.version` et renvoie la version, ou None si
/// l'attribut est inexistant / cassé / marked-broken.
pub fn eval_version(input: &str, attr: &str) -> Result<Option<String>>;
```

Suit le pattern existant des wrappers (`nh`, `nix`, `nvd` dans
`src/nix/tools.rs`). Erreurs eval = `Option<None>` + log debug, pas
crash.

**`src/nix/version_cache.rs`** — cache key (input-name, input-rev, attr) → version

```
~/.cache/cheni/version-cache.json
{
  "<input-name>": {
    "<input-rev-sha>": {
      "<attr-path>": "<version>"
    }
  }
}
```

- Clé hiérarchisée par `input-name` pour permettre, à terme,
  d'évaluer depuis plusieurs inputs (e.g. `nixpkgs` ET
  `nixpkgs-latest`) sans collision
- Atomic writes via `util::atomic_write` (PID suffix, tmp+rename)
- Pas de TTL temporel — la clé `input-rev` invalide automatiquement
  quand l'utilisateur fait `nix flake update`
- Cache miss → eval, store
- Cache corrompu (parse fail) → log debug, repart from scratch
- Pas d'éviction active : la taille du cache est bornée par le
  nombre de packages × le nombre de revs visités. Si ça devient un
  problème, on ajoute un GC plus tard.

### Refactor des consumers

**`cmd/check.rs`** — cœur du changement

Avant :
1. liste packages installés
2. cache invalidation logic Repology
3. `repology::lookup_versions` (HTTP, retry, slug)
4. compare installed vs upstream
5. affiche

Après :
1. liste packages installés (idem)
2. (étape supprimée)
3. pour chaque pkg, résoudre attr-path nixpkgs (idem qu'avant), puis
   `nix::eval::eval_version("nixpkgs-latest", attr)` avec hit cache
4. compare installed vs evaluated
5. affiche (delta vs nixpkgs-latest)

**`cmd/pin.rs`**

Aujourd'hui : appelle `repology::lookup_versions` uniquement pour
afficher la version cible avant le pin (le routing flake lui-même ne
dépend pas de Repology).

Après : eval `nixpkgs-latest#<attr>.version` pour afficher la même
info depuis la source qu'on va concrètement utiliser. Plus cohérent.

**`cmd/search.rs`**

Badge delta : aujourd'hui `installed != repology_latest`, après
`installed != nixpkgs_latest`. Sémantique plus actionable — un delta
affiché correspond à une action que cheni peut faire.

**`cmd/doctor.rs`**

Check "Repology cache" supprimé. Remplacé par check "version cache"
(taille fichier, dernière modif, parse OK). Section pin/freeze
coherence inchangée.

**`cmd/bug_report.rs`**

Section "Repology cache" remplacée par section "Version cache" — même
structure, autre fichier.

## Data flow

### `cheni check`

```
┌─────────────────────┐
│ list installed pkgs │  (parsing modules, déjà existant)
└──────────┬──────────┘
           │
           v
┌─────────────────────┐
│ resolve attr-path   │  (déjà existant)
└──────────┬──────────┘
           │
           v
┌─────────────────────┐    ┌──────────────────┐
│ version_cache lookup│───▶│ hit: use cached  │
│ key=(input,rev,attr)│    └──────────────────┘
└──────────┬──────────┘
           │ miss
           v
┌─────────────────────┐
│ nix eval            │
│ nixpkgs-latest#attr │
│ .version            │
└──────────┬──────────┘
           │
           v
┌─────────────────────┐
│ store in cache      │
└──────────┬──────────┘
           │
           v
┌─────────────────────┐
│ compare vs installed│  (calver-aware, déjà existant)
└──────────┬──────────┘
           │
           v
┌─────────────────────┐
│ render delta        │
└─────────────────────┘
```

### `cheni pin <pkg>`

Identique au flow actuel, sauf l'étape "fetch upstream version for
display" qui passe de `repology::lookup_versions` à
`nix::eval::eval_version("nixpkgs-latest", attr)`.

## Edge cases

| Situation | Comportement |
|-----------|--------------|
| `nix eval` fail (broken / inexistant / IFD blocked) | `Option<None>`, log debug, package "version unknown" dans l'affichage |
| User n'a pas d'input `nixpkgs-latest` configuré | `cmd/check.rs` skippe le delta-check, log info: "configure des pins ou ajoute nixpkgs-latest pour activer le check" |
| Cache corrompu (parse fail) | log debug, ré-éval from scratch |
| Concurrent `cheni check` × 2 | `atomic_write` garantit l'absence de corruption (PID suffix) |
| Premier run après merge (cache Repology résiduel) | ignoré silencieusement, peut être nettoyé manuellement par l'user (pas de migration auto) |

## Tests

- **`src/nix/tests/eval.rs`** (sibling) — `nix eval` mocké via
  trait/fn pointer, tests parallel-safe
- **`src/nix/tests/version_cache.rs`** (sibling) — tests purs sur
  write/read/invalidation par rev change, pas de réseau, pas de mut
  globale
- Suppression intégrale de `src/api/tests/repology.rs`
- Smoke tests binaire conservés, on s'assure que `cheni check`
  fonctionne sans input `nixpkgs-latest` configuré

## Housekeeping

- Modifs uncommitted sur `src/api/repology.rs` (~624 L en changement) :
  jetées via `git checkout -- src/api/` (à confirmer avec
  l'utilisateur avant — rien d'utile à récupérer si ce design tient)
- `Cargo.toml` : audit des deps spécifiques Repology (probablement
  zéro à retirer, `reqwest`/`tokio` étant déjà requis par self-update
  via `src/release.rs`)
- `CLAUDE.md` : section "Erreurs externes connues" — retirer
  l'entrée Repology, ajouter mention du version-cache
- Mémoires utilisateur : pas de purge nécessaire (aucune mémoire ne
  cite Repology directement)
- Release post-merge : **v0.6.0** (breaking sémantique : le sens du
  delta affiché change)
- Naming des commandes : pointé en review utilisateur comme
  perfectible — **out of scope ici**, pass séparé après merge

## Out of scope (explicitement)

- Pas de mode `--external` invoquant `nix-update` ou autre source
  amont. Si le besoin réapparaît, ce sera une feature séparée avec
  son propre design.
- Pas de migration automatique du cache Repology existant (suppression
  manuelle ou laisser pourrir).
- Pas de refactor du naming des commandes (signalé comme perfectible
  mais hors scope).
- Pas de fork de nh ni de PR upstream — cohérent avec le scope cheni
  documenté dans `CLAUDE.md`.

## Critères de succès

1. `cargo build && cargo clippy && cargo test` passent
2. `nix build .#cheni` passe (gate de release obligatoire, cf.
   `feedback_release_sandbox_gate`)
3. `cheni check` affiche un delta sémantiquement correct (= "ce que
   nixpkgs-latest a de plus neuf que ce que tu as installé") sur au
   moins un package pinnable
4. `cheni pin <pkg>` affiche la même version cible que celle
   réellement installée par le rebuild qui suit
5. ~1200 LOC supprimées net (Repology + cache + tests)
6. Aucun appel HTTP sortant déclenché par `cheni check`, `cheni pin`,
   `cheni search`, `cheni doctor` (vérifié manuellement avec
   `strace`/équivalent ou par absence de la dep côté code)
