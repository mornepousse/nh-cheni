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

### Fixed
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

### Build
- `flake.nix` switched from `cargoHash` to `cargoLock.lockFile` — no
  more manual hash bumps after `cargo add`.
- Clippy clean (0 warnings); 58 unit tests + 2 doc tests.

### Documentation
- README: new Scripting section with `--json` examples, History &
  rollback table, Discovery + Maintenance tables, short aliases table.
- DESIGN.md: refreshed architecture tree, replaced misleading roadmap
  with a feature-themed shipped list, updated Future Ideas.
- Memory / agent notes updated with the above.
