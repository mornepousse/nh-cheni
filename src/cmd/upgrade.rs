//! `cheni upgrade` command.
//!
//! Full system upgrade: update all flake inputs, rebuild, clean
//! obsolete pins, and optionally garbage-collect old generations.

use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::debug;

use crate::nix::{config, pins};

/// Options for `cheni upgrade`.
pub struct UpgradeOptions {
    /// Run garbage collection after the rebuild (default: off).
    /// This DELETES old generations — you won't be able to rollback!
    pub gc: bool,
    /// Skip cleanup of obsolete pins.
    pub no_clean_pins: bool,
    /// Skip the preview + confirmation step.
    pub yes: bool,
}

/// Run `cheni upgrade`.
///
/// Full system upgrade:
/// 1. Update all flake inputs (`nix flake update`)
/// 2. Rebuild the system (`nh os switch`)
/// 3. Clean obsolete pins (`cheni clean` logic)
/// 4. (optional, with --gc) Garbage-collect old generations
pub fn run(opts: UpgradeOptions) -> Result<()> {
    let nix_config = config::detect()?;
    let config_path = nix_config.flake_dir.to_str()
        .context("Config path is not valid UTF-8")?;

    println!("{}\n", "=== cheni upgrade ===".bold());

    update_flake_inputs(&nix_config.flake_dir)?;

    if !opts.yes && !preview_and_confirm(config_path, &nix_config.hostname)? {
        return Ok(());
    }

    rebuild_system(config_path)?;
    run_pin_cleanup_step(&nix_config.flake_dir, opts.no_clean_pins)?;
    if opts.gc {
        run_gc_step()?;
    }

    println!("\n{} Upgrade complete!", "✓".green());
    if !opts.gc {
        println!(
            "{}",
            "Old generations kept for rollback. Use --gc to reclaim disk space later.".dimmed()
        );
    }
    Ok(())
}

/// Step 1: refresh every flake input. Bails if `nix flake update` fails.
fn update_flake_inputs(flake_dir: &Path) -> Result<()> {
    println!("{} Updating all flake inputs...", "[1/4]".dimmed());
    let status = Command::new("nix")
        .args(["flake", "update"])
        .current_dir(flake_dir)
        .status()
        .map_err(|e| crate::nix::tools::tool_error("nix", e))?;
    if !status.success() {
        anyhow::bail!("nix flake update failed");
    }
    Ok(())
}

/// Step 1.5: evaluate pending changes via `nix build --dry-run`, show a
/// human summary, then ask for confirmation.
///
/// Returns Ok(true) if the caller should proceed with the rebuild,
/// Ok(false) if the user either cancelled or there's nothing to do.
fn preview_and_confirm(config_path: &str, hostname: &str) -> Result<bool> {
    println!("\n{} Evaluating changes (no download)...\n", "[preview]".dimmed());

    let flake_ref = format!(
        "{}#nixosConfigurations.{}.config.system.build.toplevel",
        config_path, hostname
    );
    let preview_output = Command::new("nix")
        .args(["build", &flake_ref, "--dry-run", "--no-link", "--print-build-logs"])
        .stderr(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .output()
        .map_err(|e| crate::nix::tools::tool_error("nix", e))?;

    if !preview_output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&preview_output.stderr));
        anyhow::bail!("Preview evaluation failed. Run 'cheni build' to see details.");
    }

    // nix --dry-run prints its summary on stderr.
    let stderr = String::from_utf8_lossy(&preview_output.stderr);
    let (to_build, to_fetch) = parse_dry_run_summary(&stderr);

    if to_build.is_empty() && to_fetch.is_empty() {
        println!("  {}", "Nothing to build or download — already up to date.".green());
        println!("\n{}", "No changes to apply.".dimmed());
        return Ok(false);
    }

    print_preview_lists(&to_build, &to_fetch);

    println!();
    if !confirm("Download and apply these changes?")? {
        println!("\n{}", "Upgrade cancelled. Flake is already updated.".yellow());
        println!("  Use '{}' to rebuild later.", "cheni upgrade --yes".bold());
        return Ok(false);
    }
    Ok(true)
}

/// Print the "to fetch" and "to build" lists, each truncated so the
/// terminal doesn't get flooded on a big update.
fn print_preview_lists(to_build: &[String], to_fetch: &[String]) {
    if !to_fetch.is_empty() {
        println!("  {} {} package(s) to download:", "↓".cyan(), to_fetch.len());
        for pkg in to_fetch.iter().take(20) {
            println!("    {}", pkg.dimmed());
        }
        if to_fetch.len() > 20 {
            println!("    {} and {} more...", "...".dimmed(), to_fetch.len() - 20);
        }
    }
    if !to_build.is_empty() {
        println!("\n  {} {} package(s) to build locally:", "⚒".yellow(), to_build.len());
        for pkg in to_build.iter().take(10) {
            println!("    {}", pkg.dimmed());
        }
        if to_build.len() > 10 {
            println!("    {} and {} more...", "...".dimmed(), to_build.len() - 10);
        }
    }
}

