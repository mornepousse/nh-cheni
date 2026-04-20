//! `cheni init` command.
//!
//! First-time setup: adds `nixpkgs-latest` input and the overlay
//! to the user's flake.nix, and creates `package-pins.json`.
//!
//! If the flake can't be modified automatically (exotic structure),
//! falls back to printing manual instructions.

use std::path::Path;

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::{debug, warn};

use crate::nix::config;

/// Run `cheni init`.
pub fn run() -> Result<()> {
    let nix_config = config::detect()?;
    let flake_dir = &nix_config.flake_dir;

    print_init_header(flake_dir, &nix_config.hostname);
    let _ = create_pins_file(flake_dir)?;
    let _ = create_freezes_file(flake_dir)?;

    let flake_path = flake_dir.join("flake.nix");
    let flake_content = std::fs::read_to_string(&flake_path)
        .context("Failed to read flake.nix")?;

    if !ensure_nixpkgs_latest_input(&flake_path, &flake_content, &nix_config.hostname)? {
        return Ok(());
    }
    if !ensure_overlay(&flake_path, &nix_config.hostname)? {
        return Ok(());
    }
    if !ensure_freeze_overlay(&flake_path, &nix_config.hostname)? {
        return Ok(());
    }

    println!("\n{} cheni is ready! Try '{}'.", "✓".green(), "cheni check".bold());
    Ok(())
}

fn print_init_header(flake_dir: &Path, hostname: &str) {
    println!("{}\n", "=== cheni init ===".bold());
    println!("  Config:   {}", flake_dir.display());
    println!("  Hostname: {}\n", hostname);
}

/// Step 1: make sure the `nixpkgs-latest` input is declared.
///
/// Returns Ok(true) when the caller should proceed to the overlay step,
/// Ok(false) when the flake was too exotic for auto-editing and we've
/// already printed manual instructions.
fn ensure_nixpkgs_latest_input(
    flake_path: &Path,
    flake_content: &str,
    hostname: &str,
) -> Result<bool> {
    if flake_content.contains("nixpkgs-latest") {
        println!("{} nixpkgs-latest already in flake.nix.", "[1/3]".dimmed());
        return Ok(true);
    }
    match add_nixpkgs_latest(flake_path, flake_content) {
        Ok(()) => {
            println!(
                "{} Added nixpkgs-latest input to flake.nix.  {}",
                "[1/3]".dimmed(),
                "OK".green()
            );
            Ok(true)
        }
        Err(e) => {
            warn!("Auto-modification failed: {}", e);
            println!(
                "{} Could not auto-modify flake.nix.  {}",
                "[1/3]".dimmed(),
                "MANUAL".yellow()
            );
            print_manual_instructions(hostname);
            Ok(false)
        }
    }
}

/// Step 2: make sure the pins overlay is wired into nixpkgs.overlays.
///
/// Re-reads the flake so we observe step 1's edit; same Ok(true)/Ok(false)
/// contract as `ensure_nixpkgs_latest_input`.
fn ensure_overlay(flake_path: &Path, hostname: &str) -> Result<bool> {
    let flake_content = std::fs::read_to_string(flake_path)
        .context("Failed to re-read flake.nix")?;

    if flake_content.contains("package-pins.json") {
        println!("{} Pin overlay already configured.", "[2/3]".dimmed());
        return Ok(true);
    }
    match add_overlay(flake_path, &flake_content, hostname) {
        Ok(()) => {
            println!(
                "{} Added cheni pin overlay to flake.nix.    {}",
                "[2/3]".dimmed(),
                "OK".green()
            );
            Ok(true)
        }
        Err(e) => {
            warn!("Overlay auto-modification failed: {}", e);
            println!(
                "{} Could not add overlay automatically.  {}",
                "[2/3]".dimmed(),
                "MANUAL".yellow()
            );
            print_overlay_instructions(hostname);
            Ok(false)
        }
    }
}

