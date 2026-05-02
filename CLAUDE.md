# cheni — Claude Code instructions

Fork personnel de [nh](https://github.com/nix-community/nh) (Yet Another
Nix Helper). Distribué via flake Nix sur `gitlab.com/harrael/nh-cheni`.

## Scope & non-goals

cheni est un **outil personnel** pour la gestion NixOS de harrael.
Aucun rayonnement community, aucune contribution upstream à nh.

**Ce que cheni est** :
- Un **fork de nh** qui tracke `viperML/nh` upstream via le remote
  `upstream` (`https://github.com/viperML/nh.git`). Future release nh
  = `git fetch upstream && git merge upstream/master`.
- Le porteur de **fonctionnalités NixOS-management spécifiques à
  harrael** (à porter progressivement depuis l'ère wrapper) : pins,
  freezes, version-cache, timeline, repology integration, audit,
  bug-report enrichi, etc.
- Empaqueté avec le binaire `nh` user-facing (pas `cheni`) pour
  préserver la muscle memory et les scripts existants. Le pname Nix
  est `cheni` pour distinguer dans le store.

**Ce que cheni n'est PAS** :
- ❌ Pas un outil community / standalone — usage perso, mono-utilisateur
- ❌ Pas de PRs upstream nh — on ne dérange pas les mainteneurs
- ❌ Pas une réimplémentation from scratch — c'est un fork qui suit nh
- ❌ Pas de CI : quality gates locaux suffisent (`cargo test &&
  cargo clippy && nix flake check`)

## Repo

- **Origin** : https://gitlab.com/harrael/nh-cheni
- **Upstream nh** (remote tracker) : https://github.com/viperML/nh
- **Wrapper-era archive** : tag `wrapper-archive-v0.8.5` (préserve
  l'ancienne implémentation de cheni en wrapper Rust qui shell-out
  à nh)
- **Local** : `~/cheni/`

Le mirror GitHub `mornepousse/cheni` (configuré côté GitLab UI) est
historique, sera à reconfigurer ou supprimer post-pivot.

## Architecture

Workspace Cargo hérité de nh, structure inchangée par rapport à
upstream :

```
crates/
├── nh/         # binaire principal + dispatch CLI (clap)
├── nh-core/    # exec layer, args, installable, update
├── nh-nixos/   # rebuild, generations, rollback
├── nh-clean/   # GC
├── nh-darwin/  # nix-darwin
├── nh-home/    # home-manager
├── nh-remote/  # remote rebuilds
└── nh-search/  # search.nixos.org
xtask/          # man-page + completions generation
```

Les ajouts cheni-spécifiques (à venir, phases 2+) iront soit dans des
modules nouveaux à l'intérieur de `crates/nh/src/`, soit (si la surface
le justifie) dans des nouveaux crates `cheni-pins`, `cheni-freezes`,
etc., listés dans le workspace.

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
   bumps above, never both at once — keep the changelog clear)
2. `cargo build` to validate that `Cargo.lock` updates cleanly
3. `git commit -am "release: <version>"` then `git tag <version>`
   (use the full version including `+cheni.<x>`)
4. `git push && git push --tags`
5. `glab release create <version> --name "<version>" --notes-file <path>`
   so the release shows up on `gitlab.com/harrael/nh-cheni/-/releases`

## Conventions code

Le style de nh (en place dans le workspace) est la référence pour les
fichiers nh-upstream — ne pas re-formatter agressivement (les merges
upstream futurs seront plus propres). Les NOUVEAUX fichiers
cheni-spécifiques peuvent suivre les conventions cheni-wrapper :

- **`run()` court** : orchestrator de quelques lignes, helpers nommés
- **Tests sibling files** quand on contrôle le fichier (pas pour les
  fichiers qui viennent d'upstream nh) :
  ```rust
  #[cfg(test)]
  #[path = "tests/<name>.rs"]
  mod tests;
  ```
- **Atomic writes** pour fichiers critiques via un helper dédié à créer
  côté cheni (pas de helper équivalent côté nh)
- **Pas de `.unwrap()` en prod** — préférer `color_eyre`/`thiserror`
  comme nh
- **Tests parallel-safe** : pas de mutation d'env globale

## Outils externes attendus
- `nh` n'est PAS attendu (notre binaire EST notre nh)
- `nix`, `nix-store`, `nix-env`, `git` (standard NixOS)
- `nvd` ou `dix` (selon ce que nh upstream utilise — actuellement `dix`
  via cargo dep)

## Workflow merge upstream nh

Pour tirer une nouvelle release nh :
```bash
git fetch upstream
git checkout main
git merge upstream/master    # résoudre conflits dans crates/nh/ si on a
                             # touché les mêmes lignes que l'upstream
cargo build                  # valider compile
cargo test                   # valider tests
nix flake check              # valider sandbox build
```

Conflits attendus : essentiellement dans `crates/nh/src/interface.rs`
si on a ajouté des subcommands cheni-spécifiques au dispatch.

## Phases de migration en cours

1. ✅ Bootstrap fork (cette commit) — code = nh upstream + packaging cheni
2. ⏳ Premier port (probablement `pin`) comme proof-of-concept
3. ⏳ Pins + Freezes + Version-cache
4. ⏳ Observabilité (timeline, history, audit)
5. ⏳ Écosystème (repology, search badges, doctor++, bug-report++)
6. ⏳ Self-update du fork
7. ⏳ Décommission du tag wrapper-archive (rester accessible mais retirer
   les références actives)

Voir `/home/mae/.claude/plans/vast-meandering-peacock.md` pour le plan
détaillé Phase 1.
