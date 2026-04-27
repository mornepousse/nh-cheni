//! `cheni diff` command.
//!
//! Compares two NixOS generations to show package changes, with an
//! optional pin/freeze policy-delta header that surfaces cheni-only
//! state (`nvd` and `nix store diff-closures` only see store paths,
//! not the policy that produced them).

use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use colored::Colorize;

use super::history::{compute_pin_freeze_delta, format_pin_freeze_delta};

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

    // Optional cross-context header — surfaces what changed in the
    // user's policy file across the gen pair. Silent on no drift /
    // no flake / non-git config dir, so the diff command stays a
    // thin wrapper in the common case.
    print_policy_delta_header(&from_path, &to_path);

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
        anyhow::bail!(
            "nix store diff-closures failed (gens {} → {}). Check both generations still exist with `cheni history`.",
            from,
            to
        );
    }

    Ok(())
}

/// Resolve gen mtimes, time-travel pins/freezes to those moments, and
/// print the delta between them. Renders nothing when:
/// - either symlink can't be stat'd (mtime missing — odd FS),
/// - the user's flake isn't git-versioned (no ground truth),
/// - the policy state is identical between the two timestamps
///   (no drift to highlight).
fn print_policy_delta_header(from_path: &str, to_path: &str) {
    let (Some(from_at), Some(to_at)) = (gen_mtime(from_path), gen_mtime(to_path)) else {
        return;
    };
    let Some(flake_dir) = policy_dir() else {
        return;
    };

    let from_pins = crate::nix::pins::read_at_time(&flake_dir, from_at);
    let to_pins = crate::nix::pins::read_at_time(&flake_dir, to_at);
    let from_freezes = crate::nix::freezes::read_at_time(&flake_dir, from_at);
    let to_freezes = crate::nix::freezes::read_at_time(&flake_dir, to_at);

    let Some(delta) = compute_pin_freeze_delta(
        &from_pins,
        &to_pins,
        &from_freezes,
        &to_freezes,
    ) else {
        return;
    };

    println!(
        "  {} {}\n",
        "Policy delta:".bold(),
        format_pin_freeze_delta(&delta).dimmed()
    );
}

/// Stat a generation symlink and return its mtime. `None` on any
/// failure mode — the caller falls back to skipping the header.
fn gen_mtime(path: &str) -> Option<SystemTime> {
    let secs = std::fs::symlink_metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(UNIX_EPOCH + Duration::from_secs(secs))
}

/// Resolve the flake dir for the policy delta header, gated on it
/// being a git work tree (same shape as the `cheni history` /
/// `cheni rollback` helpers — kept duplicated to keep each command's
/// gating independent).
fn policy_dir() -> Option<std::path::PathBuf> {
    let dir = crate::nix::config::detect().ok()?.flake_dir;
    if !crate::nix::git::is_repo(&dir) {
        return None;
    }
    Some(dir)
}

