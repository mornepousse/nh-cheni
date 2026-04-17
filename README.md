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

Updates available:
  flatpak                  1.16.4         → 1.16.6         (minor)
  github-copilot-cli       1.0.19         → 1.0.21         (minor)
  htop                     3.4.1          → 3.5.0          (minor)
  vivaldi                  7.9.3970.47    → 7.9.3970.50    (minor)

Major updates (use 'cheni pin --force' to apply):
  mesa                     24.3.2         → 25.1.0         (major)

Flake inputs:
  claude-code              2.1.112        UPDATE (latest: 2026-04-17)
  zen-browser              ?              UPDATE (latest: 2026-04-17)
  kesp-controller          2.0.7          ok

Up to date: 112 | Minor: 4 | Major: 1 | Newer: 6 | Unknown: 11
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
cheni check             # see what's outdated
cheni pin <package>     # pin a package for update
cheni update            # apply all pins (rebuild system)
```

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

| Command         | What it does                                     |
|-----------------|--------------------------------------------------|
| `cheni update`  | Apply pins: refresh `nixpkgs-latest` + rebuild   |
| `cheni build`   | Rebuild with human-readable error parsing        |
| `cheni clean`   | Auto-remove obsolete pins (nixpkgs caught up)    |

### Setup

| Command       | What it does                              |
|---------------|-------------------------------------------|
| `cheni init`  | Add `nixpkgs-latest` input to your flake  |

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