/// Step 3: make sure the freeze overlay is wired into nixpkgs.overlays.
///
/// Detection uses the `package-freezes.json` marker string (same idea
/// as the pin overlay's `package-pins.json` marker) so re-running
/// `cheni init` on an already-configured flake is a no-op.
fn ensure_freeze_overlay(flake_path: &Path, hostname: &str) -> Result<bool> {
    let flake_content = std::fs::read_to_string(flake_path)
        .context("Failed to re-read flake.nix")?;

    if flake_content.contains("package-freezes.json") {
        println!("{} Freeze overlay already configured.", "[3/3]".dimmed());
        return Ok(true);
    }
    match add_freeze_overlay(flake_path, &flake_content) {
        Ok(()) => {
            println!(
                "{} Added cheni freeze overlay to flake.nix. {}",
                "[3/3]".dimmed(),
                "OK".green()
            );
            Ok(true)
        }
        Err(e) => {
            warn!("Freeze overlay auto-modification failed: {}", e);
            println!(
                "{} Could not add freeze overlay automatically.  {}",
                "[3/3]".dimmed(),
                "MANUAL".yellow()
            );
            print_freeze_overlay_instructions(hostname);
            Ok(false)
        }
    }
}

/// Create package-pins.json if it doesn't exist.
fn create_pins_file(flake_dir: &Path) -> Result<bool> {
    let path = flake_dir.join("package-pins.json");

    if path.exists() {
        debug!("package-pins.json already exists");
        return Ok(false);
    }

    crate::util::atomic_write(&path, "[]\n")
        .context("Failed to create package-pins.json")?;

    debug!("Created package-pins.json");
    Ok(true)
}

/// Create package-freezes.json if it doesn't exist.
///
/// Empty object rather than the pin overlay's empty array — the freeze
/// file is a map `{ name: entry }`, not a flat list. The overlay's
/// `builtins.pathExists` guard makes this file optional at eval time,
/// but seeding it keeps `cheni freeze` from having to create the file
/// on first invocation.
fn create_freezes_file(flake_dir: &Path) -> Result<bool> {
    let path = flake_dir.join("package-freezes.json");

    if path.exists() {
        debug!("package-freezes.json already exists");
        return Ok(false);
    }

    crate::util::atomic_write(&path, "{}\n")
        .context("Failed to create package-freezes.json")?;

    debug!("Created package-freezes.json");
    Ok(true)
}

/// Add `nixpkgs-latest` input to flake.nix.
///
/// Strategy: find the `inputs = {` block and insert after the nixpkgs line.
fn add_nixpkgs_latest(flake_path: &Path, content: &str) -> Result<()> {
    // Collect lines once; the original code called `content.lines().count()`
    // inside the outer loop and `content.lines().nth(j)` inside the inner
    // loop, which made each iteration walk the whole content again — O(n³)
    // overall on a file with N lines.
    let lines: Vec<&str> = content.lines().collect();

    let insert_line = find_nixpkgs_insert_line(&lines)
        .context("Could not find nixpkgs input in flake.nix")?;
    debug!("Inserting nixpkgs-latest after line {}", insert_line + 1);

    let new_content = build_content_with_latest_input(&lines, insert_line);
    // Atomic write: a crash mid-write on flake.nix would break *all*
    // future rebuilds. rename-on-top ensures the file is either the
    // original or the new content, never truncated.
    crate::util::atomic_write(flake_path, &new_content)
        .context("Failed to write modified flake.nix")?;

    Ok(())
}

/// Locate the line to insert `nixpkgs-latest` after.
///
/// Matches either `nixpkgs.url = "...";` (single-line form) or
/// `nixpkgs = { ... };` (multi-line form); for the latter we scan
/// forward for the closing `};` so the new input doesn't land inside
/// the existing declaration. Returns None if no nixpkgs input is
/// recognisable — the caller converts that into a MANUAL fallback.
fn find_nixpkgs_insert_line(lines: &[&str]) -> Option<usize> {
    const PATTERNS: [&str; 2] = ["nixpkgs.url", "nixpkgs = {"];
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if !PATTERNS.iter().any(|p| trimmed.contains(p)) {
            continue;
        }
        // Single-line form ends on the same line.
        if trimmed.ends_with(';') || trimmed.ends_with("\";") {
            return Some(i);
        }
        // Multi-line form: scan forward for the closing `};`.
        // Fall back to the declaration line itself if no closer is found,
        // keeping behaviour identical to the original implementation.
        for (j, next_line) in lines.iter().enumerate().skip(i + 1) {
            let t = next_line.trim();
            if t.starts_with("};") || t == "};" {
                return Some(j);
            }
        }
        return Some(i);
    }
    None
}

