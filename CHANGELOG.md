# Changelog

All notable changes to this project. Versioning is calendar-ish while
in `0.1.0-alpha` — expect breaking changes. When `v1.0.0` ships, this
file switches to [Keep a Changelog](https://keepachangelog.com) with
semver.

## Unreleased

### Changed
- **`print_step` and `print_separator` consolidated** in
  `crate::output`. The two helpers were duplicated byte-for-byte
  in `cmd/upgrade.rs` and `cmd/self_update.rs` (the only multi-step
  commands today). Both call sites now thin-alias to the shared
  helpers, so adding a step to a third command picks up the same
  visual without a third copy.
- **`format_elapsed` consolidated** in `crate::util` — four
  identical copies in `cmd/build.rs`, `cmd/upgrade.rs`,
  `cmd/rollback.rs`, `cmd/self_update.rs` collapsed to thin
  aliases over the shared helper. Same `Ss` / `MmSs` output, same
  semantics, single source of truth.

### Added
- **`cheni history` cross-reference tip** — the footer now ends
  with `Tip: rollback with \`cheni rollback <N>\` or compare two
  with \`cheni diff <from> <to>\`.` so the natural follow-up
  actions are one line away from the gen list, mirroring the
  search/why discovery-action pattern from earlier in this
  Unreleased batch.

### Changed
- **Marker glyphs unified** across the cheni surface. The bare `!`
  marker drifted into seven call sites where it was used
  inconsistently for both warnings (yellow) and errors (red),
  overlapping with the `⚠` (yellow) and `✗` (red) markers used
  elsewhere. Convention now: `⚠` for warnings, `✗` for errors,
  `→` for actionable hints, `·` for neutral bullets — no more `!`.
- **`cheni init` collapses to one line on a re-run.** When every
  artefact init would create is already in place (pins file,
  freezes file, nixpkgs-latest input, both overlays), it prints
  a single `✓ Already initialised` banner instead of three
  `[N/3] already configured` lines, then jumps straight to the
  next-steps block.

### Added
- **`cheni status` gains the freshness signals** previously only
  visible in `cheni check` and the interactive banner: the
  Flake inputs list highlights `nixpkgs` / `nixpkgs-latest` in
  yellow when ≥3 days old, and the Suggestions section now
  surfaces two new actionable lines — "nixpkgs floor is Xd ago,
  run `cheni upgrade`" and "flake.lock has uncommitted bumps,
  `git diff flake.lock`/`git checkout flake.lock`". `cheni status`
  finally tells the user whether the snapshot they're reading is
  fresh and consistent.

### Changed
- **Centralised `is_flake_lock_dirty`** in `crate::nix::git`. The
  same `git diff --name-only flake.lock` shell-out lived in
  `cmd/upgrade.rs`, `cmd/doctor.rs`, and `cmd/interactive.rs`. Now
  all four call sites (those three plus the new `cmd/status.rs`
  use) route through one helper, so the dirty-lock signal can't
  drift in wording or behaviour between surfaces.
- **Unified `Xd ago` format** across `cheni status`, `cheni check`,
  `cheni doctor`, and the interactive banner. Previously two
  variants drifted in parallel — `1 day ago / N days ago` and
  `1d ago / Nd ago`. Single source of truth in
  `crate::util::format_days_ago`; column views and inline narration
  both use the compact form for consistency in cross-command
  reading.
- **`cheni search` and `cheni why` get cross-reference tips** at
  their footers:
  - `cheni search` ends with `Tip: pin one with \`cheni pin <name>\`
    (newer version via nixpkgs-latest).` so a user who searched a
    package name knows the next action.
  - `cheni why` ends with `Tip: see if it has updates with \`cheni
    check\`.` so the discovery flow ("which file declares X?")
    points at the obvious follow-up.

## [0.5.7] — 2026-04-27

### Added
- **Dynamic shell completion (zsh)**. `cheni completion zsh`
  now post-processes clap_complete's static output to swap
  `_default` fallbacks for custom completers, plus appends a
  helper-functions block. Tab-completion now offers:
  - **Pinned package names** for `cheni unpin <Tab>` (reads
    `package-pins.json`).
  - **Frozen package names** for `cheni unfreeze <Tab>` and
    `cheni freeze <Tab>` (reads `package-freezes.json`).
  - **Generation numbers** for `cheni rollback <Tab>`,
    `cheni diff <from> <to>`, and `cheni history --delete <Tab>`
    (lists `/nix/var/nix/profiles/system-N-link`, newest first).
  - **Module categories** for `-c / --category` flags on
    `cheni check` and `cheni pin` (auto-detected from
    `modules/`).
  Backed by a hidden `cheni __complete <kind>` helper that prints
  one candidate per line — fast enough for per-Tab calls (no
  network, no eval). bash and fish keep clap_complete's static
  output as before; the dynamic infrastructure can be ported to
  them on demand.

### Documentation
- **`cheni --help` after-help block restructured**. The single
  "Common workflows" dump (40+ lines) is now split into six purpose-
  sorted sections: *Daily flow* (interactive menu, check, upgrade,
  status), *Per-package policy* (pin/freeze/clean), *Build vs
  upgrade cheat sheet* (now including `--boot`), *History &
  rollback* (collapsed redundant `--delete` examples),
  *Discovery* (search, why), *Maintenance* (doctor, self-update,
  verify, diagnose), *Environment*. Each line is one verb with one
  purpose, scanned at a glance.

### Changed
- **Interactive menu banner** now shows a multi-line "where am I"
  snapshot before the action picker. Adds the nixpkgs floor age
  (yellow + actionable hint when ≥3 days), an active-freezes count
  on the same line as pins, a `flake.lock: dirty` warning when
  uncommitted bumps are pending, and a "cheni vX.Y.Z available"
  line when the cache reports a newer release. The user picks an
  action with the current state in mind instead of having to run
  `cheni status` separately first.

### Added
- **`cheni doctor` gains three checks** that close gaps surfaced
  during the v0.5.0–v0.5.6 cycle:
  - **nixpkgs floor age** — tighter thresholds than the generic
    "input > 30 days" check (3 days = "due", anything past = warn).
    Stale nixpkgs invalidates the assumption of every other check
    that says "you're up to date", so it gets its own line near the
    top of the report.
  - **flake.lock dirty** — same trap surfaced by `cheni upgrade`'s
    preflight, escalated to a doctor-level check so a passive
    "is my setup healthy?" run catches it before the next rebuild
    silently applies all the pending bumps.
  - **cheni release available** — sync read of the cache filled by
    `cheni check`'s async self-update probe. Closes the loop
    started in v0.5.3 (check) / v0.5.4 (status): now all three
    "where am I" surfaces (check, status, doctor) reflect the same
    answer with no extra network calls.

## [0.5.6] — 2026-04-27

### Added
- **Freshness signals in `cheni check`**. Two new pieces of context
  at the top of the report so a "no updates available" line can no
  longer hide stale data:
  - `nixpkgs floor: Xd ago (rev …)` header — shows how old the
    user's `nixpkgs` flake input is, with an actionable hint
    (`→ run cheni upgrade`) when ≥3 days. The Repology section
    answers "what has upstream shipped" but the floor explains
    *what nixpkgs the comparison is against*.
  - Per-input local age column (`today` / `1d ago` / `Nd ago`) in
    the Flake inputs block, mirroring `cheni status`. So a user
    who runs `check` daily can spot drift across all inputs at a
    glance, not just nixpkgs.
- **Centralised User-Agent + sentinel test** (`crate::http::USER_AGENT`).
  The v0.5.5 incident — a hardcoded `cheni/0.1` blocklisted by Repology
  — turned out to also live in `nix/flake.rs`'s GitHub/GitLab probe
  (same prototype-era literal). Both call sites now reference the
  central constant, joined by the existing two in `release.rs` and
  `cmd/search`. The new `no_hardcoded_user_agent_outside_http_module`
  test in `src/tests/http.rs` walks every `.rs` file under `src/` and
  fails CI if any `.user_agent(` call doesn't go through the central
  constant.
- **"All Unknown" runtime guard** in `cheni check`. When the report
  shows zero classified packages (Up-to-date + Minor + Major + Newer)
  with 10+ Unknown, cheni now prints a yellow warning suggesting
  `cheni check -v --refresh` to inspect the real HTTP status. The
  threshold spares minimal configs (< 10 packages) where all-Unknown
  is plausibly real. Catches future API breakages of any flavour
  (UA block, IP ban, TLS fingerprint filter, full outage) — the
  v0.5.5 silent-fail signature would have been a loud yellow line
  with this guard in place.

## [0.5.5] — 2026-04-25

### Fixed
- **Repology lookups returning Unknown for everything**. The
  Repology client was sending `User-Agent: cheni/0.1` — a hardcoded
  prototype-era string that lasted in `api/repology.rs` while every
  other HTTP path (release, search) had moved to
  `concat!("cheni/", env!("GIT_DESCRIBE"))`. Repology eventually
  blanket-blocked that User-Agent with HTTP 403, and `cheni check`
  silently classified every package as "Unknown" because the HTML
  403 body failed to deserialise as JSON in `parse_response`. Two
  fixes: User-Agent now carries the live `git describe` like the
  other clients, and `query_one` short-circuits on any non-2xx
  status into a clean Unknown classification with the real status
  in the debug log instead of a misleading parse-error trail.

## [0.5.4] — 2026-04-25

### Added
- **`cheni status` echoes the self-update hint** in its Suggestions
  block when the cache (filled by `cheni check`) reports a newer
  release. Sync read — status never hits the network on its own,
  so the line only surfaces after a recent `cheni check`. Closes
  the loop B from the previous release: now both "where am I"
  surfaces (status + check) reflect the same answer.

### Changed
- **Kernel + linux-firmware now surface as real packages** in
  `cheni upgrade` preview and `cheni check --pending`. The artefact
  filter used a blanket `linux-` prefix that was eating the bare
  kernel (`linux-zen-6.19.12` → name="linux-zen") along with
  `linux-firmware` and `linux-pam`. Replaced with a version-suffix
  discriminant: `-modules`, `-shrunk`, `-modules-shrunk` in the
  version segment route the entry to the artefact bucket;
  everything else stays a real package. Result: kernel bumps now
  show up as user-visible changes in the preview, instead of being
  hidden behind an artefacts tally.

## [0.5.3] — 2026-04-25

### Added
- **`cheni check` surfaces a self-update hint** when the user pinned
  cheni at a release tag (`gitlab:harrael/cheni/vX.Y.Z`) and a newer
  release shipped on GitLab. One-line invitation to run
  `cheni self-update`, printed at the very tail of the report.
  Cached for 24h in `~/.cache/cheni/self-update-check.json` so the
  GitLab tags API isn't hit on every `cheni check`. Silent on cache
  miss + offline / rate-limited.
- **`cheni upgrade --boot`** stages the new generation for next
  boot via `nh os boot` instead of live-switching via `nh os
  switch`. Required when a critical component is changing
  (dbus → dbus-broker, init swap, …) — nh's pre-switch check
  refuses the live activation in those cases. The flag is
  available on the interactive menu via the existing Upgrade
  entries.
- **Auto-detection of critical-component changes** during the
  upgrade preview. Today: flags `dbus-broker` landing in either
  the build or fetch bucket (the swap that triggered the
  Pre-switch check error in the v0.5.0 → v0.5.2 development
  cycle). When detected and `--boot` is not already set, cheni
  prints a yellow warning and offers to flip to boot mode for
  this rebuild — saving the user the post-failure debug cycle
  of "switch refused → reboot does nothing → manual `nh os
  boot` → reboot again".

### Documentation
- **Release gate now requires `nix flake check`**. RELEASING.md
  promotes the build/clippy/test trio to a quartet that includes
  the sandboxed flake check, and `cheni-release-manager.md` makes
  it non-skippable. The v0.5.1 → v0.5.2 cycle hit a regression
  exactly because git wasn't in the sandbox PATH and the local gate
  didn't catch it; the stricter gate prevents the recurrence.

## [0.5.2] — 2026-04-25

### Added
- **`Pre-switch check` pattern** in `cheni build` and `cheni
  diagnose`. When `nh os switch` (via `cheni build`/`cheni
  upgrade`) refuses the live activation because a critical
  component is moving (dbus → dbus-broker, sysvinit → systemd,
  pulseaudio → pipewire, …), cheni now extracts the actual
  change line and points at the canonical recovery path: `sudo
  nh os boot ~/nixos-config && sudo reboot`. Previously this
  case fell through to two unrelated false-positive matches
  (`aes_generic`, generic systemd-service-failed) which weren't
  actionable.
- **`cheni self-update` now bumps tag-pinned cheni inputs**. When
  the user's `flake.nix` pins cheni at a specific release tag
  (`gitlab:harrael/cheni/vX.Y.Z`), self-update queries GitLab for
  the latest release and rewrites the URL to the new tag before
  running `nix flake update cheni`. Previously, `nix flake update`
  alone re-resolved the same tag and reported "already up to
  date", so tag-pinned setups stayed stuck on the original
  version forever. Branch-tracking pins (`gitlab:harrael/cheni`,
  `…/main`) are unaffected — they bump on `nix flake update` as
  before. Anti-downgrade guard: the bump only fires when the
  reported latest is strictly newer than the current pin.

### Fixed
- **`flake.nix` adds `git` to `nativeCheckInputs`** for the Nix
  sandbox check phase. The pin/freeze time-travel tests (added in
  v0.5.0) spawn `git init`/`add`/`commit` through fixture helpers;
  they passed locally (shell PATH has git) but failed in the Nix
  sandbox where PATH is empty. Adding `pkgs.git` to
  `nativeCheckInputs` makes `nix flake check` green without leaking
  git into the runtime closure. This fix unblocks `nh os switch`
  for any user who pinned cheni at v0.5.1 and ran `nix flake check`.

## [0.5.1] — 2026-04-25

### Added
- **`cheni check --pending`**: appends a closure dry-run section to
  the regular Repology view, listing what would actually rebuild at
  the next `cheni upgrade` or `cheni build`. Surfaces kernel + base
  nixpkgs packages + transitive deps that the upstream-named
  Repology scan can't see by construction. The two views are
  intentionally separate — `check` answers "is upstream ahead of
  what nixpkgs ships?", `--pending` answers "what would my next
  rebuild change?" — and combining them costs ~30s of evaluation.

### Documentation
- CLAUDE.md gains an explicit scope statement (personal tool,
  no community distribution, no upstream nh contributions).
- README intro rewritten to clarify what cheni is and is not.

## [0.5.0] — 2026-04-25

Breaking refactor: `cheni update` is removed. The "apply pins"
workflow now lives behind `cheni upgrade --pins-only`, and every
upgrade run surfaces the dirty-`flake.lock` trap up front.

### Breaking
- **`cheni update` removed.** Replaced by `cheni upgrade --pins-only`.
  The two commands had heavily overlapping semantics — both rebuilt,
  both followed `nh os switch`, both differed only in the scope of
  the input refresh. The merged surface is one verb (`upgrade`) with
  a scope flag, mirroring `nix flake update [<input>]`.
- The `cheni up` shell alias is gone with it. Update any
  alias / hooks pointing at `cheni update` to
  `cheni upgrade --pins-only` (or drop the `--pins-only` to get
  the full upgrade behaviour).

### Added
- `cheni upgrade --pins-only`: refresh `nixpkgs-latest` only, run
  the anti-downgrade check (was specific to the old `update`),
  preview, rebuild, clean obsolete pins. Equivalent to the old
  `cheni update` plus the upgrade preview/cleanup.
- **Dirty-`flake.lock` warning** at the start of every upgrade. If
  a previous upgrade was cancelled at the preview prompt, the lock
  file is already updated on disk while the rebuild didn't happen.
  Any subsequent rebuild — even a `--pins-only` one — applies all
  those pending bumps. The warning explains the trap and points at
  `git checkout flake.lock` to discard them.
- Cross-context wrappers (six in total: `history` annotation,
  `rollback` policy-drift, `history --delete` policy-loss,
  `search` Repology + badges, `diff` policy-delta header, `build`
  pre-flight). Each surfaces pins/freezes state that `nh` cannot
  see, on flows that were previously plain wrappers.

### Fixed
- `cheni upgrade` step 1 streams `nix flake update` events live so
  the user sees per-input bullets as they arrive instead of staring
  at `[1/4] Updating flake inputs` for the duration of a network
  fetch.
- `cheni search` columns now align regardless of name length —
  ANSI escapes from `colored` no longer confuse the format-string
  width.

## [0.4.1] — 2026-04-20

UX polish across every long-running command. No breaking changes.

### Added
- `cheni check` shows a real progress indicator instead of a static
  spinner: `Repology 12/45  ·  flake inputs …` updates in place,
  driven by an `AtomicUsize` counter that Repology bumps on every
  cache hit or API resolution.
- Every long-running command now reports elapsed time:
  `cheni build`, `cheni update`, `cheni upgrade`, `cheni self-update`,
  `cheni rollback`.
- **No-op warnings before the confirmation prompt** in `cheni upgrade`,
  `cheni update`, and `cheni self-update`. When the tool can predict
  that the rebuild will be pure re-eval noise (flake inputs
  unchanged + either dirty git tree or already-applied pins), it
  says so above the `[Y/n]` with an actionable opt-out.

### Changed
- `cheni upgrade` preview collapses home-manager / nixos-system
  artefacts (options.json, hm_.manpath, user-environment, …) into
  a single dimmed summary line instead of listing them individually.
  Real packages stay one line each.
- Final summaries are truthful: `19 packages changed (19 new)` is
  gone. Artefact-only rebuilds now say `no user-facing package
  changes (N system artefacts rebuilt)` or `nothing changed`, with
  a one-line explanation of *why* (dirty tree / home-manager
  re-eval).
- Step numbering unified across `upgrade` / `update` / `self-update`:
  no more duplicate `[3/4] Rebuilding system` followed by `[2/4]
  Rebuilding system...` from an inline label that wasn't removed
  when the readability overhaul landed.
- `cheni rollback` uses `util::confirm` instead of calling
  `dialoguer::Confirm` directly — only remaining direct-dialoguer
  caller, cleanup for consistency.

## [0.4.0] — 2026-04-20

Desktop-user feature bump. Notable:

### Added
- **`cheni freeze <pkg> --major N`** — tracks the latest `N.y.z`
  instead of strict-locking one version. `cheni upgrade` bumps the
  frozen rev to today's nixpkgs when upstream is still on major N,
  and holds it once upstream moves to N+1 (with a visible warning).
  Strict locks (no `--major`) unchanged — the flag is additive.
- **11 new `cheni diagnose` patterns** — including unfree, broken,
  collision, home-manager file conflict, GitHub rate limit, OOM
  (exit 137), DNS, syntax error, option-type mismatch, systemd
  activation failure, bootloader install, untrusted substituter,
  `dependencies couldn't be built`, sandbox violation, and the FD-
  exhaustion `Too many open files`. Catalogue: 17 → 33 patterns.

### Changed
- `cheni upgrade` grows a step 1b ("Freeze refresh") between flake
  update and preview. Reports per-entry whether a constrained
  freeze was bumped, held (upstream moved past the major), up to
  date, or unknown (offline/eval failure). Non-fatal — network
  issues don't block the upgrade.
- `nix::flake::fetch_commit_info` now honors `Retry-After` on 429
  responses from GitHub/GitLab. Same policy as the Repology
  client, shared via `crate::http::parse_retry_after`.

### Internal
- HTTP helpers (`http_timeout`, `check_content_length`,
  `verify_body_size`, `parse_retry_after`, `MAX_BODY_BYTES`,
  `RATE_LIMIT_*`) moved from `api/net` to `crate::http`. Closes the
  cross-sibling `nix/` → `api/` layering warning flagged by the
  post-v0.2.0 review. `api::net` → `crate::http` for all consumers.
- `util::confirm`, `util::tree_glyph`, `util::format_ymd` /
  `format_ymd_hm` extracted. Removes ~80 lines of duplication across
  `pin`, `freeze`, `unfreeze`, `upgrade`, `history`.
- `nix::store::find_by_name` and `nix::flake::short_hash` promoted
  to `pub(crate)` so commands stop copy-pasting the same helpers.
- `cmd::freeze::freeze_one` split into `gather_freeze_context` +
  `apply_freeze` to keep the orchestrator under 30 lines.
- `FreezeEntry.major_constraint: Option<u32>` added (serialised as
  `majorConstraint`, `skip_serializing_if = Option::is_none` so
  strict-lock entries stay byte-identical on round-trip).
- `nix::flake::query_pkg_version_at_rev` queries a package's
  `.version` at a specific nixpkgs rev via `nix eval --raw --expr`
  (pure — system injected from `std::env::consts`, `--impure` not
  used; fetchTree content-addressed by narHash).

### Security
- `nix eval` in the freeze-refresh path runs pure. `--impure` was
  dropped after the security audit flagged it as an unnecessary
  capability (only used for `builtins.currentSystem` — now we
  resolve the system from Rust directly).
- `freezes::validate_entry` rejects `majorConstraint > 9999` as
  defence-in-depth against payload edits to
  `package-freezes.json`.
- `DIAGNOSE.md` catalogue now lists 33 patterns.

### Fixed
- `src/tests/util.rs` atomic-write tests had been silently clobbered
  in the consolidation pass; restored. +3 tests.

## [0.3.0] — 2026-04-20

Desktop-user quality-of-life pass on top of v0.2.0, plus the
`freeze`/`unfreeze` commands — the semantic inverse of `pin`.
No breaking changes; every item below is additive or a
readability improvement to an existing command.

### Added
- **`cheni freeze <pkg>` / `cheni unfreeze <pkg|--all>`** — hold a
  package at its **current** nixpkgs revision while the rest of the
  system continues to move, the inverse of `cheni pin` (which routes
  through `nixpkgs-latest` to get a *newer* version). Uses a new
  `package-freezes.json` + a data-driven overlay that calls
  `builtins.fetchTree` with a pinned `rev + narHash` — no
  per-package flake input added, `flake.lock` stays clean, the
  overlay degrades to identity when the JSON file is missing.
  `cheni freeze` with no arg lists active freezes; `cheni status`
  grows a "Freezes" section; `cheni check` skips frozen packages
  from the Repology comparison and surfaces them in a dedicated
  "Frozen (held at their snapshot)" block; `cheni doctor` validates
  each entry's `rev`/`narHash` shape and flags orphans.
- **`cheni diagnose [file]`** — scan a rebuild log (from a path or
  stdin) and surface known-issue hints in a `what / why / fix`
  format. Starts with five curated patterns (`aes_generic`,
  fixed-output hash mismatch, `No space left on device`, missing
  flake attribute, infinite recursion); the list grows one entry at
  a time as real logs cross our desks.
- **Diagnose hints injected on rebuild failure** — when
  `cheni upgrade` or `cheni self-update` fails, the raw failure is
  followed by a compact postscript listing any patterns the
  diagnose library recognised. No extra flag; the user sees
  actionable hints automatically instead of a wall of store paths.

### Changed
- **`cheni upgrade` preview is now dense** — store names replaced
  by `name   old → new  [tag]`, with a per-section aggregate
  (`2 major, 8 minor, 9 patch, 1 new`). Major bumps colour-flagged
  so they're hard to miss.
- **`cheni rollback` shows a from→to preview and asks confirmation**
  — current gen + target gen with date and NixOS label, explicit
  direction ("moving back N generations"), reminder that the
  current generation stays reachable until the next GC. Invalid
  targets fail upfront with a clear message instead of a cryptic
  nix-env error. `-y / --yes` bypasses the prompt.
- **`cheni history --gc` and `cheni upgrade --gc` preview first** —
  `nix-collect-garbage --dry-run` runs as the user (no sudo), the
  count of paths about to disappear is displayed, and the user is
  asked to confirm before the real sudo GC step kicks in.
- **`cheni upgrade` and `cheni self-update` prettify nh output live**
  — `/nix/store/<hash>-` prefixes are stripped on every line before
  display, merged through a single OS pipe so stdout/stderr stay in
  emission order. `cheni build` gains the same prettification on
  its existing stderr streamer. The raw bytes are still captured
  for the structured error parsers.
- **`cheni why` renders as a tree** — Unicode box-drawing
  characters (`├── └── │`) replace flat indentation. Matches carry
  a short role tag when the line is unambiguous: `[enabled]`,
  `[disabled]`, `[system]`, `[home]`.

### Internal
- New `src/output/` module with `prettify_line` and a merged-pipe
  runner (`run_streaming`) used by the commands that shell out to
  `nh`. `os_pipe = "1"` added as a dep (pure Rust, no FFI).
- New `src/nix/gc.rs` wrapping the `nix-collect-garbage --dry-run`
  preview and its pure `parse_path_count` helper.
- `nix::store::split_name_version` promoted to `pub(crate)` so the
  upgrade preview can reuse the existing name/version parser.
- `cmd::history::read_generations` and `Generation` exposed at
  crate visibility to power `cheni rollback`'s preview.
- Six new test files (`output/tests/prettify.rs`,
  `nix/tests/gc.rs`, `cmd/tests/diagnose.rs`,
  `cmd/tests/why.rs`, `cmd/tests/rollback.rs`,
  `cmd/tests/upgrade.rs`), plus fixtures in several existing
  files. Total test count crossed 380.

## [0.2.0] — 2026-04-19

### Added
- **`cheni verify [--tag v…]`** — read-only signature check on the
  installed cheni (or any tag). Works anywhere, doesn't touch the
  flake or the Nix store.

### Fixed
- **Self-update crash on second upgrade** — `reqwest::blocking`
  inside the tokio async runtime was crashing at drop ("Cannot
  drop a runtime in a context where blocking is not allowed"). The
  verification path is now async end-to-end via `reqwest::Client`.

### Internal
- Verification primitives (public key embedding, URL derivation,
  signature check, dev-suffix stripping) extracted from
  `cmd/self_update` into a new top-level `release` module.
  `cmd::self_update` and `cmd::verify` share the same trust anchor
  and behaviour.

## [0.1.0-beta] — 2026-04-19

First release with signed tarballs. Signature verification is
still soft on this version (the in-flight cheni v0.1.0-alpha
doesn't know how to verify yet); from v0.1.0-beta onwards,
`cheni self-update` verifies by default.

### Added
- **Signed releases via minisign** — every tagged release tarball
  gets a `.minisig` asset published to the GitLab release page.
  Public key checked in at `public-keys/cheni-release.pub`
  (fingerprint `358A303A12B2640B`) and embedded in the cheni
  binary at compile time.
- **`cheni self-update` verifies the signature** — between
  `nix flake update cheni` and `nh os switch`, the new release's
  tarball and `.minisig` are downloaded and verified against the
  embedded public key. `--allow-unsigned` is the documented
  escape hatch for key rotation, local dev, or transitional
  upgrades from versions that don't expose a `ref` in
  `flake.lock`.
- **SECURITY.md** — user-facing threat model, manual verification
  procedure, and compromise-response plan.

### Changed
- **Retry-After honored on Repology 429s** — the fixed 3s wait is
  now a fallback; a server-supplied value in `[1, 30]` seconds is
  respected. Anything outside the range (or missing) still falls
  through to the default.
- **HTTP body cap of 5 MiB** applied to every
  Repology/GitHub/GitLab response via new
  `api::net::check_content_length` + `verify_body_size` helpers.
- **`$USER` / `$LOGNAME` sanitised** before splicing into
  `/etc/profiles/per-user/...` paths — defence-in-depth against
  environment tampering.
- **Sudo and `nix-store` call sites routed through `tool_error`**
  so a missing binary produces the same actionable install hint
  as the rest of the codebase.
- **Tests extracted from every remaining inline `mod tests`**
  block (cmd/obsolete, version/parse, version/compare) into
  sibling files, matching the project-wide convention.

### Internal
- Seven project-scoped Claude Code agents committed under
  `.claude/agents/` (`cheni-release-manager`,
  `cheni-code-reviewer`, `cheni-security-auditor`,
  `cheni-nix-integration`, `cheni-repology-debugger`,
  `cheni-flake-maintainer`, `cheni-test-author`) so future
  sessions inherit the project conventions instead of re-deriving
  them.

## [0.1.0-alpha] — 2026-04-14

First tagged release. The "Added" list below covers what shipped
with the initial version.

### Added
- **`cheni completion <shell>`** — emit bash / zsh / fish / elvish /
  powershell completion scripts on stdout. Pipe into your shell's
  completion dir (e.g. `cheni completion fish > ~/.config/fish/completions/cheni.fish`).
- **`cheni man`** — emit a roff man page on stdout. Pipe into a
  `man1/` directory to get `man cheni` working.
- **Version reflects every commit** — `cheni --version` now displays
  `0.1.{count}-alpha ({short-hash})` where `{count}` is
  `git rev-list --count HEAD`. Computed at compile time from `build.rs`;
  `Cargo.toml` stays at the static literal that Cargo requires.

### Fixed
- `cheni self-update` no longer claims "New version: <old version>"
  — the in-flight binary is still the old one until the user opens a
  new shell. Prints a hint to run `cheni --version` in a new shell
  instead.

### Internal
- Readability pass across `run()` and other large functions in
  `cmd/{check,history,build,pin,status,upgrade,init,update,bug_report,
  why,search,doctor}.rs`, `nix/{flake,store,config}.rs`. Tests gained
  coverage for the newly-extracted helpers (`aggregate_versions`,
  `is_revision_outdated`, `find_nixpkgs_insert_line`,
  `build_content_with_latest_input`, `parse_and_sort_results`,
  `relevance_rank`). Test count 80 → 87.
- `FlakeInput.last_modified` dropped (genuinely unused);
  `FlakeInput.days_old` kept (read by `cheni doctor`).
- `debug_assert!` removed from `pick_highest_version` — it
  contradicted the documented empty-slice contract and broke
  `cargo test`.

### Added (earlier in this cycle)
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