/// Step 2: invoke `nh os switch` with the activation step inline.
///
/// Uses the merged-pipe streamer so `/nix/store/<hash>-...` noise is
/// stripped from the output live. On failure, the raw (non-prettified)
/// buffer is fed to the diagnose pattern library so the user gets an
/// actionable hint along with the raw error.
fn rebuild_system(config_path: &str) -> Result<()> {
    println!("\n{} Rebuilding system...\n", "[2/4]".dimmed());
    let out = crate::output::stream::run_streaming(
        "nh",
        &["os", "switch", config_path],
        None,
    )?;
    if !out.status.success() {
        crate::cmd::diagnose::print_hints_for(&out.raw_buffer);
        anyhow::bail!("System rebuild failed. Fix the issue and run 'cheni build' again.");
    }
    Ok(())
}

/// Step 3: either clean obsolete pins or announce the skip — `no_clean`
/// decides which branch is taken so the step label stays aligned.
fn run_pin_cleanup_step(flake_dir: &Path, no_clean: bool) -> Result<()> {
    if no_clean {
        println!("\n{} {}", "[3/4]".dimmed(), "Skipping pin cleanup (--no-clean-pins)".dimmed());
        return Ok(());
    }
    println!("\n{} Checking for obsolete pins...", "[3/4]".dimmed());
    clean_obsolete_pins(flake_dir)
}

/// Step 4: GC generations older than 30 days (only when --gc is set —
/// the rollback guarantee comes from keeping this off by default).
fn run_gc_step() -> Result<()> {
    println!(
        "\n{} {}",
        "[4/4]".dimmed(),
        "Collecting garbage (generations > 30 days)...".yellow()
    );
    println!(
        "  {} This will delete old generations — rollback won't work past this point!",
        "!".yellow()
    );
    let status = Command::new("sudo")
        .args(["nix-collect-garbage", "--delete-older-than", "30d"])
        .status()
        .map_err(|e| crate::nix::tools::tool_error("sudo", e))
        .context("running nix-collect-garbage")?;
    if !status.success() {
        println!("{}", "  (garbage collection skipped or failed)".dimmed());
    }
    Ok(())
}

/// Parse the summary output of `nix build --dry-run`.
///
/// Returns (to_build, to_fetch) — lists of package names.
/// Example output:
///   these 3 derivations will be built:
///     /nix/store/abc-foo-1.0.drv
///     /nix/store/def-bar-2.0.drv
///   these 5 paths will be fetched (12.3 MiB download, ...):
///     /nix/store/xyz-baz-3.0
fn parse_dry_run_summary(stderr: &str) -> (Vec<String>, Vec<String>) {
    let mut to_build = Vec::new();
    let mut to_fetch = Vec::new();

    enum Section {
        None,
        Build,
        Fetch,
    }
    let mut section = Section::None;

    for line in stderr.lines() {
        let trimmed = line.trim();

        // Detect section headers
        if trimmed.contains("derivations will be built") || trimmed.contains("derivation will be built") {
            section = Section::Build;
            continue;
        }
        if trimmed.contains("paths will be fetched") || trimmed.contains("path will be fetched") {
            section = Section::Fetch;
            continue;
        }

        // Parse store paths: /nix/store/<hash>-<name>
        if trimmed.starts_with("/nix/store/") {
            if let Some(name) = extract_store_name(trimmed) {
                match section {
                    Section::Build => to_build.push(name),
                    Section::Fetch => to_fetch.push(name),
                    Section::None => {}
                }
            }
        } else if !trimmed.is_empty() && !trimmed.starts_with("/nix/store/") {
            // Section ended
            section = Section::None;
        }
    }

    (to_build, to_fetch)
}

/// Extract package name + version from a store path.
/// e.g. "/nix/store/abc123-vivaldi-7.9.drv" -> "vivaldi-7.9"
fn extract_store_name(path: &str) -> Option<String> {
    let after_prefix = path.strip_prefix("/nix/store/")?;
    // Skip 32-char hash + hyphen
    if after_prefix.len() < 34 {
        return None;
    }
    let name = &after_prefix[33..];
    // Strip trailing .drv
    Some(name.trim_end_matches(".drv").to_string())
}

/// Ask a yes/no question. Default is yes.
fn confirm(question: &str) -> Result<bool> {
    print!("{} {} ", question, "[Y/n]".dimmed());
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let answer = input.trim().to_lowercase();

    Ok(answer.is_empty() || answer == "y" || answer == "yes")
}

/// Remove pins that are now obsolete (nixpkgs caught up with nixpkgs-latest).
fn clean_obsolete_pins(flake_dir: &Path) -> Result<()> {
    let current_pins = pins::read(flake_dir)?;

    if current_pins.is_empty() {
        println!("  No pins to check.");
        return Ok(());
    }

    let lock_path = flake_dir.join("flake.lock");
    let obsolete = super::obsolete::count_obsolete_pins(&lock_path, &current_pins);

    if obsolete == 0 {
        println!("  All {} pin(s) still needed.", current_pins.len());
        return Ok(());
    }

    // If nixpkgs caught up, all pins are obsolete — clear them all
    let removed = pins::clear(flake_dir)?;
    println!(
        "  {} Removed {} obsolete pin(s).",
        "✓".green(),
        removed
    );
    debug!("Cleaned {} obsolete pins", removed);

    Ok(())
}
