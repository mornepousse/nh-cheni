# cheni clean Extension Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extend `cheni clean` with `--orphans`, `--cruft`, and `--all` flags. Default behaviour (obsolete-only) unchanged for backwards compat.

**Architecture:** Extend `src/cmd/clean.rs`. Add `CleanOptions` struct, four pure detection functions, four apply helpers, three phase orchestrators called from a refactored `run()`. Module split kept simple — single file, sibling tests via `src/cmd/tests/clean.rs`.

**Tech Stack:** Rust 2021, anyhow, dialoguer (already used). No new dependencies.

**Spec source:** `docs/superpowers/specs/2026-04-28-cheni-clean-extension-design.md`

---

### Task 1: Refactor `run()` to take `CleanOptions` + add the obsolete phase helper

**Files:**
- Modify: `src/cmd/clean.rs`
- Modify: `src/main.rs` (CLI flag wiring)

The current `pub fn run() -> Result<()>` becomes `pub fn run(opts: CleanOptions) -> Result<()>`. Same default behaviour (obsolete only when no flags). Refactor the obsolete-detection body into a private `run_obsolete_phase` helper.

- [ ] **Step 1: Add `CleanOptions` + refactor `run`**

In `src/cmd/clean.rs`:

```rust
//! `cheni clean` command.
//!
//! Detects obsolete pins (when nixpkgs has caught up with nixpkgs-latest
//! after a regular `upgrade`) and removes them automatically.
//!
//! With `--orphans`, also removes pins/freezes that no module declares.
//! With `--cruft`, also removes `result*` symlinks in flake_dir and
//! truncates the version cache when over 10 MiB.

use anyhow::Result;
use colored::Colorize;

use crate::nix::{config, pins};

use super::obsolete::count_obsolete_pins;

/// CLI options for `cheni clean`.
#[derive(Debug, Default)]
pub struct CleanOptions {
    /// Remove pins/freezes that no active module declares.
    pub orphans: bool,
    /// Remove `result*` symlinks + truncate oversized version cache.
    pub cruft: bool,
    /// Skip confirmation prompts.
    pub yes: bool,
}

/// Run `cheni clean`.
///
/// Always runs the obsolete phase (default behaviour). The `--orphans`
/// and `--cruft` flags add additional phases, each with its own
/// confirmation prompt.
pub fn run(opts: CleanOptions) -> Result<()> {
    let nix_config = config::detect()?;

    run_obsolete_phase(&nix_config)?;

    if opts.orphans {
        // Task 3 will plug the orphans phase here
    }
    if opts.cruft {
        // Task 5 will plug the cruft phase here
    }

    Ok(())
}

/// Drop pins that nixpkgs has caught up on.
fn run_obsolete_phase(nix_config: &config::NixConfig) -> Result<()> {
    let current_pins = pins::read(&nix_config.flake_dir)?;
    if current_pins.is_empty() {
        println!("{} No pins to clean.", "✓".green());
        return Ok(());
    }

    let lock_path = nix_config.flake_dir.join("flake.lock");
    let obsolete_count = count_obsolete_pins(&lock_path, &current_pins);

    if obsolete_count > 0 {
        let count = pins::clear(&nix_config.flake_dir)?;
        println!(
            "{} Removed {} obsolete {}. nixpkgs has caught up with nixpkgs-latest.",
            "✓".green(),
            count.to_string().bold(),
            crate::util::pluralize(count, "pin")
        );
    } else {
        println!(
            "Pins are still active (nixpkgs-latest is ahead). {} {} kept.",
            current_pins.len().to_string().bold(),
            crate::util::pluralize(current_pins.len(), "pin")
        );
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/clean.rs"]
mod tests;
```

- [ ] **Step 2: Wire CLI flags in `src/main.rs`**

Find the `Clean` variant in `enum Commands`. Add the flags:

```rust
/// Drop obsolete pins (and optionally orphan pins/freezes + cruft).
#[command(after_help = "Example: cheni clean --all")]
Clean {
    /// Also remove pins/freezes that no module declares.
    #[arg(long)]
    orphans: bool,

    /// Also remove `result*` symlinks + truncate oversized version cache.
    #[arg(long)]
    cruft: bool,

    /// Shortcut for --orphans --cruft.
    #[arg(long)]
    all: bool,

    /// Skip confirmation prompts.
    #[arg(long)]
    yes: bool,
},
```

In the dispatch arm:

```rust
Commands::Clean { orphans, cruft, all, yes } => {
    cmd::clean::run(cmd::clean::CleanOptions {
        orphans: orphans || all,
        cruft: cruft || all,
        yes,
    })?;
}
```

