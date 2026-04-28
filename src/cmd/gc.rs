//! `cheni gc` — disk-space orchestrator.
//!
//! Wraps generation pruning + `nix-collect-garbage` with safety
//! guards and a structured preview. See
//! `docs/superpowers/specs/2026-04-28-cheni-gc-design.md`.

#![allow(dead_code)]
// Task 7 will remove this once run() is wired into main dispatch.

use anyhow::Result;

/// Refuse to gc if the user would keep fewer than this — without `--force`.
pub(crate) const MIN_SAFETY_FLOOR: usize = 3;

/// Default number of recent generations to keep.
pub(crate) const DEFAULT_KEEP: usize = 10;

/// CLI options for `cheni gc`.
#[derive(Debug)]
pub struct GcOptions {
    /// Number of recent generations to keep.
    pub keep: usize,
    /// Audit + preview, do not delete anything.
    pub dry_run: bool,
    /// Skip the confirmation prompt.
    pub yes: bool,
    /// Brief output (one-line summary).
    pub brief: bool,
    /// Override the safety floor (allow keep < MIN_SAFETY_FLOOR).
    pub force: bool,
}

impl Default for GcOptions {
    fn default() -> Self {
        Self {
            keep: DEFAULT_KEEP,
            dry_run: false,
            yes: false,
            brief: false,
            force: false,
        }
    }
}

use colored::Colorize;

use crate::cmd::history::{plan_prune_keep_n, read_generations, PrunePlan};

/// Result of the audit phase: the prune plan + the dead-path count.
pub(crate) struct GcAudit {
    pub plan: PrunePlan,
    /// Lower bound on store paths that would be reclaimed.
    /// Real reclaim is higher because deleting generations releases
    /// additional closures the dry-run can't see.
    pub dead_paths_lower_bound: usize,
}

/// Run the audit phase: read generations, build the prune plan,
/// query the current dead-path count.
pub(crate) fn audit_plan(keep: usize) -> Result<GcAudit> {
    let generations = read_generations()?;
    let plan = plan_prune_keep_n(&generations, keep);
    let preview = crate::nix::gc::preview(&[])?;
    Ok(GcAudit {
        plan,
        dead_paths_lower_bound: preview.paths,
    })
}

/// Render the audit + preview block (used by run() before the apply phase).
pub(crate) fn print_audit(audit: &GcAudit, total_generations: usize) {
    println!("{}", "Audit:".bold());
    let kept = audit.plan.kept_count();
    let deleted = audit.plan.deleted_ids.len();

    if kept > 0 {
        let first = audit.plan.kept_ids.first().copied().unwrap_or(0);
        let last = audit.plan.kept_ids.last().copied().unwrap_or(0);
        println!(
            "  {} generation(s), kept: {} most recent (gen {}..{})",
            total_generations.to_string().bold(),
            kept.to_string().green(),
            first,
            last,
        );
    } else {
        println!(
            "  {} generation(s), keeping NONE",
            total_generations.to_string().bold(),
        );
    }

    if deleted > 0 {
        let first = audit.plan.deleted_ids.first().copied().unwrap_or(0);
        let last = audit.plan.deleted_ids.last().copied().unwrap_or(0);
        println!(
            "  {} generation(s) to remove (gen {}..{})",
            deleted.to_string().yellow(),
            first,
            last,
        );
    }

    println!();
    println!("{}", "Preview:".bold());
    println!(
        "  Currently dead: {} store path(s) {}",
        audit.dead_paths_lower_bound.to_string().bold(),
        "(lower bound — generations not yet released)".dimmed(),
    );
}

/// Refuse the gc plan if it would leave fewer than `MIN_SAFETY_FLOOR`
/// generations. `force` overrides the floor but never zero (keeping
/// zero generations would brick rollback entirely).
pub(crate) fn check_safety_guard(kept_count: usize, force: bool) -> Result<()> {
    if kept_count == 0 {
        anyhow::bail!(
            "Refusing to keep 0 generations — that would leave you unable \
             to rollback. Increase --keep."
        );
    }
    if kept_count < MIN_SAFETY_FLOOR && !force {
        anyhow::bail!(
            "Would keep only {} generation(s) — below the safety floor of {}. \
             Use --force to override if you really mean it.",
            kept_count,
            MIN_SAFETY_FLOOR
        );
    }
    Ok(())
}

use std::process::Command;

/// Run the apply phase: delete the planned generations, then run gc.
/// Caller has already shown the audit and obtained confirmation.
pub(crate) fn apply_gc(plan: &PrunePlan) -> Result<()> {
    if plan.deleted_ids.is_empty() {
        // Edge case: keep >= total. Nothing to delete; still run gc
        // to clean any pre-existing dead paths.
    } else {
        println!("\n{}", "Pruning generations...".bold());
        crate::cmd::history::apply_deletion(&plan.deleted_ids)?;
        println!("  {} {} generations removed", "✓".green(), plan.deleted_ids.len());
    }

    println!("\n{}", "Running garbage collection...".bold());
    let status = Command::new("sudo")
        .args(["/run/current-system/sw/bin/nix-collect-garbage"])
        .status()
        .map_err(|e| crate::nix::tools::tool_error("sudo", e))?;
    if !status.success() {
        anyhow::bail!(
            "nix-collect-garbage failed. Disk may be full or a roots scan failed — \
             try `nix-store --gc --print-roots` to inspect what's pinning paths."
        );
    }
    println!("  {} reclaim complete", "✓".green());
    Ok(())
}

use dialoguer::{theme::ColorfulTheme, Confirm};

/// Orchestrate the gc flow: audit → safety → preview → confirm → apply.
pub fn run(opts: GcOptions) -> Result<()> {
    if !opts.brief {
        println!("{}\n", "=== cheni gc ===".bold());
    }

    let audit = audit_plan(opts.keep)?;
    check_safety_guard(audit.plan.kept_count(), opts.force)?;

    let total_generations = audit.plan.kept_count() + audit.plan.deleted_ids.len();

    if audit.plan.deleted_ids.is_empty() && audit.dead_paths_lower_bound == 0 {
        if opts.brief {
            println!("gc: nothing to do");
        } else {
            println!("{} Already minimal — nothing to remove.", "✓".green());
        }
        return Ok(());
    }

    if !opts.brief {
        print_audit(&audit, total_generations);
        println!();
    }

    if opts.dry_run {
        if opts.brief {
            println!(
                "gc dry-run: {} gens would be removed, {} dead path(s)",
                audit.plan.deleted_ids.len(),
                audit.dead_paths_lower_bound,
            );
        } else {
            println!("{}", "(dry-run — nothing applied)".dimmed());
        }
        return Ok(());
    }

    if !opts.yes {
        let theme = ColorfulTheme::default();
        let go = Confirm::with_theme(&theme)
            .with_prompt("Proceed with gc?")
            .default(false)
            .interact()
            .map_err(|e| anyhow::anyhow!("reading confirmation: {e}"))?;
        if !go {
            println!("{}", "  Cancelled — nothing removed.".yellow());
            return Ok(());
        }
    }

    let started = std::time::Instant::now();
    apply_gc(&audit.plan)?;
    let elapsed = started.elapsed();

    if opts.brief {
        println!(
            "gc: {} gens, {}+ paths reclaimed in {}s",
            audit.plan.deleted_ids.len(),
            audit.dead_paths_lower_bound,
            elapsed.as_secs(),
        );
    } else {
        println!("\n{} Done in {}s.", "✓".green().bold(), elapsed.as_secs());
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/gc.rs"]
mod tests;
