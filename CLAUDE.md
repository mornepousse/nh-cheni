# cheni — Claude Code instructions

CLI Rust pour la gestion granulaire de paquets NixOS. Distribué via flake
Nix. Mirror GitHub auto depuis GitLab.

## Scope & non-goals

cheni est un **outil personnel**. Son but : améliorer l'usage de
NixOS *à mon niveau d'utilisateur*, pas concurrencer ou remplacer les
projets existants.

**Ce que cheni est** :
- Une **couche d'augmentation au-dessus de nh** (qui reste l'outil
  canonique de rebuild). cheni shell-out à `nh os switch` et ajoute
  les choses que nh ne sait pas voir.
- Le porteur de **deux états que nh ignore** : pins (per-package
  routing vers nixpkgs-latest) et freezes (per-package lock à un
  rev nixpkgs).
- Un client **Repology** pour répondre à "est-ce que upstream a
  publié plus neuf que nixpkgs ?".
- Un **layer cross-context** : history annotation, rollback drift
  warning, search badges, diff policy header, build pre-flight,
  check --pending — tous croisent l'état pins/freezes/Repology
  avec les flows de nh.

**Ce que cheni n'est PAS** (et ne doit pas devenir) :
- ❌ Pas un **fork de nh**. Discussion explicite session 2026-04-25 :
  rejeté. Sans contribution upstream possible (workflow IA), le
  choix réel est entre wrapper (statu quo, fragilité parsing
  acceptée) et fork-and-freeze (lourd). Wrapper retenu.
- ❌ Pas de **PRs upstream**. cheni évolue dans son repo, jamais
  vers nh / nixpkgs / nvd.
- ❌ Pas un **outil community / standalone**. cheni est packagé
  comme flake mais pour usage perso, pas pour rayonner.
- ❌ Pas un **rebuild tool**. Si un changement cheni demande de
  réimplémenter ce que nh fait déjà, c'est un signal d'arrêt — la
  bonne réponse est "wrapper plus mince" ou "feature reportée",
  pas "réécrire la roue".
- ❌ Pas un **serveur tool**. Cible = desktop NixOS (rollback
  critique, interactivité tolérée, upgrades réguliers manuels).

**Quand un changement cheni semble pousser dans une de ces
directions interdites, c'est un signal de remettre en question le
changement, pas de violer le scope.**

## Repo
- **Origin** : https://gitlab.com/harrael/cheni
- **Mirror** : https://github.com/mornepousse/cheni (push mirror via GitLab UI)
- **Local** : `~/cheni/`

## Versioning — RELEASING.md fait foi

Source de vérité unique : fichier `./VERSION` à la racine. Lu par
`build.rs` ET par `flake.nix` (`pkgs.lib.fileContents ./VERSION`).

Pour cut une release :
1. Bump `VERSION` (format `vX.Y.Z` ou `vX.Y.Z-alpha`)
2. Bump `Cargo.toml::version` à la même valeur sans le `v` (Cargo veut
   un SemVer pur)
3. `git commit -am "release: vX.Y.Z"` puis `git tag vX.Y.Z`
4. `git push && git push --tags`

Ne JAMAIS utiliser `CARGO_PKG_VERSION` ou `git rev-list count` comme
version affichée — voir `RELEASING.md` pour le pourquoi détaillé.

Entre deux releases :
- `cargo build` local → `cheni vX.Y.Z-N-gHASH-dirty` (via `git describe`)
- Nix sandbox → `cheni vX.Y.Z` (lit VERSION, pas de `.git/`)

## Architecture

```
src/
├── main.rs            # clap dispatch, configure_runtime/resolve/dispatch
├── cmd/*.rs           # une commande par fichier ; run() = orchestrator + helpers
├── nix/               # interactions NixOS (store, config, flake, pins)
├── api/               # Repology client + cache
├── version/           # parsing/comparaison versions (calver-aware)
├── util.rs            # atomic_write (tmp + rename, PID suffix)
└── **/tests/*.rs      # tests via #[cfg(test)] #[path] mod tests
```

## Conventions code

- **`run()` court** : orchestrator de quelques lignes. La logique va
  dans des helpers nommés (`gather_*`, `print_*_section`, `dispatch_*`,
  `classify_*`, etc.). Aucune fonction n'excède ~100 lignes hors menu
  statique.
- **Tests sibling files** : pas inline dans le fichier source. Pattern :
  ```rust
  #[cfg(test)]
  #[path = "tests/<name>.rs"]
  mod tests;
  ```
- **Atomic writes** pour tout fichier critique (pins, cache, flake.nix) :
  via `util::atomic_write` qui fait tmp + rename avec PID suffix.
- **Pas de `.unwrap()` en prod**. Les `.expect()` doivent annoter un
  invariant ("stderr was set to piped, must be Some") ou un regex
  compile-time validé par les tests.
- **Tests parallel-safe** : pas de mutation d'env globale. Factoriser
  une fonction pure qui prend la valeur en paramètre, tester celle-là.
  (Le sandbox Nix lance `cargo test` avec full parallelism — local
  `--test-threads=1` ne reproduit pas le bug.)

## Outils externes attendus
- `nh` 4.3+ (rebuild)
- `nix`, `nix-store`, `nix-env`, `git` (standard NixOS)
- `nvd` (optionnel, utilisé par `diff` et `history --diff`)

## Erreurs externes connues
- Repology API : 429 fréquents, retry 1× (honore `Retry-After` header,
  capé à 30s sinon fallback 3s), log debug only
- GitHub API : rate limit anonymous = 60 req/h
- GitLab API : 600 req/min anonymous
- HTTP timeout : default 30s, override via `CHENI_HTTP_TIMEOUT=<secs>`
  (min 5s)
- HTTP body : capé à 5 MiB (via `api::net::check_content_length` +
  `verify_body_size`), refuse les réponses anormalement grosses

## Tests / qualité

```bash
cargo build
cargo test                         # parallel-safe, sibling file tests
cargo clippy --all-targets
cargo audit                        # RustSec advisories (cargo install cargo-audit)
nix flake check
```

CI minimum à atteindre avant push : `cargo build && cargo clippy &&
cargo test && cargo audit` (le dernier si `cargo-audit` est installé,
sinon on signale les advisories lors de la prochaine release).
