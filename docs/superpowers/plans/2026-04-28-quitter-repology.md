# Quitter Repology — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Supprimer la dépendance Repology de cheni et la remplacer par une comparaison nix-native entre l'input nixpkgs courant et `nixpkgs-latest`.

**Architecture:** On ajoute deux modules `src/nix/eval.rs` (wrapper `nix eval --raw <input>#<attr>.version`) et `src/nix/version_cache.rs` (cache `(input-name, input-rev, attr) → version` sous `~/.cache/cheni/version-cache.json`, atomic writes, invalidation par changement de rev). On migre les 5 commandes consommatrices (`check`, `pin`, `search`, `doctor`, `bug_report`) puis on supprime intégralement `src/api/repology.rs`, `src/api/cache.rs` et leurs tests (~1200 LOC).

**Tech Stack:** Rust 2021, tokio (déjà présent), anyhow, serde_json, `util::atomic_write`, subprocess `nix eval`. Pas de nouvelle dépendance.

**Spec source:** `docs/superpowers/specs/2026-04-28-quitter-repology-design.md`

---

## Préambule — État des lieux

Avant la première tâche, l'engineer doit :

- Lire `CLAUDE.md` (conventions code + scope cheni)
- Lire le spec ci-dessus
- Vérifier l'état git : `git status` doit montrer des modifs uncommitted sur `src/api/repology.rs`, `src/api/cache.rs`, `src/api/tests/repology.rs`, `src/tests/http.rs` — **on les jette en Task 1** (rien à sauver, on supprime ces fichiers de toute façon)

---

### Task 1: Reset du WIP Repology

**Files:**
- Reset: `src/api/cache.rs`, `src/api/repology.rs`, `src/api/tests/repology.rs`, `src/tests/http.rs`

- [ ] **Step 1: Vérifier l'état des modifs**

```bash
git status
git diff --stat
```

Expected: les 4 fichiers listés modifiés, ~538 lignes en changement. Ces modifs partent à la poubelle car on supprime tout ce qui touche à Repology.

- [ ] **Step 2: Reset propre**

```bash
git checkout -- src/api/cache.rs src/api/repology.rs src/api/tests/repology.rs src/tests/http.rs
git status
```

Expected: working tree clean.

- [ ] **Step 3: Vérifier que le build est sain**

```bash
cargo build
cargo clippy --all-targets -- -D warnings
cargo test
```

Expected: tout passe (état post-v0.5.10).

---

### Task 2: Créer `src/nix/eval.rs` — wrapper `nix eval`

**Files:**
- Create: `src/nix/eval.rs`
- Create: `src/nix/tests/eval.rs`
- Modify: `src/nix/mod.rs` (ajouter `pub mod eval;`)

- [ ] **Step 1: Écrire le test (sibling file)**

Crée `src/nix/tests/eval.rs` :

```rust
//! Tests for `nix::eval`.

use super::*;

#[test]
fn parse_version_strips_trailing_newline() {
    assert_eq!(parse_eval_output("128.5.0\n"), Some("128.5.0".to_string()));
}

#[test]
fn parse_version_strips_quotes_and_whitespace() {
    assert_eq!(parse_eval_output("  \"1.2.3\"  \n"), Some("1.2.3".to_string()));
}

#[test]
fn parse_version_rejects_empty() {
    assert_eq!(parse_eval_output(""), None);
    assert_eq!(parse_eval_output("\n"), None);
    assert_eq!(parse_eval_output("   "), None);
}

#[test]
fn parse_version_rejects_error_marker() {
    assert_eq!(parse_eval_output("error: attribute 'version' missing"), None);
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test --lib nix::eval
```

Expected: FAIL — `nix::eval` doesn't exist yet.

- [ ] **Step 3: Implémenter `src/nix/eval.rs`**

