# nh-cheni — Claude Code instructions

Personal fork of [nh](https://github.com/viperML/nh) (Yet Another Nix
Helper) by harrael. Distributed via Nix flake at
`gitlab.com/harrael/nh-cheni`.

## Scope & non-goals

nh-cheni is a **personal-use tool** for harrael's NixOS workflow.
No community ambition, no upstream contributions to nh.

**What nh-cheni IS**:
- A **fork of nh** that tracks `viperML/nh` upstream via the
  `upstream` git remote (`https://github.com/viperML/nh.git`). New
  nh release = `git fetch upstream && git merge upstream/master`.
- The carrier of **NixOS-management features specific to harrael**:
  pins, freezes, version-cache, timeline, events, check, doctor,
  bug-report, self-update.
- Packaged with the user-facing binary named `nh` (NOT `cheni`) to
  preserve muscle memory and existing scripts. The Nix store path
  identifies the package as `nh-cheni-<version>`.

**What nh-cheni is NOT**:
- ❌ Not a community / standalone tool — single-user, mono-machine
  scope.
- ❌ No upstream PRs to nh — we don't bother the maintainers.
- ❌ Not a from-scratch reimplementation — it's a fork that follows nh.
- ❌ No CI: local quality gates suffice (`cargo test && cargo clippy
  && nix flake check`).

## Repo

- **Origin**: https://gitlab.com/harrael/nh-cheni
- **Upstream nh** (remote tracker): https://github.com/viperML/nh
- **Wrapper-era archive**: tag `wrapper-archive-v0.8.5` (preserves
  the previous cheni implementation as a thin wrapper that shelled
  out to nh; rollback target).
- **Local checkout**: `~/cheni/`

The GitHub mirror `mornepousse/cheni` (configured on the GitLab
side) is a historical artifact; reconfigure or delete it post-
pivot when convenient.

## Architecture

Cargo workspace inherited from nh, structure unchanged from
upstream:

```
crates/
├── nh/         # main binary + top-level CLI dispatch (clap)
├── nh-core/    # exec layer, args, installable, update
├── nh-nixos/   # rebuild, generations, rollback
│               # ← cheni-spec modules live HERE alongside upstream files
├── nh-clean/   # GC
├── nh-darwin/  # nix-darwin
├── nh-home/    # home-manager
├── nh-remote/  # remote rebuilds
└── nh-search/  # search.nixos.org
xtask/          # man-page + completions generation
```

The cheni-specific code is **all inside `crates/nh-nixos/`**, in
flat-named modules: `pins.rs`, `freezes.rs`, `timeline.rs`,
`events.rs`, `check.rs`, `doctor.rs`, `bug_report.rs`,
`self_update.rs`, `versioning.rs`, `version_cache.rs`,
`cheni_meta.rs`. Shared utilities live under
`crates/nh-nixos/src/cheni_util/{atomic,time,validation,flake}.rs`.

The cheni-spec **additions to upstream nh files** are kept tiny
(append-only) so future merges have a small conflict surface:

| File | What we add |
|---|---|
| `crates/nh-nixos/src/args.rs` | `OsXxxArgs` structs + `OsSubcommand::Xxx` variants + `FeatureRequirements` arms |
| `crates/nh-nixos/src/nixos.rs` | One dispatch arm per cheni subcommand |
| `crates/nh-nixos/src/lib.rs` | `pub mod` declarations for cheni-spec modules |
| `crates/nh/build.rs` | Decompose option-B workspace version → `CHENI_FULL_VERSION` |

See `README.md` for a deeper walkthrough.

## Versioning

No `VERSION` file (unlike the wrapper era). The single version
source of truth is `Cargo.toml`, field `[workspace.package].version`.
All crates inherit via `version.workspace = true`.

**Format — option B**: `<nh-base>+cheni.<cheni-layer>`, e.g.
`4.3.2+cheni.0.1.0`. The `+cheni.<x>` part is semver build metadata,
which Cargo accepts and ignores for version resolution. Decomposed
by `crates/nh/build.rs` and rendered as
`nh 4.3.2 (cheni 0.1.0, <rev>)` in `nh --version`.

