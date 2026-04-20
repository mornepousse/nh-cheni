# cheni — Design Document

## Vision

Make NixOS package management accessible to everyone.

NixOS is powerful but its UX is hostile:
- No way to update a single package — it's all or nothing
- Build errors are walls of cryptic nix store paths
- No visual overview of what's installed or outdated

cheni is a CLI tool that fixes this. It's humble, utilitarian, and works
with any flake-based NixOS configuration.

## Core Principles

1. **CLI-first** — every feature is a command, scriptable, composable. TUI later as a visual layer on top
2. **Config-integrated** — works WITH the user's flake, modifies it intelligently. The config is the source of truth
3. **Non-destructive** — always preview before applying, easy rollback, auto-cleanup
4. **Lightweight** — no nixpkgs evaluation for simple operations (Repology API + cache)
5. **Community-ready** — clean code, no hardcoded paths, works on any flake-based NixOS setup

## How it works

### The nixpkgs-latest mechanism

NixOS pulls all packages from a single `nixpkgs` snapshot. Updating one
package means updating everything. cheni solves this with a second input:

```
nixpkgs          (Apr 1)  →  everything
nixpkgs-latest   (Apr 17) →  only pinned packages (via overlay)
```

`package-pins.json` lists which packages come from `nixpkgs-latest`:
```json
["legcord", "cmake"]
```

When the user runs `upgrade` later and `nixpkgs` catches up, cheni
auto-cleans obsolete pins.

### Flake input packages

Packages from flake inputs (zen-browser, affinity, claude-code) are
handled differently — `cheni pin zen-browser` runs
`nix flake update zen-browser` instead of adding an overlay pin.
Same UX for the user, different mechanism under the hood.

## Commands

### `cheni check`
Show available updates. Read-only, no side effects.

```
$ cheni check

nixpkgs (47 packages):
  legcord          1.1.0  →  1.2.2     (minor)
  cmake            3.28.0 →  3.29.0    (minor)
  kicad            9.0.2  →  10.0.1    (major)

flake inputs:
  zen-browser      1.2.0  →  1.3.0     (zen-browser)
  claude-code      1.0.0  →  1.1.0     (claude-code)

Up to date: 41 | Minor: 2 | Major: 1 | Unknown: 2
```

Supports filtering by module directory:
```
$ cheni check -c dev      # only packages from modules/dev/
$ cheni check -c apps     # only packages from modules/apps/
```

The `--category <NAME>` argument takes any subdirectory name under
`modules/` — so `cheni check -c gaming` works without any code change
if the user has `modules/gaming/`.

### `cheni pin <pkg>`
Pin a package to nixpkgs-latest (or update its flake input).

```
$ cheni pin legcord
Pinned legcord (nixpkgs-latest)
Run 'cheni update' to apply.

$ cheni pin zen-browser
Pinned zen-browser (flake input)
Run 'cheni update' to apply.
```

Pin by module directory with grouped confirmation:
```
$ cheni pin -c dev

Minor updates (safe):
  gcc-arm-embedded   14.2.0  →  14.2.1
  cmake              3.28.0  →  3.29.0
  openocd            0.12.0  →  0.12.1

Pin 3 minor updates? [Y/n] y

Major updates (breaking changes possible):
  kicad              9.0.2   →  10.0.1

Pin 1 major update? [y/N] n

Pinned 3 packages.
Run 'cheni update' to apply.
```

### `cheni unpin <pkg>`
Remove a pin.

```
$ cheni unpin legcord
Unpinned legcord.

$ cheni unpin --all
Removed 5 pins.
```

### `cheni update`
Apply all current pins: update nixpkgs-latest + rebuild.

```
$ cheni update

[1/3] Updating nixpkgs-latest...
[2/3] Updating flake inputs: zen-browser...
[3/3] Rebuilding system...

3 packages updated successfully.
```

### `cheni init`
First-time setup. Modifies the user's flake.nix.

```
$ cheni init

Detected flake at ~/nixos-config
Hostname: morthinkpad

[1/3] Adding nixpkgs-latest input...        OK
[2/3] Adding overlay to nixosConfigurations... OK
[3/3] Creating package-pins.json...         OK

Done! You can now use 'cheni check' and 'cheni pin'.
```

If auto-modification fails (exotic flake structure), falls back to
printing manual instructions.

### `cheni status`
Show current state: active pins, generations.

```
$ cheni status

Config: ~/nixos-config (morthinkpad)
nixpkgs:        Apr 1, 2026  (4747257)
nixpkgs-latest: Apr 17, 2026 (4bd9165)

Active pins (3):
  legcord          1.2.2
  cmake            3.29.0
  openocd          0.12.1

Current generation: 142 (Apr 17, 2026)
```

## Config Detection

cheni finds the NixOS config in this order:
1. `$CHENI_CONFIG` environment variable (if set)
2. Current directory (if it contains a flake.nix with nixosConfigurations)
3. `~/nixos-config`
4. `/etc/nixos`