```rust
//! Wrapper autour de `nix eval --raw` pour récupérer une version
//! depuis un attr d'un input flake.
//!
//! Cette couche remplace l'ancien client Repology : on demande à nix
//! lui-même "quelle version sait-il faire ?" plutôt que d'interroger
//! un agrégateur tiers.

use crate::nix::tools::tool_error;
use anyhow::Result;
use log::debug;
use std::process::Command;

/// Évalue `<input>#<attr>.version` via `nix eval --raw`.
///
/// Renvoie :
/// - `Ok(Some(version))` si l'éval réussit et qu'une version non-vide
///   est extraite
/// - `Ok(None)` si l'attr n'existe pas / est broken / l'éval échoue
///   (loggé en debug, jamais propagé en erreur — un package sans
///   version upstream est un cas normal, pas une condition d'arrêt)
/// - `Err` uniquement si `nix` lui-même est introuvable (ENOENT)
pub fn eval_version(input: &str, attr: &str) -> Result<Option<String>> {
    let target = format!("{input}#{attr}.version");
    debug!("nix eval --raw {target}");

    let output = Command::new("nix")
        .args(["eval", "--raw", &target])
        .output()
        .map_err(|e| tool_error("nix", e))?;

    if !output.status.success() {
        debug!(
            "nix eval failed for {}: {}",
            target,
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return Ok(None);
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    Ok(parse_eval_output(&raw))
}

/// Normalise le stdout de `nix eval --raw` en `Option<String>`.
///
/// Pure pour permettre des tests parallel-safe sans subprocess.
pub(crate) fn parse_eval_output(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches('"').trim();
    if trimmed.is_empty() || trimmed.starts_with("error:") {
        return None;
    }
    Some(trimmed.to_string())
}

#[cfg(test)]
#[path = "tests/eval.rs"]
mod tests;
```

- [ ] **Step 4: Wire le module**

Edit `src/nix/mod.rs` — ajouter `pub mod eval;` après les modules existants (ordre alphabétique conservé).

- [ ] **Step 5: Run test to verify it passes**

```bash
cargo test --lib nix::eval
```

Expected: 4 tests passent.

- [ ] **Step 6: Lint**

```bash
cargo clippy --all-targets -- -D warnings
```

Expected: 0 warning.

- [ ] **Step 7: Commit**

```bash
git add src/nix/eval.rs src/nix/tests/eval.rs src/nix/mod.rs
git commit -m "feat(nix): add eval_version wrapper around nix eval --raw"
```

---

### Task 3: Créer `src/nix/version_cache.rs` — cache hiérarchisé

**Files:**
- Create: `src/nix/version_cache.rs`
- Create: `src/nix/tests/version_cache.rs`
- Modify: `src/nix/mod.rs` (ajouter `pub mod version_cache;`)

- [ ] **Step 1: Écrire les tests (sibling file)**

Crée `src/nix/tests/version_cache.rs` :

```rust
//! Tests for `nix::version_cache`.

use super::*;
use tempfile::TempDir;

fn fresh_cache_path() -> (TempDir, std::path::PathBuf) {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("version-cache.json");
    (dir, path)
}

#[test]
fn empty_when_file_missing() {
    let (_dir, path) = fresh_cache_path();
    let cache = VersionCache::load(&path).expect("load empty");
    assert!(cache.lookup("nixpkgs-latest", "rev1", "firefox").is_none());
}

#[test]
fn store_then_lookup_roundtrip() {
    let (_dir, path) = fresh_cache_path();
    let mut cache = VersionCache::load(&path).expect("load empty");
    cache.store("nixpkgs-latest", "rev1", "firefox", "128.5.0");
    cache.save(&path).expect("save");

    let reloaded = VersionCache::load(&path).expect("reload");
    assert_eq!(
        reloaded.lookup("nixpkgs-latest", "rev1", "firefox"),
        Some("128.5.0".to_string())
    );
}

#[test]
fn rev_change_invalidates_lookup() {
    let (_dir, path) = fresh_cache_path();
    let mut cache = VersionCache::load(&path).expect("load");
    cache.store("nixpkgs-latest", "rev1", "firefox", "128.5.0");
    // même attr, rev différent -> miss
    assert!(cache.lookup("nixpkgs-latest", "rev2", "firefox").is_none());
}

#[test]
fn different_inputs_dont_collide() {
    let (_dir, path) = fresh_cache_path();
    let mut cache = VersionCache::load(&path).expect("load");
    cache.store("nixpkgs", "revA", "firefox", "128.0.0");
    cache.store("nixpkgs-latest", "revA", "firefox", "129.0.0");
    assert_eq!(
        cache.lookup("nixpkgs", "revA", "firefox"),
        Some("128.0.0".to_string())
    );
    assert_eq!(
        cache.lookup("nixpkgs-latest", "revA", "firefox"),
        Some("129.0.0".to_string())
    );
}

#[test]
fn corrupt_file_treated_as_empty() {
    let (_dir, path) = fresh_cache_path();
    std::fs::write(&path, "not json {{").expect("write garbage");
    let cache = VersionCache::load(&path).expect("load tolerates corrupt");
    assert!(cache.lookup("nixpkgs-latest", "rev1", "firefox").is_none());
}
```

- [ ] **Step 2: Vérifier que `tempfile` est dispo**

```bash
grep tempfile Cargo.toml
```

Expected: `tempfile` listé en `[dev-dependencies]` (déjà présent dans cheni). Si absent, ajouter `tempfile = "3"` à `[dev-dependencies]`.

- [ ] **Step 3: Run test to verify it fails**

```bash
cargo test --lib nix::version_cache
```

Expected: FAIL — `VersionCache` doesn't exist yet.

- [ ] **Step 4: Implémenter `src/nix/version_cache.rs`**

```rust
//! Cache `(input-name, input-rev, attr) → version` persisté sur disque.
//!
//! Remplace l'ancien cache HTTP Repology. La clé inclut le rev d'input
//! pour que `nix flake update` invalide automatiquement les entrées
//! sans avoir à raisonner sur des TTLs temporels.
//!
//! Format JSON :
//! ```json
//! {
//!   "nixpkgs-latest": {
//!     "<rev-sha>": { "<attr>": "<version>" }
//!   }
//! }
//! ```

use crate::util::atomic_write;
use anyhow::Result;
use log::debug;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct VersionCache {
    /// input-name -> rev -> attr -> version
    inputs: HashMap<String, HashMap<String, HashMap<String, String>>>,
}

impl VersionCache {
    /// Charge le cache depuis `path`. Un fichier manquant ou corrompu
    /// renvoie un cache vide (loggé en debug pour le second cas).
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)?;
        match serde_json::from_str::<Self>(&raw) {
            Ok(c) => Ok(c),
            Err(e) => {
                debug!("version-cache parse fail at {}: {e}; treating as empty", path.display());
                Ok(Self::default())
            }
        }
    }

    /// Lookup. Renvoie `None` si l'entrée n'existe pas pour ce triplet.
    pub fn lookup(&self, input: &str, rev: &str, attr: &str) -> Option<String> {
        self.inputs
            .get(input)?
            .get(rev)?
            .get(attr)
            .cloned()
    }

    /// Store une version. Pas de save() implicite — le caller batch
    /// les writes puis appelle `save()` une fois.
    pub fn store(&mut self, input: &str, rev: &str, attr: &str, version: &str) {
        self.inputs
            .entry(input.to_string())
            .or_default()
            .entry(rev.to_string())
            .or_default()
            .insert(attr.to_string(), version.to_string());
    }

    /// Persiste le cache via `atomic_write` (tmp+rename, PID suffix).
    pub fn save(&self, path: &Path) -> Result<()> {
        let payload = serde_json::to_vec_pretty(self)?;
        atomic_write(path, &payload)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/version_cache.rs"]
mod tests;
```

- [ ] **Step 5: Wire le module**

Edit `src/nix/mod.rs` — ajouter `pub mod version_cache;` (ordre alphabétique).

- [ ] **Step 6: Run test to verify it passes**

```bash
cargo test --lib nix::version_cache
```

Expected: 5 tests passent.

- [ ] **Step 7: Lint**

```bash
cargo clippy --all-targets -- -D warnings
```

Expected: 0 warning.

- [ ] **Step 8: Commit**

```bash
git add src/nix/version_cache.rs src/nix/tests/version_cache.rs src/nix/mod.rs
git commit -m "feat(nix): add version_cache for (input,rev,attr) -> version"
```

---

### Task 4: Helper d'orchestration `cached_eval_version`

**Files:**
- Modify: `src/nix/eval.rs` (ajouter le helper)
- Modify: `src/nix/tests/eval.rs` (ajouter test sur le helper)

Objectif : centraliser le pattern "cache lookup, miss → eval, store, return" pour que les call-sites n'aient pas à dupliquer cette logique.

- [ ] **Step 1: Écrire le test**

Append à `src/nix/tests/eval.rs` :

```rust
#[test]
fn cached_eval_returns_cached_value_without_subprocess() {
    use crate::nix::version_cache::VersionCache;

    let mut cache = VersionCache::default();
    cache.store("nixpkgs-latest", "rev1", "firefox", "128.5.0");

    // Si le cache hit, le subprocess n'est jamais appelé — on peut
    // donc tester avec un input/rev factice qui ne référence pas
    // un vrai flake.
    let v = lookup_or_eval(&mut cache, "nixpkgs-latest", "rev1", "firefox")
        .expect("cache hit");
    assert_eq!(v, Some("128.5.0".to_string()));
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test --lib nix::eval::tests::cached_eval_returns
```

Expected: FAIL — `lookup_or_eval` doesn't exist.

- [ ] **Step 3: Ajouter le helper à `src/nix/eval.rs`**

Append après `parse_eval_output` :

```rust
use crate::nix::version_cache::VersionCache;

/// Renvoie la version pour `(input, rev, attr)`, en consultant le
/// cache d'abord. Cache miss → `eval_version` puis store. Le caller
/// est responsable du `cache.save(path)` une fois la batch terminée.
pub fn lookup_or_eval(
    cache: &mut VersionCache,
    input: &str,
    rev: &str,
    attr: &str,
) -> Result<Option<String>> {
    if let Some(v) = cache.lookup(input, rev, attr) {
        return Ok(Some(v));
    }
    let evaluated = eval_version(input, attr)?;
    if let Some(ref v) = evaluated {
        cache.store(input, rev, attr, v);
    }
    Ok(evaluated)
}
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test --lib nix::eval
```

Expected: tous les tests passent (4 + 1 nouveau).

- [ ] **Step 5: Commit**

```bash
git add src/nix/eval.rs src/nix/tests/eval.rs
git commit -m "feat(nix): add lookup_or_eval orchestrator (cache-first)"
```

---

### Task 5: Helper "rev d'un input flake"

**Files:**
- Modify: `src/nix/flake.rs` (ajouter `read_input_rev`)
- Modify: `src/nix/tests/flake.rs` (sibling, ajouter test)

Pour utiliser `lookup_or_eval`, on a besoin du rev courant de `nixpkgs-latest`. `read_flake_inputs()` existe déjà mais renvoie une liste filtrée — on a besoin d'une fonction plus directe pour un input nommé.

Note : `read_input_by_name` existe déjà (vu dans le grep préliminaire). Vérifier que `FlakeInput` expose le rev — sinon ajouter un getter ou une nouvelle fonction `read_input_rev(flake_dir, name) -> Option<String>`.

- [ ] **Step 1: Inspection préalable**

```bash
grep -n "pub fn read_input_by_name\|pub struct FlakeInput\|locked_rev\|rev:" src/nix/flake.rs | head -20
```

- [ ] **Step 2: Décider du shape**

Si `FlakeInput` expose déjà un champ `locked_rev: String` ou similaire :
→ **pas besoin de nouvelle fonction**, on appelle `read_input_by_name(flake_dir, "nixpkgs-latest").map(|i| i.locked_rev)` dans le call-site. **Skip Steps 3-5, jump à Step 6.**

Si `FlakeInput` ne l'expose pas :
→ Ajouter un getter ou fonction wrapper `read_input_rev(flake_dir, name) -> Option<String>` qui parse `flake.lock` et renvoie le rev. Continuer Steps 3-5.

- [ ] **Step 3: Écrire le test (si nécessaire)**

Sibling `src/nix/tests/flake.rs` (existe déjà — append au fichier) :

```rust
#[test]
fn read_input_rev_returns_locked_rev() {
    // Fixture : un flake.lock minimal avec un input "nixpkgs-latest"
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("flake.lock"), r#"{
      "nodes": {
        "root": { "inputs": { "nixpkgs-latest": "nixpkgs-latest" } },
        "nixpkgs-latest": {
          "locked": { "rev": "abc123def456", "type": "github" },
          "original": {}
        }
      },
      "root": "root"
    }"#).unwrap();

    let rev = super::read_input_rev(dir.path(), "nixpkgs-latest");
    assert_eq!(rev, Some("abc123def456".to_string()));
}

#[test]
fn read_input_rev_missing_input_returns_none() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("flake.lock"), r#"{
      "nodes": { "root": { "inputs": {} } },
      "root": "root"
    }"#).unwrap();
    assert_eq!(super::read_input_rev(dir.path(), "nixpkgs-latest"), None);
}
```

- [ ] **Step 4: Run test to verify it fails**

```bash
cargo test --lib nix::flake::tests::read_input_rev
```

Expected: FAIL.

- [ ] **Step 5: Implémenter `read_input_rev`**

Dans `src/nix/flake.rs`, ajouter :

```rust
/// Renvoie le `rev` lock d'un input flake nommé, ou `None` si
/// l'input n'existe pas dans `flake.lock` ou n'a pas de `locked.rev`.
pub fn read_input_rev(flake_dir: &Path, name: &str) -> Option<String> {
    read_input_by_name(flake_dir, name)
        .and_then(|i| i.locked_rev.clone())
        // Adapter le nom de champ si différent. Si le champ n'existe
        // pas sur FlakeInput, retourner directement via parsing JSON
        // ad-hoc (cf. read_one_input pour le pattern de parsing).
}
```

**Note pour l'engineer :** vérifier le nom exact du champ sur `FlakeInput` avant de coller ce code. Adapter au schéma réel.

- [ ] **Step 6: Run tests**

```bash
cargo test --lib nix::flake
cargo clippy --all-targets -- -D warnings
```

Expected: tout passe.

- [ ] **Step 7: Commit (uniquement si on a ajouté quelque chose)**

```bash
git add src/nix/flake.rs src/nix/tests/flake.rs
git commit -m "feat(flake): expose read_input_rev for version-cache keys"
```

---

### Task 6: Migrer `cmd/pin.rs`

**Files:**
- Modify: `src/cmd/pin.rs` (lignes 11, 186, 374 — call sites Repology)
- Modify: `src/cmd/tests/pin.rs` (sibling, si présent — adapter mocks)

C'est le plus petit consumer (2 call sites). On commence par lui pour valider l'approche avant d'attaquer `check.rs`.

- [ ] **Step 1: Lire les 2 call sites**

```bash
grep -B2 -A10 "repology::lookup_versions" src/cmd/pin.rs
```

Comprendre ce que la fonction veut afficher : la version "cible" du pin (ce qui sera installé après le rebuild).

- [ ] **Step 2: Identifier l'attr-path et l'input cible**

Aujourd'hui : `repology::lookup_versions(&[(name, Some(installed))])` renvoie un `PackageLookup` dont on lit `latest_version`.

Demain : on veut `nixpkgs-latest#<attr>.version`. Le `name` côté pin est généralement déjà l'attr-path (à vérifier au call site — sinon il existe une fonction de résolution `name → attr` dans le module).