Two distinct things are bumped on different occasions:

- **Merging upstream nh** → bump the **nh-base** part to whatever
  version of nh upstream we just merged. The cheni-layer part stays.
  Example: after merging nh `v4.4.0`, version becomes
  `4.4.0+cheni.0.1.0`. Use the `cheni-upstream-merger` agent to
  drive this workflow.

- **Adding a cheni feature/fix** → bump the **cheni-layer** part
  (semver discipline: `0.1.0` → `0.2.0` for new subcommand,
  `0.1.0` → `0.1.1` for fix/polish). The nh-base stays.
  Example: after adding `nh os trace`, version becomes
  `4.3.2+cheni.0.2.0`.

To cut a release:
1. Bump `[workspace.package].version` in `Cargo.toml` (one of the two
   bumps above, never both at once — keep the changelog clear).
2. `cargo build` to validate that `Cargo.lock` updates cleanly.
3. `git commit -am "release: v<full-version>"` then
   `git tag -a "v<full-version>" -m "release v<full-version>"`
   (use the full version including `+cheni.<x>`).
4. `git push origin main && git push origin "v<full-version>"`.
5. `glab release create "v<full-version>" -R harrael/nh-cheni --name
   "v<full-version>" --notes-file <path>` so the release shows up on
   `gitlab.com/harrael/nh-cheni/-/releases`.

## Code conventions

The style of upstream nh (already in place across the workspace) is
the reference for **nh-upstream files** — do not reformat them
aggressively (future upstream merges stay clean). **NEW cheni-spec
files** follow these conventions:

- **Short `run()`** — `OsXxxArgs::run` should be a few lines that
  delegate to named helpers (`gather_*`, `print_*_section`,
  `classify_*`, `resolve_*`).
- **Inline `mod tests`** at the bottom of each cheni-spec module
  (NOT sibling files via `#[path]` — that was the wrapper-era
  convention). Rationale: cheni-spec modules sit in the nh-nixos
  crate alongside nh-upstream files that use inline tests; mixing
  patterns within one crate is jarring.
  ```rust
  #[cfg(test)]
  #[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
  mod tests {
      use super::*;
      // ...
  }
  ```
- **Atomic writes** for any file the CLI mutates: use
  `cheni_util::atomic::write` (handles tmp + fsync + rename + 0o600
  + O_NOFOLLOW). Don't write a 4th private copy.
- **No `.unwrap()` in prod** — `?` on `color_eyre::eyre::Result`
  everywhere. `.expect("…")` only when the message annotates a
  verifiable invariant.
- **Parallel-safe tests** — no `std::env::set_var`, no
  `std::env::set_current_dir`, no shared paths. Use `tempfile::TempDir`
  and the `_in()` pattern.
- **English in artifacts** — code comments, README, this file,
  agent files in `.claude/agents/`. Conversation with Claude can
  stay in French.
- **Validation BEFORE format** — any value flowing from disk into
  a Nix expression goes through `cheni_util::validation::*` first.
  Defence in depth at every splice site, not just at write time.

## External tools expected

- `nh` is NOT expected (our binary IS our nh)
- `nix`, `nix-store`, `nix-env`, `git` (standard NixOS)
- `nvd` or `dix` (whatever nh upstream uses — currently `dix` via a
  cargo dep)

## Workflow — merge upstream nh

To pull a new nh release:

```bash
git fetch upstream
git checkout main
git merge upstream/master --no-ff -m "Merge upstream nh <tag>"
# Resolve expected conflicts (additive — keep both sides):
#   - crates/nh-nixos/src/args.rs (OsSubcommand variants)
#   - crates/nh-nixos/src/nixos.rs (dispatch arms)
#   - Cargo.toml workspace.version (keep upstream's nh-base, our +cheni.<x> suffix)
cargo build
cargo test --workspace
nix flake check
```