- [ ] **Step 3: Create empty sibling test file**

Create `src/cmd/tests/clean.rs`:

```rust
//! Tests for `cmd::clean`.

use super::*;
```

(Will be filled by Tasks 2-5.)

- [ ] **Step 4: Build + verify**

```
cargo build
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- clean --help    # should show new flags
cargo run -- clean           # should produce same output as before (no flags = no change)
```

- [ ] **Step 5: Commit**

```
git add src/cmd/clean.rs src/cmd/tests/clean.rs src/main.rs
git commit -m "refactor(clean): introduce CleanOptions + run_obsolete_phase

Pure refactor: cheni clean (no flags) keeps identical behaviour.
Adds --orphans, --cruft, --all, --yes flags wired through main but
the orphans/cruft phases are stubs filled by subsequent commits."
```

---

### Task 2: Pure detection — `find_orphan_pins` + tests

**Files:**
- Modify: `src/cmd/clean.rs`
- Modify: `src/cmd/tests/clean.rs`

- [ ] **Step 1: Tests first**

In `src/cmd/tests/clean.rs`:

```rust
use super::*;
use std::collections::HashSet;

fn declared(names: &[&str]) -> HashSet<String> {
    names.iter().map(|s| s.to_string()).collect()
}

#[test]
fn find_orphan_pins_returns_pins_not_in_declared() {
    let pins = vec!["firefox".to_string(), "kicad".to_string()];
    let decl = declared(&["kicad"]);
    let orphans = find_orphan_pins(&pins, &decl);
    assert_eq!(orphans, vec!["firefox".to_string()]);
}

#[test]
fn find_orphan_pins_handles_empty_pins() {
    let decl = declared(&["firefox"]);
    let orphans = find_orphan_pins(&[], &decl);
    assert!(orphans.is_empty());
}

#[test]
fn find_orphan_pins_handles_all_declared() {
    let pins = vec!["firefox".to_string(), "kicad".to_string()];
    let decl = declared(&["firefox", "kicad", "vivaldi"]);
    let orphans = find_orphan_pins(&pins, &decl);
    assert!(orphans.is_empty());
}

#[test]
fn find_orphan_pins_when_no_modules_returns_all_as_orphans() {
    // Edge case: declared is empty (no active modules detected).
    // The phase orchestrator will short-circuit BEFORE calling this fn,
    // but the pure detection must still be sound: every pin is orphan.
    let pins = vec!["firefox".to_string()];
    let decl = HashSet::new();
    let orphans = find_orphan_pins(&pins, &decl);
    assert_eq!(orphans, vec!["firefox".to_string()]);
}
```

- [ ] **Step 2: Run to verify failure**

```
cargo test cmd::clean::tests::find_orphan_pins
```

Expected: 4 failures.

- [ ] **Step 3: Implement**

In `src/cmd/clean.rs`, add:

```rust
use std::collections::HashSet;

/// Returns the list of pin names that no active module declares.
pub(crate) fn find_orphan_pins(
    pins: &[String],
    declared_packages: &HashSet<String>,
) -> Vec<String> {
    pins.iter()
        .filter(|name| !declared_packages.contains(*name))
        .cloned()
        .collect()
}
```

- [ ] **Step 4: Run tests to verify**

```
cargo test cmd::clean
cargo clippy --all-targets -- -D warnings
```

Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```
git add src/cmd/clean.rs src/cmd/tests/clean.rs
git commit -m "feat(clean): find_orphan_pins pure detection helper"
```

---

### Task 3: Phase `run_orphans_phase` + apply helpers + freeze detection

**Files:**
- Modify: `src/cmd/clean.rs`
- Modify: `src/cmd/tests/clean.rs`

- [ ] **Step 1: Add `find_orphan_freezes` test + impl**

Test (append to tests/clean.rs):

```rust
#[test]
fn find_orphan_freezes_returns_freezes_not_in_declared() {
    let mut freezes = std::collections::HashMap::new();
    freezes.insert("firefox".to_string(), crate::nix::freezes::FreezeEntry::default());
    freezes.insert("kicad".to_string(), crate::nix::freezes::FreezeEntry::default());
    let decl = declared(&["kicad"]);
    let orphans = find_orphan_freezes(&freezes, &decl);
    assert_eq!(orphans, vec!["firefox".to_string()]);
}
```

(`FreezeEntry::default()` may or may not exist. If not, construct one with whatever fields it has — `FreezeEntry { version: "1.0".into(), frozen_at: "2026-04-28".into() }` or similar.)