- [ ] **Step 3: Refactor du premier call site (~ligne 186)**

Remplacer :
```rust
use crate::api::repology;
// ...
let lookups = repology::lookup_versions(&[(name.to_string(), Some(installed.to_string()))]).await?;
let latest = lookups.first().and_then(|l| l.latest_version.clone());
```

Par :
```rust
use crate::nix::eval::lookup_or_eval;
use crate::nix::flake::read_input_rev;
use crate::nix::version_cache::VersionCache;
// ...
let cache_path = crate::config::version_cache_path();  // helper à ajouter si absent
let mut cache = VersionCache::load(&cache_path)?;
let rev = read_input_rev(flake_dir, "nixpkgs-latest")
    .ok_or_else(|| anyhow!("input 'nixpkgs-latest' introuvable dans flake.lock"))?;
let latest = lookup_or_eval(&mut cache, "nixpkgs-latest", &rev, name)?;
cache.save(&cache_path)?;
```

**Important** : `name` doit être l'attr-path utilisable par `nix eval`. Si ce n'est pas le cas, utiliser le résolveur attr-path déjà présent ailleurs dans `cmd/pin.rs` (chercher `attr_path` ou similaire).

- [ ] **Step 4: Refactor du second call site (~ligne 374)**

Pattern identique pour la version batch (`packages: Vec<(String, Option<String>)>`) — boucler sur les packages, partager le même `VersionCache` chargé une fois en début, save une fois à la fin.

