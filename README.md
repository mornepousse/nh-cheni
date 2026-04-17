# nixup

Granular package updates for NixOS.

On NixOS, updating one package means updating everything. nixup fixes this
by letting you check, select, and apply updates per-package — integrated
with your flake configuration.

## Quick Start

```bash
nixup init              # one-time setup (adds nixpkgs-latest to your flake)
nixup check             # see what's outdated
nixup pin legcord       # pin a package to the latest version
nixup update            # apply pins (rebuild system)
```

## Commands

| Command              | Description                                      |
|----------------------|--------------------------------------------------|
| `nixup check`        | Show available updates                           |
| `nixup check --dev`  | Show updates for packages in `modules/dev/` only |
| `nixup pin <pkg>`    | Pin a package to nixpkgs-latest                  |
| `nixup pin --dev`    | Pin all minor updates in `modules/dev/`          |
| `nixup unpin <pkg>`  | Remove a pin                                     |
| `nixup unpin --all`  | Remove all pins                                  |
| `nixup update`       | Apply pins (update nixpkgs-latest + rebuild)     |
| `nixup init`         | First-time setup                                 |
| `nixup status`       | Show active pins and system info                 |

## How It Works

nixup adds a second `nixpkgs` input (`nixpkgs-latest`) to your flake.
When you pin a package, it gets pulled from `nixpkgs-latest` via an
overlay. Everything else stays on your regular `nixpkgs`.

When you do a full system `upgrade` later, `nixpkgs` catches up and
nixup auto-cleans the obsolete pins.

For flake input packages (zen-browser, etc.), `nixup pin` updates the
flake input directly instead.

## Version Safety

nixup distinguishes between:
- **Minor updates** (1.2.0 → 1.3.0) — safe, selectable with `nixup pin`
- **Major updates** (9.0 → 10.0) — breaking changes possible, requires `--force`

```
$ nixup check

nixpkgs:
  legcord          1.1.0  →  1.2.2     (minor)
  kicad            9.0.2  →  10.0.1    (major)
```

## Requirements

- NixOS with flakes enabled
- A flake-based configuration

## Install

```bash
# Try it
nix run gitlab:harrael/nixup

# Or add to your flake
{
  inputs.nixup.url = "gitlab:harrael/nixup";
}
```

## Status

Early development. See [DESIGN.md](DESIGN.md) for the roadmap.

## License

MIT
