//! `cheni rollback` command.
//!
//! Rolls back to the previous NixOS generation (or a specific one).

use std::process::Command;

use anyhow::{Context, Result};
use colored::Colorize;

/// Run `cheni rollback`.
///
/// Switches the system back to a previous generation.
/// If `target` is None, rolls back to the previous generation.
/// Otherwise rolls back to the specified generation number.
pub fn run(target: Option<u32>) -> Result<()> {
    println!("{}\n", "=== cheni rollback ===".bold());

    let cmd_args: Vec<String> = match target {
        None => {
            println!("  Rolling back to the previous generation...");
            vec![
                "/run/current-system/sw/bin/nixos-rebuild".to_string(),
                "switch".to_string(),
                "--rollback".to_string(),
            ]
        }
        Some(num) => {
            println!("  Rolling back to generation {}...", num.to_string().bold());
            vec![
                "/run/current-system/sw/bin/nix-env".to_string(),
                "-p".to_string(),
                "/nix/var/nix/profiles/system".to_string(),
                "--switch-generation".to_string(),
                num.to_string(),
            ]
        }
    };

    println!("{}", "  Requires sudo for system switch.\n".dimmed());

    let mut cmd = Command::new("sudo");
    cmd.args(&cmd_args);

    let status = cmd.status()
        .map_err(|e| crate::nix::tools::tool_error("sudo", e))
        .context("running rollback")?;

    if !status.success() {
        anyhow::bail!("Rollback failed");
    }

    // For --switch-generation we also need to activate
    if target.is_some() {
        println!("\n  Activating generation...");
        let activate_status = Command::new("sudo")
            .args(["/nix/var/nix/profiles/system/bin/switch-to-configuration", "switch"])
            .status()
            .map_err(|e| crate::nix::tools::tool_error("sudo", e))
            .context("activating generation")?;

        if !activate_status.success() {
            anyhow::bail!("Activation failed — system may be in inconsistent state");
        }
    }

    println!("\n{} Rolled back successfully!", "✓".green());
    println!(
        "  Run '{}' to see all generations.",
        "cheni history".bold()
    );

    Ok(())
}