Hostname is detected via `hostname` command and matched against
`nixosConfigurations` in the flake. If no match, cheni asks the user.

## Package Name Resolution

Store names don't always map cleanly to Repology projects, and Repology
projects can contain many entries for the same nixpkgs channel. Two
related problems, two distinct passes:

**Project lookup** — picking the right Repology project URL:
1. Apply the small `NAME_MAPPINGS` table for known oddballs (Qt 6
   sub-packages → `qt`, terminal emulator name conflicts, etc.)
2. Otherwise try the store name directly. ~80% of packages just work.
3. On 404 → classified as "Unknown" (visible with `--details`).

**Entry selection** — once we've fetched the project page, picking the
right nix entry from it. The page can contain firefox + firefox-esr +
firefox-bin + firefox-mobile (all `visiblename: "firefox"`), or
`kdePackages.breeze-icons` + `libsForQt5.breeze-icons`. Cascade:
1. If the caller passed an installed-version hint, prefer the entry
   whose version matches exactly, then by major.
2. Otherwise, name-match by `srcname` → `binname` → `visiblename`.
3. Filter pre-release versions (`3.15.0a7`, `2.0-beta1`, ...) when
   the installed version is stable — Repology often returns the
   alpha/rc as "latest" otherwise.

Results are cached on disk (`~/.cache/cheni/versions.json`, 1h TTL,
written atomically). `cheni check --refresh` wipes the cache to force
re-fetch.

## Pin Auto-Cleanup

After a system `upgrade` (when nixpkgs is updated), cheni checks if
`nixpkgs` has caught up with `nixpkgs-latest`. If so, obsolete pins
are removed automatically:

```
Cleaned 3 obsolete pins (nixpkgs caught up)
```

## Architecture

```
cheni/
├── src/
│   ├── main.rs              # CLI entry (clap), subcommand routing
│   ├── lib.rs               # Library facade for tests and reuse
│   ├── util.rs              # atomic_write and other small utilities
│   ├── tests/               # unit tests for root modules (util.rs)
│   ├── cmd/                 # One file per command
│   │   ├── mod.rs
│   │   ├── check.rs         # cheni check
│   │   ├── bug_report.rs    # cheni bug-report
│   │   ├── pin.rs           # cheni pin / unpin
│   │   ├── update.rs        # cheni update
│   │   ├── upgrade.rs       # cheni upgrade (full system upgrade)
│   │   ├── build.rs         # cheni build (rebuild + error parsing)
│   │   ├── init.rs          # cheni init
│   │   ├── status.rs        # cheni status
│   │   ├── doctor.rs        # cheni doctor (health checks)
│   │   ├── search.rs        # cheni search (nix search wrapper)
│   │   ├── why.rs           # cheni why (find declaring .nix file)
│   │   ├── clean.rs         # cheni clean (obsolete pins)
│   │   ├── self_update.rs   # cheni self-update (verifies signature)
│   │   ├── verify.rs        # cheni verify (read-only signature check)
│   │   ├── diagnose.rs      # cheni diagnose (clarify rebuild logs)
│   │   ├── history.rs       # cheni history (list + --prune/--delete/--keep)
│   │   ├── rollback.rs      # cheni rollback (with from→to preview)
│   │   ├── diff.rs          # cheni diff <from> <to>
│   │   ├── interactive.rs   # menu when run with no subcommand
│   │   ├── obsolete.rs      # shared helpers for pin obsolescence
│   │   └── tests/           # unit tests per cmd module
│   ├── nix/                 # NixOS system interaction
│   │   ├── mod.rs
│   │   ├── store.rs         # Read installed packages from store
│   │   ├── config.rs        # Detect flake, hostname, modules
│   │   ├── flake.rs         # Parse flake.lock, check remote inputs
│   │   ├── pins.rs          # Read/write package-pins.json
│   │   ├── gc.rs            # nix-collect-garbage --dry-run preview
│   │   ├── tools.rs         # Friendly ENOENT → install-hint mapper
│   │   └── tests/           # unit tests per nix module
│   ├── api/                 # External data sources
│   │   ├── mod.rs
│   │   ├── net.rs           # HTTP timeout + body-size cap helpers
│   │   ├── repology.rs      # Repology API client (rate-limited)
│   │   ├── cache.rs         # On-disk cache (~/.cache/cheni)
│   │   └── tests/           # unit tests per api module
│   ├── output/              # Live output prettification
│   │   ├── mod.rs
│   │   ├── prettify.rs      # Strip /nix/store/<hash>- from a line
│   │   ├── stream.rs        # Spawn a child with merged stdout/stderr pipe
│   │   └── tests/
│   ├── release.rs           # Minisign signature verification
│   ├── version/             # Version logic
│   │   ├── mod.rs
│   │   ├── parse.rs         # Parse version strings (semver + calver)
│   │   └── compare.rs       # Major/minor/newer detection
│   └── tests/               # Unit tests for root-level modules
│       ├── util.rs
│       └── release.rs
├── public-keys/
│   ├── cheni-release.pub    # Trusted minisign public key
│   └── README.md            # Fingerprint + manual verification procedure
├── VERSION                  # Source of truth for the displayed version
├── Cargo.toml
├── flake.nix
├── build.rs                 # Reads VERSION + git describe at build time
├── DESIGN.md
├── SECURITY.md              # Threat model + verify procedure
├── RELEASING.md             # Release protocol (bump, sign, publish)
└── README.md
```

