# cheni

Granular package updates for NixOS.

On NixOS, updating one package means updating everything. cheni fixes this
by letting you check, select, and apply updates per-package — integrated
with your flake configuration.

## Quick Start

```bash
cheni init              # one-time setup (adds nixpkgs-latest to your flake)
cheni check             # see what's outdated
cheni pin legcord       # pin a package to the latest version
cheni update            # apply pins (rebuild system)
```

## Commands

| Command              | Description                                      |
|----------------------|--------------------------------------------------|
| `cheni check`        | Show available updates                           |
| `cheni check --dev`  | Show updates for packages in `modules/dev/` only |
| `cheni pin <pkg>`    | Pin a package to nixpkgs-latest                  |
| `cheni pin --dev`    | Pin all minor updates in `modules/dev/`          |
| `cheni unpin <pkg>`  | Remove a pin                                     |
| `cheni unpin --all`  | Remove all pins                                  |
| `cheni update`       | Apply pins (update nixpkgs-latest + rebuild)     |
| `cheni init`         | First-time setup                                 |
| `cheni status`       | Show active pins and system info                 |

## How It Works

cheni adds a second `nixpkgs` input (`nixpkgs-latest`) to your flake.
When you pin a package, it gets pulled from `nixpkgs-latest` via an
overlay. Everything else stays on your regular `nixpkgs`.

When you do a full system `upgrade` later, `nixpkgs` catches up and
cheni auto-cleans the obsolete pins.

For flake input packages (zen-browser, etc.), `cheni pin` updates the
flake input directly instead.

## Version Safety

cheni distinguishes between:
- **Minor updates** (1.2.0 → 1.3.0) — safe, selectable with `cheni pin`
- **Major updates** (9.0 → 10.0) — breaking changes possible, requires `--force`

```
$ cheni check

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
nix run gitlab:harrael/cheni

# Or add to your flake
{
  inputs.cheni.url = "gitlab:harrael/cheni";
}
```

## Status

Early development. See [DESIGN.md](DESIGN.md) for the roadmap.

## License

MIT
