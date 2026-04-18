# cheni

> **Granular package updates for NixOS.**
> (_cheni_ — Swiss-French for "mess/clutter". This tool tidies up the clutter of Nix updates.)

On NixOS, updating one package means updating everything. `cheni` fixes
this: check, select, and apply updates **per-package** — fully integrated
with your flake configuration.

---

## What it does

```
$ cheni check

Flake inputs (updates available):
  affinity-nix             2.6.5-         UPDATE (latest: 2026-04-18)
  claude-code              2.1.114        ok
  kesp-controller          2.0.7          ok
  zen-browser              ?              UPDATE (latest: 2026-04-18)

Updates available:
  flatpak                  1.16.4         → 1.16.6         (minor)
  htop                     3.4.1          → 3.5.0          (minor)
  vivaldi                  7.9.3970.47    → 7.9.3970.50    (minor)

Major updates (use 'cheni pin --force' to apply):
  mesa                     24.3.2         → 26.0.4         (major)

Up to date: 116 | Minor: 3 | Major: 1 | Newer: 6 | Unknown: 11
```

Then pin what you want, and apply:

```
$ cheni pin flatpak vivaldi
$ cheni update
```

Only those two packages get updated. Everything else stays on your
current `nixpkgs`.

---

## Quick start

```bash
cheni init              # one-time flake setup
cheni                   # interactive menu (no subcommand → picker)
cheni check             # see what's outdated
cheni pin <package>     # pin a package for update
cheni update            # apply all pins (rebuild system)
```

Run `cheni` with no arguments for an interactive menu showing the
current state and a list of every command. Pick one with the arrow
keys; cheni prompts for any extra input it needs.

---

## Commands

### Inspection

| Command              | What it does                                     |
|----------------------|--------------------------------------------------|
| `cheni check`        | Show available updates (nixpkgs + flake inputs)  |
| `cheni check --dev`  | Show updates for packages in `modules/dev/` only |
| `cheni status`       | Show config path, active pins, input timestamps  |

### Pinning

| Command                   | What it does                                |
|---------------------------|---------------------------------------------|
| `cheni pin <pkg>`         | Pin a single nixpkgs package                |
| `cheni pin --dev`         | Pin all minor updates in `modules/dev/`     |
| `cheni pin --dev --force` | Include major updates (breaking changes)    |
| `cheni pin --flakes`      | Update all flake inputs (zen-browser, etc.) |
| `cheni unpin <pkg>`       | Remove a specific pin                       |
| `cheni unpin --all`       | Remove all pins                             |

### Apply

| Command         | What it does                                          |
|-----------------|-------------------------------------------------------|
| `cheni update`  | Apply pins: refresh `nixpkgs-latest` + rebuild        |
| `cheni build`   | Rebuild current state with human-readable errors      |
| `cheni upgrade` | Full upgrade: update all inputs, preview, build       |
| `cheni clean`   | Auto-remove obsolete pins (nixpkgs caught up)         |

### History & rollback

| Command                              | What it does                                         |
|--------------------------------------|------------------------------------------------------|
| `cheni history`                      | List recent generations + per-step package summary   |
| `cheni history --diff`               | Show full per-package diff between generations       |
| `cheni history --limit 30`           | Show more than the default 10 generations            |
| `cheni rollback`                     | Roll back to the previous generation                 |
| `cheni rollback 405`                 | Roll back to a specific generation                   |
| `cheni diff 405 409`                 | Compare two generations (uses `nvd` if available)    |
| `cheni history --prune`              | Pick generations to delete from a multi-select list  |
| `cheni history --delete 405 406`     | Delete specific generations                          |
| `cheni history --delete 400..410`    | Delete a range (inclusive)                           |
| `cheni history --keep 20`            | Keep only the 20 most recent                         |
| `cheni history --older-than 30d`     | Delete generations older than 30 days (d/w/m/y)      |
| `cheni history --keep 20 --gc`       | Also reclaim disk space after deletion               |

The active generation is always protected — cheni refuses to delete
the currently-booted system to keep rollback safe.

### Discovery

| Command              | What it does                                    |
|----------------------|-------------------------------------------------|
| `cheni search <q>`   | Search nixpkgs                                  |
| `cheni why <pkg>`    | Find which `.nix` file in your config declares it |

### Maintenance

| Command              | What it does                                      |
|----------------------|---------------------------------------------------|
| `cheni doctor`       | Health checks (paths, pins, flake, store access)  |
| `cheni self-update`  | Refresh the cheni flake input + rebuild           |
| `cheni init`         | One-time setup: add `nixpkgs-latest` to your flake |

---

## How it works

```
       ┌──────────────────────────────────────┐
       │ flake.nix                             │
       │                                       │
       │   nixpkgs         ────► most packages │
       │   nixpkgs-latest  ────► pinned only ◄─┘
       └──────────────────────────────────────┘
                  ▲
                  │ overlay reads
                  │
       ┌──────────────────────────────────────┐
       │ package-pins.json                    │
       │ ["flatpak", "vivaldi"]               │
       └──────────────────────────────────────┘
```

`cheni` adds a second `nixpkgs` input (`nixpkgs-latest`) to your flake.
When you pin a package, it's pulled from `nixpkgs-latest` via an
overlay. Everything else stays on your regular `nixpkgs`.

When you do a full system `upgrade`, `nixpkgs` catches up with
`nixpkgs-latest` and `cheni clean` removes obsolete pins.

For **flake inputs** (zen-browser, claude-code, etc.), `cheni pin`
updates the input directly with `nix flake update <input>`.

---

## Version safety

`cheni` distinguishes three update types:

| Status      | Meaning                         | Selection           |
|-------------|---------------------------------|---------------------|
| `minor`     | Safe bump (1.2.0 → 1.3.0)       | Default             |
| `major`     | Breaking changes possible       | Requires `--force`  |
| `newer`     | You have a newer version        | No action needed    |

Calendar versions (e.g. `2026.04.01`) are not flagged as major.

---

## Build errors

If a build fails, `cheni build` parses the cryptic Nix output and shows
a clean summary:

```
$ cheni build

Building...
... build output ...

✗ Build failed with 1 error(s):

  [1]  Undefined variable — naoetuh
       Variable 'naoetuh' is not defined.
       File: modules/desktop/desktop-others.nix
       15|     libreoffice-fresh
       16|     naoetuh
         |     ^
       17|   ];
       Hint: Check spelling, or add the variable to your function arguments.
```

Recognised error patterns:
- Hash mismatch (sha256, cargoHash)
- Undefined variable (with file + line + context)
- Unfree / broken packages
- Infinite recursion
- File not found (git staging reminder)
- Builder failure
- Package collision
- Incompatible Python version

---

## Requirements

- NixOS with flakes enabled
- A flake-based configuration
- [`nh`](https://github.com/viperML/nh) (used internally for rebuilds)

---

## Install

### Try it once

```bash
nix run gitlab:harrael/cheni -- check
```

### Install via your flake

```nix
# In your flake.nix inputs:
inputs.cheni.url = "gitlab:harrael/cheni";

# Then add to your NixOS config:
environment.systemPackages = [
  inputs.cheni.packages.x86_64-linux.default
];
```

Run `cheni init` once to set up `nixpkgs-latest` and the overlay in
your flake.

---

## Status

Early alpha — expect rough edges. Feedback and PRs welcome.

See [DESIGN.md](DESIGN.md) for architecture and roadmap.

## License

MIT — see [LICENSE](LICENSE).
