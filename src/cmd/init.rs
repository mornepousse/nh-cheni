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

    println!("{}\n", "=== cheni init ===".bold());
    println!("  Config:   {}", flake_dir.display());
    println!("  Hostname: {}\n", nix_config.hostname);

    // Step 1: Create package-pins.json if missing
    let _pins_created = create_pins_file(flake_dir)?;

    // Step 2: Add nixpkgs-latest input to flake.nix
    let flake_path = flake_dir.join("flake.nix");
    let flake_content = std::fs::read_to_string(&flake_path)
        .context("Failed to read flake.nix")?;

    // Check if already initialized
    if flake_content.contains("nixpkgs-latest") {
        println!("{} nixpkgs-latest already in flake.nix.", "[1/2]".dimmed());
    } else {
        match add_nixpkgs_latest(&flake_path, &flake_content) {
            Ok(()) => {
                println!(
                    "{} Added nixpkgs-latest input to flake.nix.  {}",
                    "[1/2]".dimmed(),
                    "OK".green()
                );
            }
            Err(e) => {
                warn!("Auto-modification failed: {}", e);
                println!(
                    "{} Could not auto-modify flake.nix.  {}",
                    "[1/2]".dimmed(),
                    "MANUAL".yellow()
                );
                print_manual_instructions(&nix_config.hostname);
                return Ok(());
            }
        }
    }

    // Check if overlay is present
    if flake_content.contains("package-pins.json") {
        println!("{} Overlay already configured.", "[2/2]".dimmed());
    } else {
        // Re-read flake after step 1 modification
        let flake_content = std::fs::read_to_string(&flake_path)
            .context("Failed to re-read flake.nix")?;

        match add_overlay(&flake_path, &flake_content, &nix_config.hostname) {
            Ok(()) => {
                println!(
                    "{} Added cheni overlay to flake.nix.       {}",
                    "[2/2]".dimmed(),
                    "OK".green()
                );
            }
            Err(e) => {
                warn!("Overlay auto-modification failed: {}", e);
                println!(
                    "{} Could not add overlay automatically.  {}",
                    "[2/2]".dimmed(),
                    "MANUAL".yellow()
                );
                print_overlay_instructions(&nix_config.hostname);
                return Ok(());
            }
        }
    }

    println!(
        "\n{} cheni is ready! Try '{}'.",
        "✓".green(),
        "cheni check".bold()
    );

    Ok(())
}

/// Create package-pins.json if it doesn't exist.
fn create_pins_file(flake_dir: &Path) -> Result<bool> {
    let path = flake_dir.join("package-pins.json");

    if path.exists() {
        debug!("package-pins.json already exists");
        return Ok(false);
    }

    std::fs::write(&path, "[]\n")
        .context("Failed to create package-pins.json")?;

    debug!("Created package-pins.json");
    Ok(true)
}

/// Add `nixpkgs-latest` input to flake.nix.
///
/// Strategy: find the `inputs = {` block and insert after the nixpkgs line.
fn add_nixpkgs_latest(flake_path: &Path, content: &str) -> Result<()> {
    // Look for the nixpkgs input line
    let nixpkgs_patterns = [
        "nixpkgs.url",
        "nixpkgs = {",
    ];

    let mut insert_after_line = None;

    for (i, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        for pattern in &nixpkgs_patterns {
            if trimmed.contains(pattern) {
                // Find the end of this input declaration
                // For single-line: nixpkgs.url = "...";
                // For multi-line: find the closing };
                if trimmed.ends_with(';') || trimmed.ends_with("\";") {
                    insert_after_line = Some(i);
                } else {
                    // Multi-line: scan forward for the closing
                    for j in (i + 1)..content.lines().count() {
                        let next_line = content.lines().nth(j).unwrap_or("");
                        if next_line.trim().starts_with("};") || next_line.trim() == "};" {
                            insert_after_line = Some(j);
                            break;
                        }
                    }
                    if insert_after_line.is_none() {
                        insert_after_line = Some(i);
                    }
                }
                break;
            }
        }
        if insert_after_line.is_some() {
            break;
        }
    }

    let insert_line = insert_after_line
        .context("Could not find nixpkgs input in flake.nix")?;

    debug!("Inserting nixpkgs-latest after line {}", insert_line + 1);

    // Build the new content
    let lines: Vec<&str> = content.lines().collect();
    let mut new_lines: Vec<String> = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        new_lines.push(line.to_string());
        if i == insert_line {
            new_lines.push(String::new());
            new_lines.push(
                "    # nixpkgs-latest: independently updated for per-package updates (cheni)"
                    .to_string(),
            );
            new_lines.push(
                "    nixpkgs-latest.url = \"github:NixOS/nixpkgs/nixos-unstable\";"
                    .to_string(),
            );
        }
    }

    let new_content = new_lines.join("\n") + "\n";
    std::fs::write(flake_path, new_content)
        .context("Failed to write modified flake.nix")?;

    Ok(())
}

/// Add the cheni overlay to the nixosSystem modules.
///
/// Strategy: find the `nixpkgs.overlays = [` block for the matching
/// hostname and add the cheni overlay.
fn add_overlay(flake_path: &Path, content: &str, _hostname: &str) -> Result<()> {
    // Look for the overlay block in the matching nixosConfiguration
    // We look for `nixpkgs.overlays = [` within the hostname's section
    let overlay_code = r#"              # cheni: per-package updates from nixpkgs-latest
              (let
                pkgs-latest = import inputs.nixpkgs-latest {
                  system = "x86_64-linux";
                  config.allowUnfree = true;
                };
                pins = builtins.fromJSON (builtins.readFile ./package-pins.json);
              in final: prev: builtins.listToAttrs (builtins.filter (x: x != null) (map (name:
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
    std::fs::write(flake_path, new_content)
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
    println!("{}", r#"  ({ config, pkgs, ... }:
  let
    pkgs-latest = import inputs.nixpkgs-latest {
      system = "x86_64-linux";
      config.allowUnfree = true;
    };
    pins = builtins.fromJSON (builtins.readFile ./package-pins.json);
  in {
    nixpkgs.overlays = [
      # cheni: pinned packages from nixpkgs-latest
      (final: prev: builtins.listToAttrs (builtins.filter (x: x != null) (map (name:
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
