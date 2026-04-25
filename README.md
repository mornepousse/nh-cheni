# cheni

> **Granular package updates for NixOS.**
> (_cheni_ — Swiss-French for "mess/clutter". This tool tidies up the clutter of Nix updates.)

<!--
  This repository may be viewed on GitHub as a read-only mirror.
  Primary development happens on GitLab — please file issues and
  merge requests there: https://gitlab.com/harrael/cheni
-->

> [!NOTE]
> Primary repo: **https://gitlab.com/harrael/cheni**
> Issues and merge requests are tracked there. The GitHub copy is an automated mirror.

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
$ cheni upgrade --pins-only
```

Only those two packages get updated. Everything else stays on your
current `nixpkgs`.

---

## Quick start

```bash
cheni init                     # one-time flake setup
cheni                          # interactive menu (no subcommand → picker)
cheni check                    # see what's outdated
cheni pin <package>            # pin a package for update
cheni upgrade --pins-only      # apply all pins (rebuild system)
```

Run `cheni` with no arguments for an interactive menu showing the
current state and a list of every command. Pick one with the arrow
keys; cheni prompts for any extra input it needs.

---

## Commands

### Inspection

| Command                     | What it does                                           |
|-----------------------------|--------------------------------------------------------|
| `cheni check`               | Show available updates (nixpkgs + flake inputs)        |
| `cheni check -c dev`        | Filter to packages declared in `modules/dev/`          |
| `cheni check --details`     | Expand the "Newer" and "Unknown" buckets               |
| `cheni check --refresh`     | Ignore the on-disk cache, re-fetch every lookup        |
| `cheni check --pending`     | Append a closure dry-run section (kernel + base, ~30s) |
| `cheni check --json`        | Machine-readable output for scripts / CI               |
| `cheni status`              | Config, active gen, flake input ages, suggestions      |

> **`check` vs `check --pending`.** Plain `cheni check` looks
> *upstream*: it scans your modules for named packages and asks
> Repology whether the current nixpkgs version is the latest one.
> `--pending` adds a second pass that looks *downstream*: a
> `nix build --dry-run` against your current `flake.lock` to list
> what would actually rebuild — kernel, base nixpkgs packages and
> transitive dependencies included, none of which appear in the
> Repology view because they aren't directly named in your modules.
> The two views answer different questions and complement each other.

### Pinning (route to a newer version via `nixpkgs-latest`)

| Command                   | What it does                                |
|---------------------------|---------------------------------------------|
| `cheni pin <pkg>`         | Pin a single nixpkgs package                |
| `cheni pin -c dev`          | Pin all minor updates in `modules/dev/`   |
| `cheni pin -c dev --force`  | Include major updates (breaking changes)  |
| `cheni pin --flakes`      | Update all flake inputs (zen-browser, etc.) |
| `cheni unpin <pkg>`       | Remove a specific pin                       |
| `cheni unpin --all`       | Remove all pins                             |

### Freezing (hold at the current version — inverse of pin)

| Command                   | What it does                                         |
|---------------------------|------------------------------------------------------|
| `cheni freeze`            | List currently frozen packages                       |
| `cheni freeze <pkg>`      | Hold `<pkg>` at its current nixpkgs rev              |
| `cheni unfreeze <pkg>`    | Release a specific freeze                            |
| `cheni unfreeze --all`    | Release every freeze at once                         |

`pin` routes a package through `nixpkgs-latest` so it gets a *newer*
version. `freeze` does the opposite — it locks the package at the
**current** nixpkgs revision while the rest of the system moves. Use
it for "nvidia 560 works, don't move me to 570 before I test" or
"new discord broke my config, hold it until upstream fixes".

### Apply

| Command                       | What it does                                          |
|-------------------------------|-------------------------------------------------------|
| `cheni build`                 | Rebuild current `flake.lock` state — no fetch         |
| `cheni upgrade --pins-only`   | Refresh `nixpkgs-latest` only, then rebuild (apply pins) |
| `cheni upgrade`               | Refresh ALL flake inputs, preview, then rebuild       |
| `cheni upgrade --gc`          | Same + `nix-collect-garbage --delete-older-than 30d`  |
| `cheni clean`                 | Auto-remove obsolete pins (nixpkgs caught up)         |

> **A typical trap with `flake.lock`.** `cheni upgrade` runs
> `nix flake update` *before* the rebuild prompt. If you cancel at
> the prompt, the lock is already updated on disk — the rebuild
> didn't happen, but every input has bumped. The next time you run
> *any* rebuild (`cheni build`, `cheni upgrade --pins-only`,
> `cheni upgrade`), all those pending bumps get applied — including
> the kernel and other base packages well outside the scope of the
> command you just typed. cheni warns about a dirty `flake.lock` at
> the start of every upgrade so you can `git checkout flake.lock`
> to discard the pending bumps before continuing.

### History & rollback

| Command                              | What it does                                         |
|--------------------------------------|------------------------------------------------------|
| `cheni history`                      | List recent generations + per-step package summary   |
| `cheni history --full`               | Don't truncate the per-step summary to terminal width |
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

| Command                     | What it does                                              |
|-----------------------------|-----------------------------------------------------------|
| `cheni doctor`              | Health checks (paths, pins, flake, store, cache, tools)   |
| `cheni self-update`         | Refresh the cheni flake input + verify + rebuild          |
| `cheni verify [--tag v…]`   | Read-only signature check on the installed cheni          |
| `cheni diagnose [file]`     | Clarify a cryptic rebuild log (file or stdin)             |
| `cheni init`                | One-time setup: add `nixpkgs-latest` to your flake        |
| `cheni bug-report`          | Print a diagnostic report ready to paste into an issue    |
| `cheni completion <shell>`  | Shell completion (bash / zsh / fish / elvish / powershell)|
| `cheni man`                 | Emit a roff man page on stdout                            |

#### Installing shell completions

```bash
# fish
cheni completion fish > ~/.config/fish/completions/cheni.fish

