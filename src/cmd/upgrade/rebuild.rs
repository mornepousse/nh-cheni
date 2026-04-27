//! Step 3 of `cheni upgrade`: invoke nh to apply the new generation,
//! plus the freeze-refresh helper that runs alongside step 1.

use std::path::Path;

use anyhow::Result;
use tracing::debug;

/// Step 1b: refresh any freezes that carry a `--major N` constraint.
///
/// Walks `package-freezes.json`, queries the new nixpkgs rev for each
/// constrained package, and either bumps the freeze (same major, new
/// patch/minor available) or holds it (upstream moved past the major).
/// Non-fatal: a prefetch / eval failure just reports "Unknown" for
/// the entry and leaves the upgrade moving forward.
pub(super) fn refresh_constrained_freezes_step(flake_dir: &Path) {
    match crate::cmd::freeze::refresh_constrained_freezes(flake_dir) {
        Ok(outcomes) if !outcomes.is_empty() => {
            crate::cmd::freeze::print_refresh_summary(&outcomes);
        }
        Ok(_) => {}
        Err(e) => {
            debug!("Freeze refresh skipped: {}", e);
        }
    }
}

/// Step 3: invoke `nh os switch` (live activation) or `nh os boot`
/// (stage for next boot) depending on `boot`.
///
/// Uses the merged-pipe streamer so `/nix/store/<hash>-...` noise is
/// stripped from the output live. On failure, the raw (non-prettified)
/// buffer is fed to the diagnose pattern library so the user gets an
/// actionable hint along with the raw error — including the
/// `Pre-switch check` recovery path that suggests `--boot` when a
/// critical-component change tripped the activation guard.
pub(super) fn rebuild_system(config_path: &str, boot: bool) -> Result<()> {
    let action = if boot { "boot" } else { "switch" };
    let out = crate::output::stream::run_streaming(
        "nh",
        &["os", action, config_path],
        None,
    )?;
    if !out.status.success() {
        crate::cmd::diagnose::print_hints_for(&out.raw_buffer);
        anyhow::bail!("System rebuild failed. Fix the issue and run 'cheni build' again.");
    }
    Ok(())
}
