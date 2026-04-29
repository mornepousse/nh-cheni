//! `cheni upgrade` command.
//!
//! Full system upgrade: update all flake inputs, rebuild, clean
//! obsolete pins, and optionally garbage-collect old generations.
//!
//! The command is split across submodules by phase:
//!
//! - [`flake_update`] — step 1: refresh flake inputs, watch for the
//!   dirty-lock / order-mismatch traps that turn a "small" upgrade
//!   into something else.
//! - [`preview`] — step 2: dry-run evaluation, change classification,
//!   confirmation prompt. Also hosts `print_pending_changes`, the
//!   read-only entry point that `cheni check` reuses.
//! - [`rebuild`] — step 3: invoke nh, plus the freeze refresh that
//!   runs alongside step 1.
//! - [`cleanup`] — steps 4 + 5: prune obsolete pins, optional GC.
//! - [`summary`] — shared state (`UpgradeStats`, `UpgradeContext`)
//!   and the closing "✓ Upgrade complete …" line.

use std::time::Instant;

use anyhow::{Context, Result};
use colored::Colorize;

use crate::nix::{config, pins};

mod cleanup;
mod flake_update;
mod preview;
mod rebuild;
mod summary;

pub(crate) use flake_update::warn_if_dirty_lock;
pub(crate) use preview::print_pending_changes;

/// Options for `cheni upgrade`.
pub struct UpgradeOptions {
    /// Run garbage collection after the rebuild (default: off).
    /// This DELETES old generations — you won't be able to rollback!
    pub gc: bool,
    /// Skip cleanup of obsolete pins.
    pub no_clean_pins: bool,
    /// Skip the preview + confirmation step.
    pub yes: bool,
    /// Refresh ONLY `nixpkgs-latest` (the per-package overlay source)
    /// instead of every flake input. Equivalent to the old `cheni
    /// update` semantics. Bails with a friendly hint when no pin is
    /// active — the flag has nothing to do then.
    pub pins_only: bool,
    /// Stage the new generation for next boot instead of live-switching.
    /// `nh os boot` skips the activation pre-checks that refuse the
    /// live switch on critical-component changes (dbus → dbus-broker,
    /// init swap, …). When this flag is off, cheni still detects those
    /// changes during preview and offers to flip the mode interactively.
    pub boot: bool,
    /// Suppress cheni's own narration (policy block, step headers).
    /// Underlying tools (nh, nix) still produce their own output.
    /// The final outcome line (✓/✗ + elapsed) is always printed.
    pub brief: bool,
}

