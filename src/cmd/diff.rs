//! `cheni diff` command.
//!
//! Compares two NixOS generations to show package changes.

use std::process::Command;

use anyhow::Result;
use colored::Colorize;

/// Run `cheni diff <gen1> <gen2>`.
///
/// Compares two generations using nvd (preferred) or nix store diff-closures.
pub fn run(from: u32, to: u32) -> Result<()> {
    println!(
        "{}\n",
        format!("=== cheni diff {} → {} ===", from, to).bold()
    );

    let from_path = format!("/nix/var/nix/profiles/system-{}-link", from);
    let to_path = format!("/nix/var/nix/profiles/system-{}-link", to);

    // Check both generations exist
    if !std::path::Path::new(&from_path).exists() {
        anyhow::bail!(
            "Generation {} not found.\nRun '{}' to list available generations.",
            from,
            "cheni history".bold()
        );
    }
    if !std::path::Path::new(&to_path).exists() {
        anyhow::bail!(
            "Generation {} not found.\nRun '{}' to list available generations.",
            to,
            "cheni history".bold()
        );
    }

    // Try nvd first (much nicer output)
    let nvd = Command::new("nvd")
        .args(["diff", &from_path, &to_path])
        .status();

    match nvd {
        Ok(s) if s.success() => return Ok(()),
        _ => {}
    }

    // Fallback: nix store diff-closures
    println!("{}", "(nvd not available, using nix store diff-closures)\n".dimmed());

    let status = Command::new("nix")
        .args(["store", "diff-closures", &from_path, &to_path])
        .status()
        .map_err(|e| crate::nix::tools::tool_error("nix", e))?;

    if !status.success() {
        anyhow::bail!("Diff command failed");
    }

    Ok(())
}
