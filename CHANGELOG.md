# Changelog

All notable changes to this project. Versioning is calendar-ish while
in `0.1.0-alpha` — expect breaking changes. When `v1.0.0` ships, this
file switches to [Keep a Changelog](https://keepachangelog.com) with
semver.

## Unreleased

### Added
- **Interactive menu** — `cheni` with no subcommand opens a keyboard
  picker with all commands + a one-line status banner. Falls back to
  `--help` when stdout/stdin isn't a TTY.
- **Short aliases** — `ck` (check), `st` (status), `up` (update),
  `ug` (upgrade), `b` (build), `h` (history), `rb` (rollback), `s` (search).
- **Selective generation deletion** — integrated into `cheni history`:
  - `--prune` interactive multi-select
  - `--delete N` / `--delete N..M` — explicit targets or ranges
  - `--keep N` — keep only the N most recent
  - `--older-than 30d` — by age (d/w/m/y)
  - `--gc` — reclaim disk space after deletion
  - Active generation is always protected (refuses to delete it)
- **`cheni check --details`** — lists the "Newer" and "Unknown" package
  buckets (previously only counts were shown).
- **`cheni check --refresh`** — wipes `~/.cache/cheni/versions.json` and
  re-fetches every Repology lookup.
- **`cheni check --json`** — stable JSON output on stdout for CI /
  scripting. See README for schema.
- **`cheni history --full`** — don't truncate the per-step summary.
- **`cheni bug-report`** — prints a markdown report (version, OS, nh/nvd
  versions, flake state, doctor output, cache stats) ready to paste
  into a GitLab issue.
- **First-run hint** — `cheni check`/`pin`/`update` detect a missing
  `nixpkgs-latest` input and show a friendly `cheni init` guide
  instead of silently producing empty output.
- **Panic hook** — on unexpected crash, prints error + location with a
  pointer to `cheni bug-report > report.md` instead of the raw Rust
  trace.
- **Tool-missing hints** — `nh`, `nix`, `nvd`, `git`, `sudo` absent →
  targeted install message with a copy-paste `environment.systemPackages`
  snippet.
- **`cheni doctor` cache check** — counts cached entries, reports age,
  warns on stale `version: null` entries, suggests `cheni check --refresh`.
- **`cheni status` suggestions** — context-aware next-step recommendations
  (missing init, obsolete pins, lock newer than active gen, pinned
  packages waiting, or ✓ all clean).
- **`cheni history` summary names** — shows the actual package names that
  changed (`↑ claude-code (2.1.113 → 2.1.114)`) instead of just counts.
  Detects closure rebuilds (same version, different content) and size
  deltas. Truncates to terminal width by default.
- **`cheni why` categorisation** — groups results by `modules/<cat>`,
  `home (user-level)`, `hosts/<host>`, or `root`. Highlights the matched
  package name in bold green.
- **`cheni check` origin column** — each outdated package is annotated
  with the `.nix` file that declares it (`in modules/dev/esp-idf.nix`).
  Scoped to modules that are actually imported by the active host
  config — commented-out modules and unused flake inputs (e.g. a
  declared-but-unreferenced `zen-browser`) are excluded.
- **Qt 6 name mappings** — `qtbase`, `qtcharts`, `qtconnectivity`,
  `qtdeclarative`, `qtmultimedia`, `qtserialport`, `qttools`,
  `qtwayland`, etc. → `qt` on Repology.
- **Hostname fallback** — `detect_hostname()` tries `/etc/hostname` and
  `$HOSTNAME` when the `hostname` binary isn't in PATH.
- **Concurrent flake checks** — one thread per input for the GitHub/
  GitLab API call, run in parallel with Repology lookups.
- **Adaptive HTTP timeouts** — default 30s per request (was 10s for
  Repology, 5s for GitHub/GitLab) to tolerate slow connections and
  weak machines. Overridable via `CHENI_HTTP_TIMEOUT=<secs>` (min 5).
  Reported in `cheni bug-report` when set.

### Changed
- **`cheni check` layout** — flake inputs section moved to the top
  (most actionable first).
- **`cheni history` output** — per-generation compact summary enabled
  by default (not just with `--diff`). Shows nixpkgs commit hash next
  to the date.
- **`cheni doctor` generation check** — no longer needs sudo; reads
  `/nix/var/nix/profiles/` directly. Threshold raised from 20 → 30.
- **`cheni doctor` hints** — suggest safe `cheni history --keep` /
  `--older-than` instead of `nh clean all` / `nix-collect-garbage`
  (which break rollback by deleting all old generations).
- **`cheni check` sort** — results ordered by relevance (exact match,
  prefix match, substring, other) instead of alphabetical.

### Changed (cont.)
- **`-c, --category <NAME>`** replaces the per-category bool flags
  on `cheni check` and `cheni pin`. The old `--dev` / `--apps` /
  `--desktop` / `--hardware` were hardcoded to one user's modules
  layout; `-c <name>` accepts any subdirectory of `modules/` so a
  user with `modules/gaming/` now gets `cheni pin -c gaming`
  without a code change.
- **Error messages surface the real reason inline** — e.g. "could
  not determine — 'du' binary not in PATH" instead of swallowing
  the cause behind a DEBUG log.
- **Help text** disambiguates `build` / `update` / `upgrade` with
  cross-references and a one-line comparison block (answered
  "what replaced my old `update` shell alias").

### Fixed
- **Pre-release "available" versions** — Repology returns the latest
  known version of a project including alpha/beta/rc pre-releases.
  The version parser stripped the suffix so `python 3.15.0a7` was
  comparing as `[3,15,0]` against an installed `3.14.3` and showing
  up as a "minor update". New `is_prerelease()` helper recognises
  PEP440 (`a7`/`b1`/`rc3`), dash suffixes (`-alpha`, `-beta`, `-rc`,
  `-pre`, `-dev`, `-unstable`, `-snapshot`) and bare `alpha`/`beta`,
  while explicitly NOT flagging calver dates or false-friend strings
  like `1.0-build42`. When the available version is a pre-release and
  the installed version isn't, cheni classifies the package as up-to-date.
- **Noto fonts mapping removed** — `noto-fonts-cjk-sans` /
  `noto-fonts-color-emoji` were mapped to Repology's `fonts:noto`
  meta-bundle, which uses calver (`2026.04.01`) while the sub-packages
  ship per-script versions (`2.004`, `2.051`). Result was a phantom
  "minor update" every time the bundle got tagged. Mapping dropped;
  the sub-packages now fall through to "Unknown" instead of producing
  bogus updates.
- **Source archives shadowing real packages** — the displaylink driver
  landed three derivations in the store: a script, the upstream `.zip`
  source download (parsed as version "620.zip"), and the actual
  `displaylink-6.2.0-30`. `pick_highest_version` selected the source
  download because `[620] > [6, 2, 0]` numerically. Added `.zip`,
  `.tar.gz`/`.tar.bz2`/`.tar.xz`/`.tar.zst`, `.tgz`/`.tbz2`/`.txz` to
  the IGNORED_SUFFIXES filter so source files are skipped and the real
  derivation wins.
- **Repology project namespace collisions** — a single Repology project
  page contains every nix entry that maps to it. For `firefox` that
  means firefox, firefox-esr (visiblename "firefox"!), firefox-mobile,
  firefox-bin, etc. — picking the first nix_unstable entry showed
  `firefox 149 > 140` (the ESR version). Same for `breeze-icons`
  (kdePackages 6.x vs libsForQt5 5.x) and the `exo` collision (Xfce
  file manager 4.20 vs LLM tool 1.0). New version-aware entry picker
  in `lookup_versions_with_installed`: when the caller provides the
  installed version, cheni prefers entries whose version matches
  (exact > major), then falls back to srcname/binname/visiblename.
  Cuts the typical "Newer than nixpkgs" list from ~7 entries to the
  2-3 that are genuinely ahead (flake-input pins, Repology lag).
- **Store version resolution** — when the store contains several
  derivations for the same package (e.g. `mesa-26.0.4` alongside
  `mesa-24.3.2-osmesa`), cheni now picks the highest semantic version
  instead of whichever iterated first.
- **Cache stale nulls** — the cache loader now drops entries with
  `version: null` on read, so past runs that cached "unknown" don't
  masquerade as hits forever.
- **Flake cwd detection** — requires `nixosConfigurations` in the cwd
  `flake.nix` before treating it as the NixOS config. Avoids picking
  up the cheni source flake when running from `~/cheni`.
- **flake.lock indirection** — resolves `root.inputs[name]` before
  reading `locked` info (handles transitive `nixpkgs_4`-style nodes).
- **Atomic writes** for `~/.cache/cheni/versions.json`,
  `package-pins.json` and `flake.nix` — tmp-file-then-rename means a
  SIGKILL or two parallel runs can never leave a half-JSON file
  behind. Critical for `package-pins.json` since the Nix overlay
  reads it at every eval.
- **Resilient overlay** — the generated overlay guards the pins
  read with `builtins.pathExists`, so deleting `package-pins.json`
  (or stopping to use cheni entirely) doesn't break the flake.
  A corresponding `cheni doctor` check flags the legacy form.
- **Graceful pins file handling** — empty or whitespace-only file
  treated as "no pins"; corrupt JSON produces a reset command the
  user can copy-paste instead of a raw serde error.
- **Byte-slicing hardened** — `description[..67]` in `cheni search`,
  Git-hash / ISO-date slicing in `flake.rs`, rev truncation in
  `status.rs` all moved to `chars().take()` so non-ASCII input can
  never panic at a codepoint boundary.
- **stderr UTF-8 errors don't truncate** — `cheni build` no longer
  stops capturing nh's stderr on a single non-UTF-8 byte; the bad
  line is logged at DEBUG and skipped.
- **`is_flake_lock_dirty` surfaces why** — `git` not found vs.
  non-zero exit vs. genuinely clean are all distinguishable in
  `-v` output instead of all returning `false`.

### Refactor (portability + cleanup)
- **Dropped hardcoded `mae` username** — `store_paths()` resolves
  the current user via `$USER` → `$LOGNAME` → `dirs::home_dir()`.
- **Dropped hardcoded `x86_64-linux`** in the init overlay
  snippet — now uses `inherit (prev) system` so aarch64-linux
  (Raspberry Pi) and aarch64-darwin (Mac M1/M2) work out of the box.
- **Shrunk `INFRASTRUCTURE_INPUTS`** to the genuinely universal set
  (`nixpkgs`, `nixpkgs-latest`, `home-manager`, `cheni`). Optional
  toolchain flakes like `rust-overlay` and `nixpkgs-esp-dev` are no
  longer silently excluded from visibility.
- **Removed `INPUT_STORE_MAPPINGS`** per-user table. Now
  `find_store_version` tries the input name directly, falling back
  to `?` when no match (still surfacing the UPDATE indicator).
- **Zero `unwrap()` in prod paths** — remaining `.expect()` calls
  assert true-by-construction invariants with a diagnostic message.
- **Regexes in `LazyLock`** — compiled once per program run, not
  per call (measurable on the `cheni build` stderr hot path).
- **`nix::tools::tool_error`** — centralised ENOENT → actionable
  install-hint mapping for `nh`, `nix`, `nvd`, `git`, `sudo`.
- **Tests extracted** — every module's `#[cfg(test)] mod tests`
  block moved to a sibling `tests/<name>.rs` file via `#[path]`.
  Source files stay short, test fixtures easy to browse. Zero
  behavioural change.

### Build
- `flake.nix` switched from `cargoHash` to `cargoLock.lockFile` — no
  more manual hash bumps after `cargo add`.
- Clippy clean (0 warnings); 95 unit tests + 2 doc tests.

### Documentation
- README: new Scripting section with `--json` examples, History &
  rollback table, Discovery + Maintenance tables, short aliases
  table, and an **Uninstalling** section documenting the graceful
  removal contract.
- DESIGN.md: refreshed architecture tree, replaced misleading roadmap
  with a feature-themed shipped list, updated Future Ideas, added
  a Packaging note for the `cargoLock.lockFile` switch.
- Memory / agent notes updated with the above.