Impl in clean.rs:

```rust
use crate::nix::freezes::Freezes;

/// Returns the list of freeze names that no active module declares.
pub(crate) fn find_orphan_freezes(
    freezes: &Freezes,
    declared_packages: &HashSet<String>,
) -> Vec<String> {
    freezes
        .keys()
        .filter(|name| !declared_packages.contains(*name))
        .cloned()
        .collect()
}
```

Verify the test passes.

- [ ] **Step 2: Apply helpers**

Add to clean.rs:

```rust
use std::path::Path;

/// Removes the listed orphan pins from `package-pins.json`.
fn apply_remove_orphan_pins(flake_dir: &Path, names: &[String]) -> Result<()> {
    pins::remove(flake_dir, names)?;
    Ok(())
}

/// Removes the listed orphan freezes from `package-freezes.json`.
fn apply_remove_orphan_freezes(flake_dir: &Path, names: &[String]) -> Result<()> {
    crate::nix::freezes::remove(flake_dir, names)?;
    Ok(())
}
```

(Verify `pins::remove(flake_dir, &[String]) -> Result<...>` exists. If not — and only `pins::clear()` exists — adapt: read all pins, filter out orphans, write back via `pins::write` or equivalent. Same for freezes::remove.)

- [ ] **Step 3: Phase orchestrator**

Add to clean.rs:

```rust
use dialoguer::{theme::ColorfulTheme, Confirm};

fn run_orphans_phase(
    nix_config: &config::NixConfig,
    yes: bool,
) -> Result<()> {
    println!("\n{}", "Orphan pins / freezes:".bold());

    // Collect declared package names from active modules.
    let active_set = config::list_active_modules(&nix_config.flake_dir, &nix_config.hostname);
    let modules = match active_set {
        Some(m) => m,
        None => {
            println!("{}", "  (no active modules detected — skipping)".dimmed());
            return Ok(());
        }
    };
    let declared: HashSet<String> = config::extract_package_names(&modules)
        .into_iter()
        .collect();

    let pins = pins::read(&nix_config.flake_dir)?;
    let freezes = crate::nix::freezes::read(&nix_config.flake_dir)?;
    let orphan_pins = find_orphan_pins(&pins, &declared);
    let orphan_freezes = find_orphan_freezes(&freezes, &declared);

    if orphan_pins.is_empty() && orphan_freezes.is_empty() {
        println!("{} No orphan pins or freezes.", "✓".green());
        return Ok(());
    }

    if !orphan_pins.is_empty() {
        println!("  Found {} orphan pin(s):", orphan_pins.len().to_string().bold());
        for name in &orphan_pins {
            println!("    · {}", name);
        }
    }
    if !orphan_freezes.is_empty() {
        println!("  Found {} orphan freeze(s):", orphan_freezes.len().to_string().bold());
        for name in &orphan_freezes {
            println!("    · {}", name);
        }
    }

    let proceed = if yes {
        true
    } else {
        Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt("Remove these orphans?")
            .default(false)
            .interact()
            .map_err(|e| anyhow::anyhow!("reading confirmation: {e}"))?
    };

    if !proceed {
        println!("{}", "  Skipped.".dimmed());
        return Ok(());
    }

    if !orphan_pins.is_empty() {
        apply_remove_orphan_pins(&nix_config.flake_dir, &orphan_pins)?;
        println!("{} Removed {} orphan pin(s).", "✓".green(), orphan_pins.len());
    }
    if !orphan_freezes.is_empty() {
        apply_remove_orphan_freezes(&nix_config.flake_dir, &orphan_freezes)?;
        println!("{} Removed {} orphan freeze(s).", "✓".green(), orphan_freezes.len());
    }
    Ok(())
}
```

- [ ] **Step 4: Wire into `run()`**

Replace the stub:

```rust
if opts.orphans {
    run_orphans_phase(&nix_config, opts.yes)?;
}
```

- [ ] **Step 5: Build + verify**

```
cargo build
cargo clippy --all-targets -- -D warnings
cargo test
```

If `pins::remove` or `freezes::remove` don't exist, add them (each is a small parse-mutate-write operation, atomic via `util::atomic_write`). Or adapt the apply helpers to reuse existing write surfaces.

- [ ] **Step 6: Commit**

```
git add src/cmd/clean.rs src/cmd/tests/clean.rs src/nix/pins.rs src/nix/freezes.rs
git commit -m "feat(clean): --orphans phase

Detects pins/freezes whose name appears in no active module and
proposes removal. Confirmation prompt unless --yes. Short-circuits
when active modules can't be detected (false-positive avoidance).
Adds pins::remove + freezes::remove if not already present."
```

