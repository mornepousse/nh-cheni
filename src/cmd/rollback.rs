//! `cheni rollback` command.
//!
//! Rolls back to the previous NixOS generation (or a specific one),
//! after showing a human-readable summary of what's changing.

use std::process::Command;

use anyhow::{anyhow, Context, Result};
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Confirm};

use super::history::{read_generations, Generation};

/// Run `cheni rollback [target]`.
///
/// When `target` is `None`, picks the generation immediately preceding
/// the currently active one. When it's `Some(n)`, uses generation `n`
/// — validated against the list of generations actually present on
/// disk, so `cheni rollback 9999` fails early with a clear message
/// instead of a cryptic nix-env error.
///
/// The function prints a "from → to" summary (numbers + dates + NixOS
/// labels), asks for confirmation (`--yes` bypasses), then performs
/// the switch with sudo.
pub fn run(target: Option<u32>, yes: bool) -> Result<()> {
    println!("{}\n", "=== cheni rollback ===".bold());

    let gens = read_generations()?;
    let current = gens
        .iter()
        .find(|g| g.is_current)
        .ok_or_else(|| anyhow!("no currently-active generation found"))?;
    let target_gen = resolve_target(&gens, current, target)?;

    print_summary(current, target_gen);

    if !yes && !confirm_rollback()? {
        println!("{}", "  Cancelled — nothing changed.".yellow());
        return Ok(());
    }

    apply_rollback(target)?;

    println!("\n{} Rolled back successfully!", "✓".green());
    println!(
        "  Run '{}' to see all generations.",
        "cheni history".bold()
    );
    Ok(())
}

/// Pick the target generation from the listing.
///
/// - `Some(n)` → return the generation with that number or error.
/// - `None` → return the highest-numbered generation strictly below
///   the current one (the "previous" generation, skipping gaps that
///   can occur after `cheni history --prune`).
fn resolve_target<'a>(
    gens: &'a [Generation],
    current: &Generation,
    target: Option<u32>,
) -> Result<&'a Generation> {
    match target {
        Some(n) if n == current.number => Err(anyhow!(
            "generation {} is already active — nothing to do",
            n
        )),
        Some(n) => gens
            .iter()
            .find(|g| g.number == n)
            .ok_or_else(|| anyhow!("generation {} not found (run `cheni history` to list available)", n)),
        None => gens
            .iter()
            .rev()
            .find(|g| g.number < current.number)
            .ok_or_else(|| anyhow!("no previous generation available — this is the oldest one")),
    }
}

fn print_summary(current: &Generation, target: &Generation) {
    println!("  {}", "Rollback preview:".bold());
    println!(
        "    current : gen {} ({})",
        current.number.to_string().bold(),
        current.date
    );
    if let Some(label) = &current.nixos_label {
        println!("              {}", label.dimmed());
    }
    println!(
        "    target  : gen {} ({})",
        target.number.to_string().bold().cyan(),
        target.date
    );
    if let Some(label) = &target.nixos_label {
        println!("              {}", label.dimmed());
    }

    let delta = current.number as i64 - target.number as i64;
    let direction = if delta > 0 { "back" } else { "forward" };
    println!(
        "\n  Moving {} {} generation{}.",
        direction,
        delta.abs(),
        if delta.abs() == 1 { "" } else { "s" }
    );
    println!(
        "  {} The current generation stays in the store until next GC,",
        "·".dimmed()
    );
    println!(
        "  {} so you can `cheni rollback {}` to return.",
        "·".dimmed(),
        current.number
    );
    println!();
}

fn confirm_rollback() -> Result<bool> {
    let theme = ColorfulTheme::default();
    Confirm::with_theme(&theme)
        .with_prompt("Proceed with rollback?")
        .default(false)
        .interact()
        .context("reading confirmation")
}

/// Execute the actual switch. Two paths:
///
/// - `target: None` → `nixos-rebuild switch --rollback` (native path;
///   nixos-rebuild handles activation itself).
/// - `target: Some(n)` → `nix-env --switch-generation n` then
///   `switch-to-configuration switch` to activate.
fn apply_rollback(target: Option<u32>) -> Result<()> {
    println!("{}", "  Requires sudo for system switch.\n".dimmed());

    let cmd_args: Vec<String> = match target {
        None => vec![
            "/run/current-system/sw/bin/nixos-rebuild".to_string(),
            "switch".to_string(),
            "--rollback".to_string(),
        ],
        Some(num) => vec![
            "/run/current-system/sw/bin/nix-env".to_string(),
            "-p".to_string(),
            "/nix/var/nix/profiles/system".to_string(),
            "--switch-generation".to_string(),
            num.to_string(),
        ],
    };

    let status = Command::new("sudo")
        .args(&cmd_args)
        .status()
        .map_err(|e| crate::nix::tools::tool_error("sudo", e))
        .context("running rollback")?;
    if !status.success() {
        anyhow::bail!("Rollback failed");
    }

    if target.is_some() {
        println!("\n  Activating generation...");
        let activate_status = Command::new("sudo")
            .args([
                "/nix/var/nix/profiles/system/bin/switch-to-configuration",
                "switch",
            ])
            .status()
            .map_err(|e| crate::nix::tools::tool_error("sudo", e))
            .context("activating generation")?;
        if !activate_status.success() {
            anyhow::bail!("Activation failed — system may be in inconsistent state");
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "tests/rollback.rs"]
mod tests;
