//! `cheni rollback` command.
//!
//! Rolls back to the previous NixOS generation (or a specific one),
//! after showing a human-readable summary of what's changing.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, UNIX_EPOCH};

use anyhow::{anyhow, Context, Result};
use colored::Colorize;

use super::history::{
    compute_pin_freeze_delta, format_pin_freeze_delta, read_generations, Generation,
};
use crate::util;

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
    let started = Instant::now();
    println!("{}\n", "=== cheni rollback ===".bold());

    let gens = read_generations()?;
    let current = gens
        .iter()
        .find(|g| g.is_current)
        .ok_or_else(|| anyhow!("no currently-active generation found"))?;
    let target_gen = resolve_target(&gens, current, target)?;

    print_summary(current, target_gen);

    // Optional cross-context warning: if the user's pin/freeze policy
    // has drifted since `target`, the rollback restores binaries that
    // were built under a different policy than the one currently on
    // disk. We show the delta so the user can decide whether the
    // mismatch matters before confirming.
    if let Some(flake_dir) = policy_drift_dir() {
        print_policy_drift(target_gen, &flake_dir);
    }

    // `default_yes = false` — rollback is a destructive operation
    // (sudo + switches the running system generation). Safer to make
    // the user explicitly type 'y' than to accept a stray Enter.
    if !yes && !util::confirm("Proceed with rollback?", false)? {
        println!("{}", "  Cancelled — nothing changed.".yellow());
        return Ok(());
    }

    apply_rollback(target)?;

    println!(
        "\n{} {} in {} — now on generation {}.",
        "✓".green().bold(),
        "Rollback complete".bold(),
        format_elapsed(started.elapsed()).dimmed(),
        target_gen.number.to_string().bold()
    );
    println!(
        "  Run '{}' to see all generations.",
        "cheni history".bold()
    );
    Ok(())
}

/// Resolve the flake directory for policy-drift annotation, gated on
/// it being a git work tree. Same shape as the `cheni history` helper
/// — duplicated rather than shared so each command stays free to
/// evolve its own gating without coupling.
fn policy_drift_dir() -> Option<PathBuf> {
    let dir = crate::nix::config::detect().ok()?.flake_dir;
    if !crate::nix::git::is_repo(&dir) {
        return None;
    }
    Some(dir)
}

/// Print the policy-drift block when the pins/freezes state has moved
/// between `target.mtime` and now.
///
/// The wording deliberately spells out the binaries-vs-policy split:
/// users new to the overlay model often expect rollback to revert the
/// pin/freeze JSON files too, and the resulting "why is X still pinned
/// after rollback?" confusion is exactly what this warning prevents.
fn print_policy_drift(target: &Generation, flake_dir: &Path) {
    let Some(t_secs) = target.mtime_secs else { return };
    let target_at = UNIX_EPOCH + Duration::from_secs(t_secs);

    let target_pins = crate::nix::pins::read_at_time(flake_dir, target_at);
    let target_freezes = crate::nix::freezes::read_at_time(flake_dir, target_at);
    let cur_pins = crate::nix::pins::read(flake_dir).unwrap_or_default();
    let cur_freezes = crate::nix::freezes::read(flake_dir).unwrap_or_default();

    let Some(delta) = compute_pin_freeze_delta(
        &target_pins,
        &cur_pins,
        &target_freezes,
        &cur_freezes,
    ) else {
        return;
    };

    println!("  {}", "Policy drifted since target:".yellow());
    println!("    {}", format_pin_freeze_delta(&delta).dimmed());
    println!(
        "    {}",
        "Rollback only swaps binaries; current pins/freezes stay in effect on next rebuild."
            .dimmed()
    );
    println!();
}

/// Local alias to the shared `crate::util::format_elapsed`.
fn format_elapsed(d: std::time::Duration) -> String {
    crate::util::format_elapsed(d)
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
        match target {
            None => anyhow::bail!(
                "Rollback failed (nixos-rebuild --rollback). The previous generation may itself be broken — try `cheni history` and pick an older one with `cheni rollback <N>`."
            ),
            Some(n) => anyhow::bail!(
                "Switching to generation {} failed. Confirm it still exists with `cheni history`.",
                n
            ),
        }
    }

    if let Some(n) = target {
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
            anyhow::bail!(
                "Activation failed — the kernel/init switched but switch-to-configuration refused. \
                 Reboot to land in generation {} cleanly, or run `cheni rollback` to return to the previous one.",
                n
            );
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "tests/rollback.rs"]
mod tests;