Then in a SEPARATE commit, bump the nh-base half of the workspace
version to whatever upstream tag we merged
(`git describe --tags upstream/master`).

## Migration phases (history)

1. ✅ Bootstrap fork — replaced wrapper code with nh upstream + cheni
   packaging (commit 25d2799, 2026-05-01).
2. ✅ Pin / Unpin (f4da1e3).
3a. ✅ Freeze / Unfreeze (1c32f71).
3b. ✅ Version-cache infra (7013a23).
4a. ✅ Timeline (d244c3a).
4b. ✅ Events / generation annotation (f45b64c).
5a. ✅ Bug-report (062afc8).
5c. ✅ Doctor MVP (2273c59).
5b. ✅ Versioning module + `nh os check` via nix eval (15f80ee +
    4451cc2).
6.  ✅ Self-update (a6b08e8).
7.  ✅ Decommission migration narrative (memory + plan updated).

Post-pivot polish (2026-05-02):
- ✅ Versioning option B (901a5fb).
- ✅ Recreated 5 cheni-* agents + 1 new (3965905).
- ✅ Audit + `cheni_util` extraction + TOCTOU/rev-validation
   security fixes (14ba73f).
- ✅ Comprehensive README rewrite (9158f4c).

The detailed plan archive is at
`/home/mae/.claude/plans/vast-meandering-peacock.md`.

## Workflow anti-régression (OBLIGATOIRE)

Source unique de vérité : `scripts/check.sh`.
- `./scripts/check.sh --fast` — suite de tests du workspace (`cargo test --workspace`) (~secondes)
- `./scripts/check.sh` — fast + le build complet (`cargo clippy && cargo build`)

`nix flake check` / `nix build .#cheni` ne sont PAS dans check.sh : ils
restent le gate de **release** (voir `/tripwire:release`), trop lourds pour
tourner à chaque fin de tour.

**Activation des hooks git (une fois par clone)** :
```bash
./scripts/install-hooks.sh   # ou: git config core.hooksPath scripts/hooks
```
`pre-push` lance le check complet et bloque le push si rouge. WIP : `git push --no-verify`.

**Hooks Claude Code** (`.claude/settings.json`, automatiques) :
- `PostToolUse` sur édition dans `crates/` ou `xtask/` → `check.sh --fast`.
- `Stop` → check complet (fast + clippy + build). Si l'env de build n'est pas
  disponible, dégrade en `--fast` seul.

**Ratchet de tests** : `.tripwire-testcount` (committé) mémorise le nombre de
tests (`grep '#[test]' crates/`). Une baisse silencieuse est signalée, et
bloque au `pre-push`. Baisse assumée → mettre à jour `.tripwire-testcount` dans
le commit.

### Norme TDD — nouvelle logique pure
Toute nouvelle fonction de logique pure (classification, parsing, décomposition
de version, validation d'entrées) : test écrit **d'abord**, en `mod tests`
inline dans le module. Le test doit être rouge avant l'implémentation, vert
après, et parallel-safe (pas d'état global muté — cf. conventions du fork).

### Économie de modèles (subagents)
Le pipeline check.sh permet de descendre en gamme SANS risque d'hallucination,
mais seulement là où un oracle rattrape l'erreur :
- **Modèle économique (haiku) OK** : transcription de code déjà spécifié,
  refactors mécaniques, extraction citée (`fichier:ligne` obligatoire) — le
  check, la compilation ou le recoupement des citations attrapent la dérive.
- **Jamais en dessous de sonnet** : review, audit, debug, **et l'écriture
  d'assertions de test** — une assertion tautologique ou un verdict halluciné
  passent l'oracle mécanique au vert. Le jugement ne descend pas en gamme.
- Toute tâche économique DOIT finir par `./scripts/check.sh --fast` vert, et
  un test rewiré/écrit DOIT prouver qu'il mord (bug transitoire → rouge → revert).
