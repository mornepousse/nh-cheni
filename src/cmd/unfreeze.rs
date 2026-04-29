//! `cheni unfreeze` command.
//!
//! Releases packages held by `cheni freeze`, routing them back through
//! the regular `nixpkgs` input. Next `cheni upgrade` will move them to
//! whatever the rest of the system is on.

use anyhow::Result;
use colored::Colorize;

use crate::nix::{config, freezes, flake};
use crate::util::confirm;

/// Run `cheni unfreeze <package>`.
pub fn unfreeze_one(name: &str, yes: bool) -> Result<()> {
    let nix_config = config::detect()?;
    let current = freezes::read(&nix_config.flake_dir)?;

    let Some(entry) = current.get(name).cloned() else {
        println!("'{}' was not frozen.", name);
        return Ok(());
    };

    println!("{}\n", "=== cheni unfreeze ===".bold());
    println!(
        "  {} is currently held at {} (since {}, rev {}).",
        name.bold(),
        entry.version.dimmed(),
        entry.frozen_at.dimmed(),
        flake::short_hash(&entry.rev).dimmed()
    );
    println!(
        "  Unfreezing routes {} back through plain nixpkgs. Next '{}' will move it",
        name,
        "cheni upgrade".bold()
    );
    println!("  to whatever version nixpkgs is on at that moment.");
    println!();

    if !yes && !confirm(&format!("Unfreeze {}?", name), false)? {
        println!("{}", "  Cancelled — freeze kept.".yellow());
        return Ok(());
    }

    let removed = freezes::remove(&nix_config.flake_dir, &[name.to_string()])?;
    if removed.is_empty() {
        // Race window: someone else removed the freeze between read and write.
        println!("'{}' was not frozen.", name);
    } else {
        crate::nix::timeline::record("unfreeze", Some(name), serde_json::json!({}));
        println!("{} Unfroze {}.", "✓".green(), name.bold());
        println!("Run '{}' to apply.", "cheni build".bold());
    }
    Ok(())
}

/// Run `cheni unfreeze --all`.
pub fn unfreeze_all(yes: bool) -> Result<()> {
    let nix_config = config::detect()?;
    let current = freezes::read(&nix_config.flake_dir)?;

    if current.is_empty() {
        println!("{} No freezes to remove.", "✓".green());
        return Ok(());
    }

    println!("{}\n", "=== cheni unfreeze ===".bold());
    println!(
        "  This will release {} {}:",
        current.len().to_string().bold(),
        crate::util::pluralize(current.len(), "freeze")
    );

    let total = current.len();
    for (idx, (name, entry)) in current.iter().enumerate() {
        let glyph = crate::util::tree_glyph(idx, total);
        println!(
            "    {} {:<28} {}",
            glyph.dimmed(),
            name.bold(),
            entry.version.dimmed()
        );
    }
    println!();
    println!(
        "  All of these will be routed back through plain nixpkgs. Next '{}'",
        "cheni upgrade".bold()
    );
    println!("  will move them to whatever version nixpkgs is on.");
    println!();

    if !yes && !confirm(&format!("Unfreeze all {}?", crate::util::count_phrase(current.len(), "freeze")), false)? {
        println!("{}", "  Cancelled — freezes kept.".yellow());
        return Ok(());
    }

    let count = freezes::clear(&nix_config.flake_dir)?;
    for name in current.keys() {
        crate::nix::timeline::record("unfreeze", Some(name), serde_json::json!({}));
    }
    println!(
        "{} Removed {} {}.",
        "✓".green(),
        count.to_string().bold(),
        crate::util::pluralize(count, "freeze")
    );
    println!("Run '{}' to apply.", "cheni build".bold());
    Ok(())
}

// `confirm` and `short_rev` removed — use `crate::util::confirm`
// and `crate::nix::flake::short_hash` directly.

#[cfg(test)]
#[path = "tests/unfreeze.rs"]
mod tests;
