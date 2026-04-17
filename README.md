# nixup

A TUI (Terminal User Interface) for NixOS -- the equivalent of [pacseek](https://github.com/moson-mo/pacseek) (Arch) for the Nix ecosystem.

## Problem

On NixOS, there is no simple tool to:
- See which installed packages have an update available
- Compare installed versions vs the latest available on nixos-unstable
- Search for new packages with their descriptions
- Do all of this **without downloading/evaluating nixpkgs** (50+ MB, slow)

## Solution

A lightweight TUI that uses the Repology API to compare installed versions (read from the local nix store) with the latest versions available on nixos-unstable. Zero heavy downloads.

## Features

### MVP (v0.1)
- [x] List installed packages with their version (from `/run/current-system/sw`)
- [x] Compare with the latest available version (Repology API)
- [x] Highlight packages with available updates (color-coded)
- [x] Filter/search through the list
- [x] Detail view for a package (description, homepage)
- [x] Select and update packages via `nix flake update` + rebuild

### Planned
- [ ] Search for new packages
- [ ] Copy package name to clipboard
- [ ] Show which NixOS module a package is installed from
- [ ] Support for flake inputs (zen-browser, etc.) -- not just nixpkgs

## Tech Stack

- **Language**: Rust
- **TUI**: ratatui (standard Rust TUI framework)
- **HTTP**: reqwest (async API requests)
- **JSON**: serde + serde_json
- **Async**: tokio
- **Store path parsing**: regex

## Architecture

```
nixup/
  src/
    main.rs           # Entry point, TUI setup
    app.rs            # Application state
    ui.rs             # ratatui widget rendering
    store.rs          # Reading packages from the nix store
    api.rs            # Repology API client
    pins.rs           # Package pinning and update logic
    types.rs          # Data structures (Package, UpdateStatus, etc.)
  Cargo.toml
  flake.nix           # Reproducible Nix build
```

## Data Sources

### Installed packages (local, instant)
```bash
nix-store -qR /run/current-system/sw | sed 's|/nix/store/[a-z0-9]{32}-||'
```
Returns entries like `legcord-1.5.4`, `vivaldi-7.1.3693.46`, etc.

### Available versions (API, lightweight)
Repology API:
```
GET https://repology.org/api/v1/project/<package>
```
Returns version, description, and more for nix_unstable and other repos.

## Keybindings

| Key     | Action                          |
|---------|---------------------------------|
| `j`/`k` | Navigate up/down                |
| `/`     | Search/filter packages          |
| `Tab`   | Toggle view (All / Updates only)|
| `Space` | Select package for update       |
| `u`     | Update selected packages        |
| `Enter` | Show package details            |
| `Esc`   | Close popup / clear message     |
| `q`     | Quit                            |

## Inspiration

- [pacseek](https://github.com/moson-mo/pacseek) -- TUI for Arch/AUR
- [disktui](https://github.com/mitsuhiko/disktui) -- Minimalist Rust TUI
- [lazygit](https://github.com/jesseduffield/lazygit) -- Exemplary TUI UX