```rust
let mut cache = VersionCache::load(&cache_path)?;
let rev = read_input_rev(flake_dir, "nixpkgs-latest")
    .ok_or_else(|| anyhow!("input 'nixpkgs-latest' introuvable"))?;
let mut results = Vec::with_capacity(packages.len());
for (name, installed) in &packages {
    let latest = lookup_or_eval(&mut cache, "nixpkgs-latest", &rev, name)?;
    results.push((name.clone(), installed.clone(), latest));
}
cache.save(&cache_path)?;
```

- [ ] **Step 5: Retirer l'import Repology**

Supprimer `use crate::api::repology;` ligne 11 si plus utilisé.

- [ ] **Step 6: Build et test**

```bash
cargo build
cargo test --lib cmd::pin
```

Expected: build OK, tests OK (les tests sibling de pin.rs vont peut-être casser s'ils mockaient Repology — adapter selon).

- [ ] **Step 7: Adapter tests sibling si cassés**

Si `src/cmd/tests/pin.rs` testait via Repology mocks :
- Remplacer les mocks par des fixtures `VersionCache` pré-remplis (la `lookup_or_eval` hit le cache, jamais le subprocess)
- Aucun mock de subprocess nécessaire — le cache pré-rempli court-circuite tout

- [ ] **Step 8: Commit**

```bash
git add src/cmd/pin.rs src/cmd/tests/pin.rs
git commit -m "refactor(pin): swap Repology lookup for nix eval via nixpkgs-latest"
```

---

### Task 7: Migrer `cmd/search.rs`

**Files:**
- Modify: `src/cmd/search.rs` (ligne 25 import, ligne 180 call site)
- Modify: `src/cmd/tests/search.rs`

Plus simple que pin.rs — un seul call site (le batch lookup pour le badge delta).

- [ ] **Step 1: Lire le call site**

```bash
grep -B5 -A15 "repology::lookup_versions" src/cmd/search.rs
```

- [ ] **Step 2: Refactor**

Pattern identique à Task 6 (Step 4) — batch loop, cache partagé, save final.

Différence sémantique à noter dans un commentaire : avant le badge se déclenchait sur "nixpkgs ≠ upstream", maintenant sur "nixpkgs-stable ≠ nixpkgs-latest". L'engineer doit mettre à jour la doc-comment qui décrit le badge :

```rust
//! - **Nixpkgs-latest delta** — when `nixpkgs-latest` has a newer
//!   version than what's on your current channel, we add a small
//!   tag like `(nixpkgs-latest 128.5)`. This replaces the prior
//!   Repology-based delta — sémantique plus actionable, le delta
//!   correspond exactement à ce qu'un `cheni pin` peut faire.
```

- [ ] **Step 3: Build et test**

```bash
cargo build
cargo test --lib cmd::search
```

- [ ] **Step 4: Adapter tests sibling si cassés** (même pattern que Task 6 Step 7)

- [ ] **Step 5: Renommer la fonction `repology_differs`**

Search.rs (ligne 229) a une fonction `repology_differs` — la renommer en `versions_differ` pour cohérence.

```bash
# Vérifier les call-sites
grep -rn "repology_differs" src/
```

Adapter le nom partout dans `src/cmd/search.rs`.

- [ ] **Step 6: Lint**

```bash
cargo clippy --all-targets -- -D warnings
```

- [ ] **Step 7: Commit**

```bash
git add src/cmd/search.rs src/cmd/tests/search.rs
git commit -m "refactor(search): nixpkgs-latest delta replaces Repology delta"
```

---

### Task 8: Migrer `cmd/check.rs` — le cœur

**Files:**
- Modify: `src/cmd/check.rs` (multiples call sites lignes 15, 103, 408, 444, 491)
- Modify: `src/cmd/tests/check.rs`

C'est le consumer principal de Repology. Le refactor est plus volumineux mais le pattern est identique aux deux précédents.

- [ ] **Step 1: Cartographier les call sites**

```bash
grep -n "repology" src/cmd/check.rs
```

Identifier :
- Les types (`HashMap<String, repology::PackageLookup>`) → remplacer par `HashMap<String, Option<String>>` (attr → version, ou `None` si eval échoué/absent)
- Les fonctions de fetch (`gather_*` etc.)
- La fonction de comparaison qui consomme `PackageLookup`

- [ ] **Step 2: Refactor du fetch (~ligne 408-444)**

La fonction `gather_*` qui fait `lookup_versions_with_progress` doit devenir une boucle séquentielle (ou parallèle avec `tokio::task::spawn_blocking` si la perf est insuffisante en séquentiel — à mesurer en Step 8).

Schéma cible :
```rust
fn gather_upstream_versions(
    flake_dir: &Path,
    packages: &[(String, Option<String>)],
    progress: Option<&ProgressBar>,
) -> Result<HashMap<String, Option<String>>> {
    let cache_path = crate::config::version_cache_path();
    let mut cache = VersionCache::load(&cache_path)?;
    let rev = read_input_rev(flake_dir, "nixpkgs-latest");

    let mut out = HashMap::with_capacity(packages.len());
    for (name, _installed) in packages {
        let latest = match &rev {
            Some(r) => lookup_or_eval(&mut cache, "nixpkgs-latest", r, name)?,
            None => None,
        };
        out.insert(name.clone(), latest);
        if let Some(pb) = progress { pb.inc(1); }
    }
    cache.save(&cache_path)?;
    Ok(out)
}
```

- [ ] **Step 3: Refactor du compare (~ligne 491)**

La fonction qui consomme `&HashMap<String, PackageLookup>` devient `&HashMap<String, Option<String>>`. Le code de delta (calver-aware) reste identique — il compare juste deux `&str`.

- [ ] **Step 4: Edge case "pas d'input nixpkgs-latest"**

Si `read_input_rev` renvoie `None`, la map résultat est rempli de `None` partout. Le rendu doit afficher un message info utilisateur :
```rust
if rev.is_none() {
    info!("Pas d'input 'nixpkgs-latest' configuré — section delta désactivée. \
           Configure des pins ou ajoute manuellement l'input pour activer le check.");
}
```

(à placer une seule fois, pas dans la boucle)

- [ ] **Step 5: Build et test**

```bash
cargo build
cargo test --lib cmd::check
```

- [ ] **Step 6: Adapter tests sibling**

Idem Tasks 6/7 — remplacer les mocks Repology par des fixtures `VersionCache` pré-remplis.

- [ ] **Step 7: Lint**

```bash
cargo clippy --all-targets -- -D warnings
```

- [ ] **Step 8: Smoke test perf séquentiel**

```bash
cargo run --release -- check
```

Mesurer le temps. Si > 30s sur ~100 packages, considérer parallélisation via `tokio::task::spawn_blocking` (le subprocess `nix eval` est synchrone). Sinon, séquentiel suffit (`nix eval` est bcp plus rapide sur un store warm — la plupart des packages sont en cache).

- [ ] **Step 9: Commit**

```bash
git add src/cmd/check.rs src/cmd/tests/check.rs
git commit -m "refactor(check): swap Repology fetch for nix eval against nixpkgs-latest"
```

---

### Task 9: Migrer `cmd/doctor.rs`

**Files:**
- Modify: `src/cmd/doctor.rs` (section "Repology cache", ~ligne 821)
- Modify: `src/cmd/tests/doctor.rs`

- [ ] **Step 1: Lire la section actuelle**

```bash
grep -B2 -A30 "Repology cache" src/cmd/doctor.rs
```

- [ ] **Step 2: Remplacer par "Version cache"**

Réécrire la section pour pointer sur `~/.cache/cheni/version-cache.json` :
- Existence du fichier
- Taille (warning si > 10 MiB par exemple)
- Parse OK ou flag corrupt
- Date de dernière modif (info, pas warning)

```rust
fn check_version_cache() -> CheckResult {
    let path = crate::config::version_cache_path();
    if !path.exists() {
        return CheckResult {
            name: "Version cache".to_string(),
            status: CheckStatus::Ok,
            detail: "no cache yet (will be created on first lookup)".into(),
        };
    }
    let metadata = match std::fs::metadata(&path) {
        Ok(m) => m,
        Err(e) => return CheckResult {
            name: "Version cache".to_string(),
            status: CheckStatus::Warn,
            detail: format!("cannot stat: {e}"),
        },
    };
    let size_mb = metadata.len() as f64 / 1_048_576.0;
    let parses = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<crate::nix::version_cache::VersionCache>(&s).ok())
        .is_some();
    let status = if !parses {
        CheckStatus::Warn
    } else if size_mb > 10.0 {
        CheckStatus::Warn
    } else {
        CheckStatus::Ok
    };
    CheckResult {
        name: "Version cache".to_string(),
        status,
        detail: format!("{:.2} MiB, parses: {}", size_mb, parses),
    }
}
```

- [ ] **Step 3: Retirer l'import `crate::api::cache`**

```bash
grep -n "use crate::api" src/cmd/doctor.rs
```

Supprimer toute ligne pointant sur `api::cache` ou `api::repology`.

- [ ] **Step 4: Build et test**

```bash
cargo build
cargo test --lib cmd::doctor
```

- [ ] **Step 5: Lint**

```bash
cargo clippy --all-targets -- -D warnings
```

- [ ] **Step 6: Commit**

```bash
git add src/cmd/doctor.rs src/cmd/tests/doctor.rs
git commit -m "refactor(doctor): replace Repology cache check with version cache check"
```

---

### Task 10: Migrer `cmd/bug_report.rs`

**Files:**
- Modify: `src/cmd/bug_report.rs` (section "Repology cache", ~ligne 127)

- [ ] **Step 1: Lire la section**

```bash
grep -B5 -A20 "Repology cache" src/cmd/bug_report.rs
```

- [ ] **Step 2: Remplacer par section "Version cache"**

Pattern : ouvrir `~/.cache/cheni/version-cache.json`, dump ses metadata (nb d'inputs, nb de revs, nb de versions cached), pas le contenu (privacy + bruit).

```rust
println!("## Version cache");
let path = crate::config::version_cache_path();
if !path.exists() {
    println!("(no cache yet)");
} else {
    match crate::nix::version_cache::VersionCache::load(&path) {
        Ok(c) => {
            // Compter les entrées
            let total: usize = /* sum nested */;
            println!("path: {}", path.display());
            println!("entries: {total}");
        }
        Err(e) => println!("load failed: {e}"),
    }
}
```

(Adapter le total counting au shape concret de `VersionCache.inputs`.)

- [ ] **Step 3: Retirer l'import `crate::api::cache`**

- [ ] **Step 4: Build, test, lint, commit**

```bash
cargo build && cargo clippy --all-targets -- -D warnings && cargo test
git add src/cmd/bug_report.rs
git commit -m "refactor(bug-report): replace Repology cache section with version cache"
```

---

### Task 11: Supprimer `src/api/repology.rs`, `src/api/cache.rs`, leurs tests

**Files:**
- Delete: `src/api/repology.rs`
- Delete: `src/api/cache.rs`
- Delete: `src/api/tests/repology.rs`
- Delete: `src/api/tests/cache.rs` (si présent)
- Modify: `src/api/mod.rs`

À ce point, plus aucune compilation unit ne devrait référencer ces modules.

- [ ] **Step 1: Vérifier l'absence de références résiduelles**

```bash
grep -rn "api::repology\|api::cache\|crate::api::cache\|crate::api::repology" src/
```

Expected: **aucune sortie** (sinon revenir aux Tasks 6-10 fixer le résidu).

- [ ] **Step 2: Supprimer les fichiers**

```bash
rm src/api/repology.rs src/api/cache.rs
rm src/api/tests/repology.rs
rm -f src/api/tests/cache.rs
ls src/api/tests/
```

- [ ] **Step 3: Mettre à jour `src/api/mod.rs`**

Ouvrir `src/api/mod.rs`. Si après suppressions le module est vide ou contient juste un commentaire, **supprimer entièrement le module** :

- Suppr `src/api/mod.rs`
- Suppr `src/api/` (rmdir si vide)
- Edit `src/lib.rs` (ou `src/main.rs`) pour retirer `pub mod api;` / `mod api;`

Sinon (si `src/api/mod.rs` garde du contenu utile, ce qui ne devrait pas être le cas vu le spec), juste retirer `pub mod cache;` et `pub mod repology;`.

- [ ] **Step 4: Build complet**

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

Expected: tout passe, ~1200 LOC dégagées.

- [ ] **Step 5: Vérifier la dégradation**

```bash
git diff --stat HEAD~10 HEAD
```

Doit montrer un net negative côté `src/api/` et un net positive sous `src/nix/`.

- [ ] **Step 6: Commit**

```bash
git add -A src/api/ src/lib.rs src/main.rs
git commit -m "remove(api): delete Repology client and HTTP cache (~1200 LOC)"
```

---

### Task 12: Audit `Cargo.toml`

**Files:**
- Modify: `Cargo.toml` (potentiellement)

- [ ] **Step 1: Identifier les deps potentiellement orphelines**

```bash
cargo machete 2>/dev/null || cargo +nightly udeps 2>/dev/null || echo "Pas d'outil — audit manuel ci-dessous"
```

Audit manuel :
- `reqwest` — utilisé par `src/release.rs` (self-update) → **garder**
- `tokio` — utilisé par `src/release.rs` et toutes les `cmd::*::run` async → **garder**
- `futures` / `futures-util` — utilisé par Repology (Semaphore + buffer_unordered). Vérifier après suppression :
  ```bash
  grep -rn "futures::\|futures_util" src/
  ```
  Si plus aucun usage, retirer la dep.
- Toute autre dep (e.g. `lru`, `dashmap`, `governor`) qui n'a plus de consumer → retirer.

- [ ] **Step 2: Si modifications, build full**

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
```

- [ ] **Step 3: Commit (si modifs)**

```bash
git add Cargo.toml Cargo.lock
git commit -m "deps: drop crates orphaned by Repology removal"
```

---

### Task 13: Mise à jour `CLAUDE.md`

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Retirer la mention Repology dans "Erreurs externes connues"**

Section actuelle :
```markdown
- Repology API : 429 fréquents, retry 1× (honore `Retry-After` header,
  capé à 30s sinon fallback 3s), log debug only
```

À remplacer par :
```markdown
- Version cache : `~/.cache/cheni/version-cache.json`, atomic writes
  via `util::atomic_write`, invalidé automatiquement par changement
  de rev d'input flake (clé `(input-name, input-rev, attr)`)
```

- [ ] **Step 2: Ajuster "Ce que cheni est"**

Section actuelle :
```markdown
- Un client **Repology** pour répondre à "est-ce que upstream a
  publié plus neuf que nixpkgs ?".
```

Remplacer par :
```markdown
- Un comparateur **multi-input nixpkgs** pour répondre à "est-ce
  que `nixpkgs-latest` sait faire plus neuf que ce que tu as ?"
  (via `nix eval --raw`, cache local).
```

- [ ] **Step 3: Ajuster "Architecture"**

Retirer `api/` du tree, ajouter `nix/eval.rs` et `nix/version_cache.rs` :

```
src/
├── ...
├── nix/               # interactions NixOS (store, config, flake, pins,
│                      # eval, version_cache)
├── ...
```

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "docs(claude-md): align with Repology removal"
```

---

### Task 14: Vérification finale

**Files:** N/A (gates de qualité)

- [ ] **Step 1: Pre-release gate**

```bash
cargo build
cargo clippy --all-targets -- -D warnings
cargo test
```

Expected: tout passe.

- [ ] **Step 2: Sandbox Nix gate (obligatoire avant release per `feedback_release_sandbox_gate`)**

```bash
nix build .#cheni
```

Expected: build sandbox passe.

- [ ] **Step 3: Smoke test binaire**

```bash
./result/bin/cheni --version
./result/bin/cheni doctor
./result/bin/cheni check  # peut prendre quelques secondes (eval first time)
./result/bin/cheni check  # second run plus rapide (cache hit)
```

- [ ] **Step 4: Vérification "zéro HTTP sortant"**

Sur une commande qui ne fait pas self-update :
```bash
strace -f -e trace=network ./result/bin/cheni check 2>&1 | grep -E "connect|sendto" | head
```

Expected : aucun appel à un host externe (les seuls socket calls doivent être nix daemon socket et éventuellement DNS local). Si du trafic apparaît, identifier la source — il ne doit plus rester aucun chemin Repology.

- [ ] **Step 5: Diff stats final**

```bash
git diff --stat origin/main...HEAD
```

Vérifier ~1200 LOC supprimées net dans `src/api/`, ~300 LOC ajoutées dans `src/nix/`. Total : sortie nette ~-900 lignes.

- [ ] **Step 6: Bump version v0.6.0**

Suivre `RELEASING.md` :
1. Edit `VERSION` → `v0.6.0`
2. Edit `Cargo.toml::version` → `0.6.0`
3. `cargo build` (régénère `Cargo.lock`)
4. `git commit -am "release: v0.6.0"`
5. `git tag v0.6.0`
6. `git push && git push --tags`

(Note : ne PAS faire ce step automatiquement — l'utilisateur déclenche les releases. À proposer en fin de plan.)

---

## Auto-review du plan

**Spec coverage** :
- ✅ Suppression repology.rs/cache.rs/tests — Task 11
- ✅ Création nix/eval.rs — Task 2
- ✅ Création nix/version_cache.rs — Task 3
- ✅ Refactor 5 commandes — Tasks 6-10
- ✅ Edge case "pas d'input nixpkgs-latest" — Task 8 Step 4
- ✅ Cargo.toml audit — Task 12
- ✅ CLAUDE.md update — Task 13
- ✅ Modifs uncommitted droppées — Task 1
- ✅ Critères de succès vérifiés — Task 14

**Placeholders** : aucun "TBD"/"TODO". Quelques notes "à adapter au schéma réel" en Task 5 (read_input_rev) parce que je n'ai pas inspecté le shape exact de `FlakeInput` — c'est honnête plutôt que d'inventer un nom de champ.

**Type consistency** :
- `VersionCache::lookup(input, rev, attr) -> Option<String>` cohérent partout
- `lookup_or_eval(&mut cache, input, rev, attr) -> Result<Option<String>>` cohérent
- `eval_version(input, attr) -> Result<Option<String>>` cohérent
- `read_input_rev(flake_dir, name) -> Option<String>` cohérent

**Out of scope respecté** : pas de mode `--external`, pas de migration auto cache, pas de refactor naming des commandes, pas de fork nh.