---

### Task 4: Pure detection — `find_result_symlinks` + `version_cache_size_bytes` + tests

**Files:**
- Modify: `src/cmd/clean.rs`
- Modify: `src/cmd/tests/clean.rs`

- [ ] **Step 1: Tests for find_result_symlinks**

Append to tests/clean.rs:

```rust
#[test]
fn find_result_symlinks_in_tempdir() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let target = dir.path().join("target");
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, dir.path().join("result")).unwrap();
    std::os::unix::fs::symlink(&target, dir.path().join("result-1")).unwrap();
    std::fs::write(dir.path().join("flake.nix"), "").unwrap();

    let mut found = find_result_symlinks(dir.path());
    found.sort();
    let names: Vec<String> = found
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, vec!["result".to_string(), "result-1".to_string()]);
}

#[test]
fn find_result_symlinks_ignores_non_results() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let target = dir.path().join("t");
    std::fs::create_dir(&target).unwrap();
    std::os::unix::fs::symlink(&target, dir.path().join("flake.nix")).unwrap();
    std::os::unix::fs::symlink(&target, dir.path().join("hello")).unwrap();

    let found = find_result_symlinks(dir.path());
    assert!(found.is_empty());
}
```

- [ ] **Step 2: Implement `find_result_symlinks`**

In clean.rs:

```rust
use std::path::PathBuf;

/// Returns the paths of `result*` symlinks in `flake_dir`.
pub(crate) fn find_result_symlinks(flake_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(flake_dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            // Must be a symlink whose name starts with "result".
            let name = e.file_name();
            let name_str = name.to_string_lossy();
            if !name_str.starts_with("result") {
                return false;
            }
            e.file_type().map(|t| t.is_symlink()).unwrap_or(false)
        })
        .map(|e| e.path())
        .collect()
}
```

- [ ] **Step 3: Implement `version_cache_size_bytes`**

```rust
/// Returns the size in bytes of the version cache, or 0 if missing.
pub(crate) fn version_cache_size_bytes() -> u64 {
    let path = crate::nix::version_cache::cache_path();
    std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
}

/// Threshold above which the version cache is considered oversized
/// and `cheni clean --cruft` proposes truncation.
pub(crate) const VERSION_CACHE_TRUNCATE_THRESHOLD: u64 = 10 * 1024 * 1024;
```

- [ ] **Step 4: Build + tests**

```
cargo test cmd::clean
cargo clippy --all-targets -- -D warnings
```

Expected: 6 tests pass (4 from Task 2 + 2 new).

- [ ] **Step 5: Commit**

```
git add src/cmd/clean.rs src/cmd/tests/clean.rs
git commit -m "feat(clean): find_result_symlinks + version_cache_size_bytes

Pure detection helpers for the cruft phase. Threshold for cache
truncation matches doctor's 10 MiB warning."
```

---

### Task 5: Phase `run_cruft_phase` + apply helpers

**Files:**
- Modify: `src/cmd/clean.rs`

- [ ] **Step 1: Apply helpers**

Append to clean.rs:

```rust
/// Deletes the `result*` symlinks. Returns the count of successfully removed.
fn apply_remove_result_symlinks(paths: &[PathBuf]) -> Result<usize> {
    let mut removed = 0usize;
    for p in paths {
        if let Err(e) = std::fs::remove_file(p) {
            tracing::debug!("failed to remove {}: {}", p.display(), e);
            continue;
        }
        removed += 1;
    }
    Ok(removed)
}

/// Truncates the version cache by removing the file.
fn apply_truncate_version_cache() -> Result<()> {
    let path = crate::nix::version_cache::cache_path();
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}
```

- [ ] **Step 2: Phase orchestrator**

