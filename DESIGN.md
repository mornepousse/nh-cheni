# nixup — Design Document

## Vision

Make NixOS package management accessible to everyone.

NixOS is powerful but its UX is hostile:
- No way to update a single package — it's all or nothing
- Build errors are walls of cryptic nix store paths
- No visual overview of what's installed or outdated

nixup is a CLI tool that fixes this. It's humble, utilitarian, and works
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
package means updating everything. nixup solves this with a second input:

```
nixpkgs          (Apr 1)  →  everything
nixpkgs-latest   (Apr 17) →  only pinned packages (via overlay)
```

`package-pins.json` lists which packages come from `nixpkgs-latest`:
```json
["legcord", "cmake"]
```

When the user runs `upgrade` later and `nixpkgs` catches up, nixup
auto-cleans obsolete pins.

### Flake input packages

Packages from flake inputs (zen-browser, affinity, claude-code) are
handled differently — `nixup pin zen-browser` runs
`nix flake update zen-browser` instead of adding an overlay pin.
Same UX for the user, different mechanism under the hood.

## Commands

### `nixup check`
Show available updates. Read-only, no side effects.

```
$ nixup check

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
$ nixup check --dev       # only packages from modules/dev/
$ nixup check --apps      # only packages from modules/apps/
```

The `--dev`, `--apps`, etc. flags are auto-detected from the `modules/`
directory structure. If a user has `modules/gaming/`, then `--gaming` works.

### `nixup pin <pkg>`
Pin a package to nixpkgs-latest (or update its flake input).

```
$ nixup pin legcord
Pinned legcord (nixpkgs-latest)
Run 'nixup update' to apply.

$ nixup pin zen-browser
Pinned zen-browser (flake input)
Run 'nixup update' to apply.
```

Pin by module directory with grouped confirmation:
```
$ nixup pin --dev

Minor updates (safe):
  gcc-arm-embedded   14.2.0  →  14.2.1
  cmake              3.28.0  →  3.29.0
  openocd            0.12.0  →  0.12.1

Pin 3 minor updates? [Y/n] y

Major updates (breaking changes possible):
  kicad              9.0.2   →  10.0.1

Pin 1 major update? [y/N] n

Pinned 3 packages.
Run 'nixup update' to apply.
```

### `nixup unpin <pkg>`
Remove a pin.

```
$ nixup unpin legcord
Unpinned legcord.

$ nixup unpin --all
Removed 5 pins.
```

### `nixup update`
Apply all current pins: update nixpkgs-latest + rebuild.

```
$ nixup update

[1/3] Updating nixpkgs-latest...
[2/3] Updating flake inputs: zen-browser...
[3/3] Rebuilding system...

3 packages updated successfully.
```

### `nixup init`
First-time setup. Modifies the user's flake.nix.

```
$ nixup init

Detected flake at ~/nixos-config
Hostname: morthinkpad

[1/3] Adding nixpkgs-latest input...        OK
[2/3] Adding overlay to nixosConfigurations... OK
[3/3] Creating package-pins.json...         OK

Done! You can now use 'nixup check' and 'nixup pin'.
```

If auto-modification fails (exotic flake structure), falls back to
printing manual instructions.

### `nixup status`
Show current state: active pins, generations.

```
$ nixup status

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

nixup finds the NixOS config in this order:
1. `$NIXUP_CONFIG` environment variable (if set)
2. Current directory (if it contains a flake.nix with nixosConfigurations)
3. `~/nixos-config`
4. `/etc/nixos`

Hostname is detected via `hostname` command and matched against
`nixosConfigurations` in the flake. If no match, nixup asks the user.

## Package Name Resolution

Store path names don't always match Repology names. Resolution cascade:

1. Try the store name directly on Repology (fast, works ~80%)
2. If not found → `nix eval nixpkgs#<name>.name` to get the real attr (slow, cached)
3. If still not found → show "unknown"

Results are cached on disk (~/.cache/nixup/versions.json, 1h TTL).

## Pin Auto-Cleanup

After a system `upgrade` (when nixpkgs is updated), nixup checks if
`nixpkgs` has caught up with `nixpkgs-latest`. If so, obsolete pins
are removed automatically:

```
Cleaned 3 obsolete pins (nixpkgs caught up)
```

## Architecture

```
nixup/
├── src/
│   ├── main.rs              # CLI entry (clap), subcommand routing
│   ├── cmd/                 # One file per command
│   │   ├── mod.rs
│   │   ├── check.rs         # nixup check
│   │   ├── pin.rs           # nixup pin / unpin
│   │   ├── update.rs        # nixup update
│   │   ├── init.rs          # nixup init
│   │   └── status.rs        # nixup status
│   ├── nix/                 # NixOS system interaction
│   │   ├── mod.rs
│   │   ├── store.rs         # Read installed packages from store
│   │   ├── config.rs        # Detect flake, hostname, modules
│   │   ├── flake.rs         # Parse/modify flake.nix, read inputs
│   │   └── pins.rs          # Read/write package-pins.json
│   ├── api/                 # External data sources
│   │   ├── mod.rs
│   │   ├── repology.rs      # Repology API client
│   │   └── cache.rs         # On-disk cache
│   └── version/             # Version logic
│       ├── mod.rs
│       ├── parse.rs         # Parse version strings
│       └── compare.rs       # Major/minor detection
├── Cargo.toml
├── flake.nix
├── DESIGN.md
└── README.md
```

## Future Modules (v2+)

### Build Error Parser
Wrap `nh os switch` / `nixos-rebuild` and translate cryptic errors:
- Hash mismatch → show expected vs got, suggest `--update-hash`
- Missing dependency → suggest package to add
- Eval error → show file + line + context
- Unfree → suggest `allowUnfree` or per-package override

### System Visualizer
Human-readable view of generations and packages:
- Generation history with diffs (what changed)
- Package tree grouped by module
- Size tracking

### TUI Mode
Visual layer on top of the CLI commands:
- `nixup tui` — interactive package manager
- Uses the same logic as CLI commands
- Multi-select, search, detail views

## Non-goals

- Replace nix/nixos-rebuild (nixup wraps them)
- Package installation GUI (declarative config is the NixOS way)
- Work on non-NixOS systems
- Work without flakes (channels not supported)