## Code Standards

### Readability
Code must be accessible for review by anyone. This means:
- Clear, descriptive variable and function names
- Comments explaining **why**, not what
- Each function does one thing
- No clever tricks — boring code is good code
- Public API documented with `///` doc comments
- Modules have a top-level `//!` doc comment explaining their purpose

### Testing
Unit tests live in sibling `tests/` directories, one file per source
module:

```
src/cmd/history.rs       ←→ src/cmd/tests/history.rs
src/nix/store.rs         ←→ src/nix/tests/store.rs
src/api/cache.rs         ←→ src/api/tests/cache.rs
src/util.rs              ←→ src/tests/util.rs
```

The source file ends with a three-line include:

```rust
#[cfg(test)]
#[path = "tests/<name>.rs"]
mod tests;
```

This keeps them as unit tests (same compile flags, access to private
items via `use super::*;`) while letting each source file stay short
and focused on production code. Format-fragile parsers — the nix
store diff-closures reader and the nh build-error matcher — have
dedicated regression fixtures so a change in upstream output fails
a test before it silently breaks cheni in the wild.

### Debugging
Three verbosity levels via `tracing` crate:
- Default: clean user-facing output only
- `-v` (debug): config detection, cache hits/misses, API calls, decisions
- `-vv` (trace): raw store paths, HTTP responses, version comparisons

Every decision point logs why it chose a path:
```rust
tracing::debug!("Package '{}': store={}  repology={} → minor update", name, installed, latest);
```

### Error Handling
- `anyhow` for application errors with context
- Zero `unwrap()` in prod paths — remaining `.expect()` calls assert
  true-by-construction invariants and include a diagnostic message
- Missing external tools (`nh`, `nix`, `nvd`, ...) go through
  `nix::tools::tool_error()` which turns the generic ENOENT into a
  targeted install hint with a copy-paste Nix config snippet
- Panic hook installed at `main()` entry: on any unexpected crash,
  prints the error + location and points the user at `cheni bug-report`
- User-facing errors are clear and actionable:
  ```
  Error: could not find flake.nix
  Hint: run 'cheni init' in your NixOS config directory
  ```

### Color Output
- Colored output by default (via `colored` crate)
- `--no-color` flag and `NO_COLOR` env var support
- Accessible: don't rely on color alone (use symbols too: ✓ ↑ ⚠ ?)

### Packaging
- `flake.nix` uses `cargoLock = { lockFile = ./Cargo.lock; }` rather than
  a manual `cargoHash`, so adding a Rust dep never requires a manual
  hash bump. Git or path sources would need `outputHashes` — none today.

## Versioning

Alpha releases — expect breaking changes. Versioning is calendar-ish for now;
the `0.1.0-alpha` series shipped the full feature set incrementally:
```
inspection      cheni check, status, doctor, search, why
pin lifecycle   cheni pin / unpin / clean
apply           cheni update, build (with error parser), upgrade (preview)
history         cheni history (list + diffs + prune/--delete/--keep/--older-than)
                cheni rollback, diff
self            cheni init, self-update
UX              interactive menu (cheni with no args)
```

Aim for `v1.0.0` once the API has settled, the test suite covers the
critical paths, and the `cheni init` flow has been validated on multiple
real-world flake layouts.

## Future ideas

### Multi-host support
Today cheni assumes one hostname per flake. Could grow to handle several
`nixosConfigurations` (laptop + desktop + server) sharing the same pin set
or scoped per host.

### Module-aware pin grouping
`cheni pin -c dev` already groups by `modules/dev/`. A natural extension is
named pin groups ("dev-toolchain", "design-apps") that can be applied or
unpinned together.

### Generation tagging / notes
Annotate a generation with a one-line note ("before kernel bump", "demo
config for talk") that surfaces in `cheni history`. Useful when keeping
many generations around for testing.

### Faster check
The Repology lookup is the dominant cost. Possible wins:
- Parallel batches with smarter rate-limiting
- Fall back to `nix-env -qa --json` for packages Repology doesn't know
- Persistent shared cache across machines

### TUI
The interactive menu already covers the "I don't remember the flag"
use case. A full TUI could add multi-select pinning, search-as-you-type
across the package list, and a live diff view.

## Non-goals

- Replace nix/nixos-rebuild (cheni wraps them)
- Package installation GUI (declarative config is the NixOS way)
- Work on non-NixOS systems
- Work without flakes (channels not supported)