```rust
fn run_cruft_phase(
    nix_config: &config::NixConfig,
    yes: bool,
) -> Result<()> {
    println!("\n{}", "Cruft:".bold());

    let symlinks = find_result_symlinks(&nix_config.flake_dir);
    let cache_size = version_cache_size_bytes();
    let cache_oversized = cache_size > VERSION_CACHE_TRUNCATE_THRESHOLD;

    if symlinks.is_empty() && !cache_oversized {
        println!("{} No cruft to clean.", "✓".green());
        return Ok(());
    }

    if !symlinks.is_empty() {
        println!("  Found {} result symlink(s):", symlinks.len().to_string().bold());
        for p in &symlinks {
            println!("    · {}", p.display());
        }
    }
    if cache_oversized {
        let mib = cache_size as f64 / (1024.0 * 1024.0);
        println!(
            "  Version cache: {:.1} MiB (over the {} MiB threshold).",
            mib,
            VERSION_CACHE_TRUNCATE_THRESHOLD / (1024 * 1024)
        );
    } else if cache_size > 0 {
        let mib = cache_size as f64 / (1024.0 * 1024.0);
        println!(
            "  Version cache: {:.1} MiB (below threshold, kept).",
            mib
        );
    }

    let proceed = if yes {
        true
    } else {
        Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt("Remove the cruft?")
            .default(false)
            .interact()
            .map_err(|e| anyhow::anyhow!("reading confirmation: {e}"))?
    };

    if !proceed {
        println!("{}", "  Skipped.".dimmed());
        return Ok(());
    }

    if !symlinks.is_empty() {
        let removed = apply_remove_result_symlinks(&symlinks)?;
        println!("{} Removed {} result symlink(s).", "✓".green(), removed);
    }
    if cache_oversized {
        apply_truncate_version_cache()?;
        println!("{} Truncated version cache.", "✓".green());
    }
    Ok(())
}
```

- [ ] **Step 3: Wire into `run()`**

```rust
if opts.cruft {
    run_cruft_phase(&nix_config, opts.yes)?;
}
```

- [ ] **Step 4: Build + verify**

```
cargo build
cargo clippy --all-targets -- -D warnings
cargo test
```

- [ ] **Step 5: Commit**

```
git add src/cmd/clean.rs
git commit -m "feat(clean): --cruft phase

Detects result* symlinks in flake_dir and an oversized version cache.
Removes them after confirmation. Cache truncation only if size exceeds
the 10 MiB threshold (matches doctor's warning)."
```

---

### Task 6: Smoke test + final verification

**Files:** N/A (gates de qualité)

- [ ] **Step 1: Pre-merge gate**

```
cargo build --release
cargo clippy --all-targets -- -D warnings
cargo test
```

- [ ] **Step 2: Sandbox Nix gate**

```
nix build .#cheni
./result/bin/cheni clean --help
./result/bin/cheni clean              # default — only obsolete phase
./result/bin/cheni clean --orphans    # detects + asks (reply N to abort)
./result/bin/cheni clean --cruft      # detects + asks (reply N to abort)
./result/bin/cheni clean --all        # all three phases
```

- [ ] **Step 3: Verify no regression**

`cheni clean` (no flags) must produce identical output to v0.6.x. Any deviation = regression.

- [ ] **Step 4: Diff stat**

```
git diff main..HEAD --stat
```

Expected: ~7 commits (Tasks 1-5 + spec/plan), positive net diff in `src/cmd/clean.rs` and small adds in `src/main.rs`.

- [ ] **Step 5: Merge to main + push**

(Controller decides — surface state.)

---

## Auto-review

**Spec coverage** :
- ✅ CleanOptions + flags (Task 1)
- ✅ Backwards compat (Task 1 — default still drops obsolete only)
- ✅ find_orphan_pins (Task 2)
- ✅ find_orphan_freezes (Task 3)
- ✅ run_orphans_phase with confirmation + active-module short-circuit (Task 3)
- ✅ find_result_symlinks (Task 4)
- ✅ version_cache_size_bytes + threshold (Task 4)
- ✅ run_cruft_phase (Task 5)
- ✅ CLI dispatch (Task 1, --all combines flags)
- ✅ Tests on pure detection helpers (Tasks 2, 3, 4)
- ✅ No regression check (Task 6)

**Placeholders** : aucun "TBD"/"TODO". Le commentaire dans Task 3 sur `pins::remove`/`freezes::remove` "verify exists; if not, add them" est une note honnête (faut vérifier — si absent, ajouter).

**Type consistency** :
- `CleanOptions { orphans, cruft, yes }` consistent across Tasks 1, 2, 3, 5
- `find_orphan_pins(pins: &[String], declared: &HashSet<String>) -> Vec<String>` consistent
- `find_orphan_freezes(freezes: &Freezes, declared: &HashSet<String>) -> Vec<String>` consistent
- `find_result_symlinks(flake_dir: &Path) -> Vec<PathBuf>` consistent
- `version_cache_size_bytes() -> u64` + `VERSION_CACHE_TRUNCATE_THRESHOLD: u64 = 10 MiB` consistent
- `apply_*` functions take `flake_dir: &Path` and `names: &[String]` / `paths: &[PathBuf]` consistently