# zsh (make sure ~/.zfunc is in your $fpath)
cheni completion zsh > ~/.zfunc/_cheni

# bash
cheni completion bash > /etc/bash_completion.d/cheni    # or ~/.bash_completion
```

#### Installing the man page

```bash
cheni man > ~/.local/share/man/man1/cheni.1
# then: man cheni
```

### Short aliases

Every frequently-used command has a two-letter alias:

| Alias        | Command        |
|--------------|----------------|
| `cheni ck`   | `check`        |
| `cheni st`   | `status`       |
| `cheni ug`   | `upgrade`      |
| `cheni b`    | `build`        |
| `cheni h`    | `history`      |
| `cheni rb`   | `rollback`     |
| `cheni s`    | `search`       |

---

## Scripting

`cheni check --json` emits a stable JSON document suitable for piping
into `jq`, Prometheus textfile exporters, or any CI gate:

```bash
# Fail a pre-commit hook if any major update is pending
cheni check --json | jq -e '.summary.major == 0' >/dev/null \
  || { echo "Major update pending, review first"; exit 1; }

# Desktop notification on new updates
count=$(cheni check --json | jq '.summary.minor + .summary.major')
[ "$count" -gt 0 ] && notify-send "$count updates available"

# Compare two machines over SSH
ssh x230t cheni check --json > x230t.json
cheni check --json > laptop.json
jq -s '.[0].minor_updates - .[1].minor_updates' laptop.json x230t.json
```

Schema:

```
{
  "flake_inputs":  [{name, installed, has_update, latest_remote_date}],
  "minor_updates": [{name, installed, available, declared_in}],
  "major_updates": [...],
  "newer":         [...],
  "unknown":       ["pkg1", "pkg2"],
  "summary":       {up_to_date, minor, major, newer, unknown}
}
```

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
- A flake-based configuration declaring `nixosConfigurations.<hostname>`
- [`nh`](https://github.com/viperML/nh) — used internally for rebuilds
- [`nvd`](https://gitlab.com/khumba/nvd) (optional) — produces nicer output
  for `cheni diff` and `cheni history --diff`; cheni falls back to
  `nix store diff-closures` if absent

## Environment variables

| Variable             | Default    | Purpose                                      |
|----------------------|------------|----------------------------------------------|
| `CHENI_CONFIG`       | _(auto)_   | Override the NixOS flake directory           |
| `CHENI_HTTP_TIMEOUT` | `30` (sec) | Per-request HTTP timeout (min 5)             |
| `NO_COLOR`           | unset      | Disable coloured output                      |
| `RUST_BACKTRACE`     | unset      | Full panic backtrace (for bug reports)       |

Raise `CHENI_HTTP_TIMEOUT` on slow connections:
```bash
CHENI_HTTP_TIMEOUT=60 cheni check
```

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

### Uninstalling

cheni is designed so that your NixOS config keeps working even if you
stop using it entirely. The overlay added by `cheni init` does:

```nix
pins = if builtins.pathExists ./package-pins.json
       then builtins.fromJSON (builtins.readFile ./package-pins.json)
       else [];
```

So an **empty or missing** `package-pins.json` is a no-op — the overlay
degrades gracefully to identity.

To fully remove cheni:

1. `cheni unpin --all` (optional — leaves `[]` in the pins file).
2. Remove `inputs.cheni.url` from `flake.nix`.
3. Remove `environment.systemPackages = [ inputs.cheni.packages... ]` if
   you had it.
4. Optional: remove the `nixpkgs-latest` input and the cheni overlay
   block. Leaving them in place is harmless once pins are empty.
5. Rebuild: `sudo nixos-rebuild switch --flake .`

The only cheni-specific file in your repo is `package-pins.json` —
safe to delete.

---

## Status

Early alpha — expect rough edges. Feedback and PRs welcome.

See [DESIGN.md](DESIGN.md) for architecture,
[CHANGELOG.md](CHANGELOG.md) for the history of changes,
[DIAGNOSE.md](DIAGNOSE.md) for the full `cheni diagnose` pattern
catalogue, and
[SECURITY.md](SECURITY.md) for the release signing / verification
model (`cheni verify`, `cheni self-update` trust chain).

## License

MIT — see [LICENSE](LICENSE).
