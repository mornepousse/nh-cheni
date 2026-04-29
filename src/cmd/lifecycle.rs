//! `cheni promote` and `cheni demote` — flip pins ↔ freezes.
//!
//! See `docs/superpowers/specs/2026-04-28-cheni-lifecycle-design.md`.

use anyhow::{bail, Result};
use colored::Colorize;

use crate::nix::{config, flake, freezes, pins, store};

/// Validate the preconditions for `cheni promote`.
///
/// Returns `Ok(())` when promote is safe to proceed, or a descriptive
/// error message to display to the user. Split out as a pure function
/// so it can be unit-tested without touching any files.
pub(crate) fn validate_promote_preconditions(
    name: &str,
    current_freezes: &freezes::Freezes,
    current_pins: &[String],
) -> Result<()> {
    if !current_freezes.contains_key(name) {
        bail!(
            "{} is not currently frozen. Run `cheni freeze {}` first if you want to freeze it, \
             or `cheni pin {}` if you want to pin it directly.",
            name,
            name,
            name
        );
    }
    if current_pins.iter().any(|p| p == name) {
        bail!(
            "{} appears in both pins and freezes — inconsistent state. \
             Run `cheni doctor` to diagnose.",
            name
        );
    }
    Ok(())
}

/// Validate the preconditions for `cheni demote`.
///
/// Returns `Ok(())` when demote is safe to proceed, or a descriptive
/// error message to display to the user. Split out as a pure function
/// so it can be unit-tested without touching any files.
pub(crate) fn validate_demote_preconditions(
    name: &str,
    current_pins: &[String],
    current_freezes: &freezes::Freezes,
) -> Result<()> {
    if !current_pins.iter().any(|p| p == name) {
        bail!(
            "{} is not currently pinned. Run `cheni pin {}` first if you want to pin it, \
             or `cheni freeze {}` if you want to freeze it directly.",
            name,
            name,
            name
        );
    }
    if current_freezes.contains_key(name) {
        bail!(
            "{} appears in both pins and freezes — inconsistent state. \
             Run `cheni doctor` to diagnose.",
            name
        );
    }
    Ok(())
}

/// Promote `name` from a freeze to a pin: removes the freeze, adds
/// the pin. Next rebuild will route the package via nixpkgs-latest
/// instead of holding the frozen version.
pub fn promote(name: &str, yes: bool) -> Result<()> {
    let nix_config = config::detect()?;

    let current_freezes = freezes::read(&nix_config.flake_dir)?;
    let current_pins = pins::read(&nix_config.flake_dir)?;

    validate_promote_preconditions(name, &current_freezes, &current_pins)?;

    if !yes {
        let theme = dialoguer::theme::ColorfulTheme::default();
        let go = dialoguer::Confirm::with_theme(&theme)
            .with_prompt(format!(
                "Promote {} from freeze to pin? (next rebuild will update via nixpkgs-latest)",
                name
            ))
            .default(true)
            .interact()
            .map_err(|e| anyhow::anyhow!("reading confirmation: {e}"))?;
        if !go {
            println!("{}", "  Cancelled — nothing changed.".yellow());
            return Ok(());
        }
    }

    freezes::remove(&nix_config.flake_dir, &[name.to_string()])?;
    pins::add(&nix_config.flake_dir, &[name.to_string()])?;

    println!(
        "{} Promoted {} from freeze to pin.",
        "✓".green(),
        name.bold()
    );
    println!(
        "  {} Next rebuild will route {} via nixpkgs-latest.",
        "·".dimmed(),
        name
    );
    Ok(())
}

/// Demote `name` from a pin to a freeze: reads the currently-installed
/// version, removes the pin, adds a freeze locked to that version.
/// Next rebuild will hold the package at the frozen version.
pub fn demote(name: &str, yes: bool) -> Result<()> {
    let nix_config = config::detect()?;

    let current_pins = pins::read(&nix_config.flake_dir)?;
    let current_freezes = freezes::read(&nix_config.flake_dir)?;

    validate_demote_preconditions(name, &current_pins, &current_freezes)?;

    let store_pkg = store::find_by_name(name).map_err(|e| {
        anyhow::anyhow!(
            "can't demote {}: not found in the store. Build first or unpin instead. ({})",
            name,
            e
        )
    })?;

    let Some((rev, nar_hash)) = flake::read_input_locked(&nix_config.flake_dir, "nixpkgs-latest")
    else {
        bail!(
            "no `nixpkgs-latest` input found in flake.lock — run `cheni init` to set it up."
        );
    };

    if !yes {
        let theme = dialoguer::theme::ColorfulTheme::default();
        let go = dialoguer::Confirm::with_theme(&theme)
            .with_prompt(format!(
                "Demote {} from pin to freeze at version {}?",
                name, store_pkg.version
            ))
            .default(true)
            .interact()
            .map_err(|e| anyhow::anyhow!("reading confirmation: {e}"))?;
        if !go {
            println!("{}", "  Cancelled — nothing changed.".yellow());
            return Ok(());
        }
    }

    let entry = freezes::FreezeEntry {
        rev,
        nar_hash,
        version: store_pkg.version.clone(),
        frozen_at: crate::cmd::freeze::today_iso(),
        major_constraint: None,
    };

    pins::remove(&nix_config.flake_dir, &[name.to_string()])?;
    freezes::add(&nix_config.flake_dir, name, entry)?;

    println!(
        "{} Demoted {} from pin to freeze at {}.",
        "✓".green(),
        name.bold(),
        store_pkg.version.dimmed()
    );
    println!(
        "  {} Next rebuild will hold {} at this version.",
        "·".dimmed(),
        name
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests/lifecycle.rs"]
mod tests;