/// Reconstruct the file with the new input declaration inserted after
/// `insert_line`. A blank line first keeps the existing style (inputs
/// are typically separated by a blank line in user flakes).
fn build_content_with_latest_input(lines: &[&str], insert_line: usize) -> String {
    let mut new_lines: Vec<String> = Vec::with_capacity(lines.len() + 3);
    for (i, line) in lines.iter().enumerate() {
        new_lines.push((*line).to_string());
        if i == insert_line {
            new_lines.push(String::new());
            new_lines.push(
                "    # nixpkgs-latest: independently updated for per-package updates (cheni)"
                    .to_string(),
            );
            new_lines.push(
                "    nixpkgs-latest.url = \"github:NixOS/nixpkgs/nixos-unstable\";".to_string(),
            );
        }
    }
    new_lines.join("\n") + "\n"
}

/// Add the cheni overlay to the nixosSystem modules.
///
/// Strategy: find the `nixpkgs.overlays = [` block for the matching
/// hostname and add the cheni overlay.
fn add_overlay(flake_path: &Path, content: &str, _hostname: &str) -> Result<()> {
    // Look for the overlay block in the matching nixosConfiguration
    // We look for `nixpkgs.overlays = [` within the hostname's section
    // The overlay is deliberately self-sufficient:
    //   - No cheni binary required at build time (it's a one-off edit).
    //   - package-pins.json missing = no pins = plain nixpkgs. Users who
    //     delete the file (e.g. uninstalling cheni) keep a working flake.
    //   - Empty pins = identity overlay.
    // Removing the overlay + the nixpkgs-latest input leaves the flake
    // exactly as it was before `cheni init`.
    let overlay_code = r#"              # cheni: per-package updates from nixpkgs-latest.
              # Safe to leave in place if cheni is uninstalled — an absent
              # or empty package-pins.json degrades to the identity overlay.
              # `inherit (prev) system` picks up the current architecture
              # automatically (x86_64-linux / aarch64-linux / aarch64-darwin).
              (final: prev:
                let
                  pkgs-latest = import inputs.nixpkgs-latest {
                    inherit (prev) system;
                    config.allowUnfree = true;
                  };
                  pins = if builtins.pathExists ./package-pins.json
                         then builtins.fromJSON (builtins.readFile ./package-pins.json)
                         else [];
                in
                builtins.listToAttrs (builtins.filter (x: x != null) (map (name:
                  if pkgs-latest ? ${name}
                  then { inherit name; value = pkgs-latest.${name}; }
                  else null
                ) pins)))"#;

    // Find `nixpkgs.overlays = [` and insert the overlay after the opening bracket
    let mut found = false;
    let lines: Vec<&str> = content.lines().collect();
    let mut new_lines: Vec<String> = Vec::new();

    for line in &lines {
        new_lines.push(line.to_string());

        if !found && line.trim().contains("nixpkgs.overlays") && line.contains("[") {
            new_lines.push(overlay_code.to_string());
            found = true;
            debug!("Inserted overlay after: {}", line.trim());
        }
    }

    if !found {
        anyhow::bail!("Could not find 'nixpkgs.overlays = [' in flake.nix");
    }

    let new_content = new_lines.join("\n") + "\n";
    crate::util::atomic_write(flake_path, &new_content)
        .context("Failed to write modified flake.nix")?;

    Ok(())
}

/// Add the cheni freeze overlay to the nixosSystem modules.
///
/// Strategy: identical to `add_overlay` — find `nixpkgs.overlays = [` and
/// insert the overlay right after the opening bracket. Both overlays
/// end up in the same list; order between them doesn't matter in
/// practice because `cheni freeze` rejects packages that are already
/// pinned (and vice versa), so the two overlays never target the same
/// attribute.
fn add_freeze_overlay(flake_path: &Path, content: &str) -> Result<()> {
    // The overlay is self-sufficient, like the pin overlay:
    //   - `builtins.pathExists` guard means a missing file = identity overlay.
    //   - Uses only `builtins.*` (no dependency on nixpkgs `lib`) so it
    //     works even if a user overrides/shadows the nixpkgs `lib` set.
    //   - `builtins.fetchTree` with a pinned `narHash` is fully content-
    //     addressed: the fetch is cached by hash, survives `nix store gc`
    //     (re-fetch by hash), and produces the same result offline once
    //     the tarball is in the store.
    //
    // Removing the overlay + the package-freezes.json file leaves the
    // flake exactly as it was before `cheni init` ran step 3.
    let overlay_code = r#"              # cheni: frozen packages — held at their snapshot nixpkgs rev.
              # Safe to leave in place if cheni is uninstalled — an absent
              # or empty package-freezes.json degrades to the identity overlay.
              # `inherit (prev) system` picks up the current architecture
              # automatically (x86_64-linux / aarch64-linux / aarch64-darwin).
              (final: prev:
                let
                  freezes = if builtins.pathExists ./package-freezes.json
                            then builtins.fromJSON (builtins.readFile ./package-freezes.json)
                            else {};
                  mkFrozen = name: entry:
                    let
                      pkgs-at-rev = import (builtins.fetchTree {
                        type = "github";
                        owner = "NixOS";
                        repo = "nixpkgs";
                        rev = entry.rev;
                        narHash = entry.narHash;
                      }) { inherit (prev) system; config.allowUnfree = true; };
                    in
                    if pkgs-at-rev ? ${name}
                    then { inherit name; value = pkgs-at-rev.${name}; }
                    else null;
                in
                builtins.listToAttrs (builtins.filter (x: x != null)
                  (builtins.attrValues (builtins.mapAttrs mkFrozen freezes))))"#;

    let mut found = false;
    let lines: Vec<&str> = content.lines().collect();
    let mut new_lines: Vec<String> = Vec::new();

    for line in &lines {
        new_lines.push(line.to_string());
        if !found && line.trim().contains("nixpkgs.overlays") && line.contains('[') {
            new_lines.push(overlay_code.to_string());
            found = true;
            debug!("Inserted freeze overlay after: {}", line.trim());
        }
    }

    if !found {
        anyhow::bail!("Could not find 'nixpkgs.overlays = [' in flake.nix");
    }

    let new_content = new_lines.join("\n") + "\n";
    crate::util::atomic_write(flake_path, &new_content)
        .context("Failed to write modified flake.nix")?;

    Ok(())
}