/// Run `cheni upgrade`.
///
/// Full system upgrade, broken into numbered steps:
/// 1. Update flake inputs + refresh major-constrained freezes
/// 2. Preview changes
/// 3. Rebuild the system
/// 4. Clean obsolete pins
/// 5. (optional, with --gc) Garbage-collect old generations
pub fn run(mut opts: UpgradeOptions) -> Result<()> {
    let started = Instant::now();
    let nix_config = config::detect()?;
    let config_path = nix_config.flake_dir.to_str()
        .context("Config path is not valid UTF-8")?;
    let total_steps = if opts.gc { 5 } else { 4 };

    if !opts.brief {
        println!("{}\n", "=== cheni upgrade ===".bold());
    }

    // --pins-only with no pins is meaningless — bail before touching
    // anything so the user gets a clear "use plain upgrade" pointer
    // instead of a no-op rebuild.
    if opts.pins_only && pins::read(&nix_config.flake_dir)?.is_empty() {
        println!(
            "  {} No pins to apply. Use '{}' for a full system upgrade.",
            "✓".green(),
            "cheni upgrade".bold()
        );
        return Ok(());
    }

    // Pre-step state check: an uncommitted flake.lock means a previous
    // run (or a manual `nix flake update`) bumped inputs that haven't
    // been built yet. The rebuild *will* apply those bumps even when
    // the current flag scope (--pins-only) implies a smaller refresh.
    // Surfacing the state up front prevents the "why did my kernel
    // update?" surprise.
    if !opts.brief {
        warn_if_dirty_lock(&nix_config.flake_dir);
    }

    let step1_title = if opts.pins_only {
        "Updating nixpkgs-latest"
    } else {
        "Updating flake inputs"
    };
    if !opts.brief {
        print_step(1, total_steps, step1_title);
    }
    let step1_start = Instant::now();
    let context = flake_update::update_flake_inputs(&nix_config.flake_dir, opts.pins_only)?;
    if !opts.brief {
        println!(
            "  {} done in {}s",
            "✓".green().dimmed(),
            step1_start.elapsed().as_secs().to_string().dimmed()
        );
    }

    // Anti-downgrade guard for the targeted refresh: if nixpkgs has
    // since caught up with (or moved past) nixpkgs-latest, applying
    // pins would either be a no-op or actively roll packages back.
    if opts.pins_only && !flake_update::verify_nixpkgs_order(&nix_config.flake_dir) {
        return Ok(());
    }

    rebuild::refresh_constrained_freezes_step(&nix_config.flake_dir);
    if !opts.brief {
        print_separator();
        print_step(2, total_steps, "Previewing changes");
    }
    let stats = match preview::preview_and_confirm(
        config_path,
        &nix_config.hostname,
        &mut opts,
        &context,
    )? {
        Some(s) => s,
        None => return Ok(()),
    };
    if !opts.brief {
        print_separator();
    }

    let step3_title = if opts.boot {
        "Staging system for next boot"
    } else {
        "Rebuilding system"
    };
    if !opts.brief {
        print_step(3, total_steps, step3_title);
    }
    let step3_start = Instant::now();
    let rebuild_result = rebuild::rebuild_system(config_path, opts.boot);
    if !opts.brief {
        print_separator();
    }
    // In brief mode: surface the rebuild error with the one-liner verdict.
    if let Err(ref e) = rebuild_result {
        let elapsed = crate::util::format_elapsed(started.elapsed());
        if opts.brief {
            println!(
                "{} Upgrade failed: {} ({})",
                "✗".red().bold(),
                e,
                elapsed.dimmed(),
            );
        } else {
            println!(
                "{} Upgrade failed at step 3 (rebuild): {} ({}s)",
                "✗".red().bold(),
                e,
                step3_start.elapsed().as_secs()
            );
            println!(
                "  {} Steps 1-2 completed (flake.lock is updated, preview confirmed).",
                "→".dimmed()
            );
            println!(
                "  {} Fix the build error and run `{}` to re-attempt the rebuild only.",
                "→".dimmed(),
                "cheni build".bold()
            );
            println!(
                "  {} Or `{}` to retry the full flow.",
                "→".dimmed(),
                "cheni upgrade --yes".bold()
            );
        }
        return Err(rebuild_result.unwrap_err());
    }
    if !opts.brief {
        println!(
            "  {} rebuild succeeded in {}s",
            "✓".green().dimmed(),
            step3_start.elapsed().as_secs().to_string().dimmed()
        );
    }

    if !opts.brief {
        print_step(4, total_steps, "Checking obsolete pins");
    }
    let step4_start = Instant::now();
    cleanup::run_pin_cleanup_step(&nix_config.flake_dir, opts.no_clean_pins)?;
    if !opts.brief {
        println!(
            "  {} done in {}s",
            "✓".green().dimmed(),
            step4_start.elapsed().as_secs().to_string().dimmed()
        );
    }

    if opts.gc {
        if !opts.brief {
            print_separator();
            print_step(5, total_steps, "Collecting garbage (> 30 days)");
        }
        cleanup::run_gc_step(opts.yes)?;
    }

    if !opts.brief {
        print_separator();
        summary::print_final_summary(started.elapsed(), &stats, &context, opts.boot);
        if !opts.gc {
            println!(
                "{}",
                "  Old generations kept for rollback. Use --gc to reclaim disk space later.".dimmed()
            );
        }
    } else {
        // --brief: one-liner verdict only.
        let elapsed = crate::util::format_elapsed(started.elapsed());
        println!(
            "{} {} ({})",
            "✓".green().bold(),
            if opts.boot { "Upgrade staged for next boot" } else { "Upgrade complete" },
            elapsed.dimmed(),
        );
    }
    crate::nix::timeline::record(
        "upgrade",
        None,
        serde_json::json!({
            "outcome": "success",
            "duration_secs": started.elapsed().as_secs(),
        }),
    );
    Ok(())
}

/// Local thin alias to the shared `crate::output::print_step` so
/// the existing `print_step(...)` call sites stay short.
fn print_step(n: usize, total: usize, title: &str) {
    crate::output::print_step(n, total, title);
}

/// Local thin alias to the shared `crate::output::print_separator`.
fn print_separator() {
    crate::output::print_separator();
}

/// Wrapper around `util::confirm` that keeps upgrade's default-yes
/// semantic at the call site (the original local helper did the same
/// thing; this version delegates to the shared prompt).
pub(super) fn confirm(question: &str) -> Result<bool> {
    crate::util::confirm(question, true)
}