/// Print manual instructions when auto-modification fails.
fn print_manual_instructions(hostname: &str) {
    println!();
    println!("{}", "Add this to your flake.nix inputs:".bold());
    println!();
    println!("  {}",
        "nixpkgs-latest.url = \"github:NixOS/nixpkgs/nixos-unstable\";".cyan()
    );
    println!();
    print_overlay_instructions(hostname);
}

/// Print overlay instructions.
fn print_overlay_instructions(_hostname: &str) {
    println!("{}", "Add this overlay to your nixosSystem modules:".bold());
    println!();
    println!("{}", r#"  ({ config, pkgs, ... }: {
    # Missing / empty file degrades to the identity overlay — remove cheni
    # safely at any time by deleting this block, the nixpkgs-latest input,
    # and (optionally) package-pins.json.
    nixpkgs.overlays = [
      # cheni: pinned packages from nixpkgs-latest
      (final: prev:
        let
          pkgs-latest = import inputs.nixpkgs-latest {
            inherit (prev) system;
            config.allowUnfree = true;
          };
          pins = if builtins.pathExists ./package-pins.json
                 then builtins.fromJSON (builtins.readFile ./package-pins.json)
                 else [];
        in
        builtins.listToAttrs (builtins.filter (x: x != null) (map (name:
          if pkgs-latest ? ${name}
          then { inherit name; value = pkgs-latest.${name}; }
          else null
        ) pins)))
    ];
  })"#.cyan());
    println!();
    println!(
        "Then create {} with content: {}",
        "package-pins.json".bold(),
        "[]".cyan()
    );
}

/// Print manual instructions for the freeze overlay when auto-modification
/// fails. The overlay is self-sufficient (identity when the JSON file is
/// absent), so users can paste it once and forget about it.
fn print_freeze_overlay_instructions(_hostname: &str) {
    println!();
    println!(
        "{}",
        "Also add this freeze overlay (inverse of pin — holds packages at a snapshot):"
            .bold()
    );
    println!();
    println!("{}", r#"  (final: prev:
    let
      freezes = if builtins.pathExists ./package-freezes.json
                then builtins.fromJSON (builtins.readFile ./package-freezes.json)
                else {};
      mkFrozen = name: entry:
        let
          pkgs-at-rev = import (builtins.fetchTree {
            type = "github";
            owner = "NixOS";
            repo = "nixpkgs";
            rev = entry.rev;
            narHash = entry.narHash;
          }) { inherit (prev) system; config.allowUnfree = true; };
        in
        if pkgs-at-rev ? ${name}
        then { inherit name; value = pkgs-at-rev.${name}; }
        else null;
    in
    builtins.listToAttrs (builtins.filter (x: x != null)
      (builtins.attrValues (builtins.mapAttrs mkFrozen freezes))))"#.cyan());
    println!();
    println!(
        "Then create {} with content: {}",
        "package-freezes.json".bold(),
        "{}".cyan()
    );
}

#[cfg(test)]
#[path = "tests/init.rs"]
mod tests;
