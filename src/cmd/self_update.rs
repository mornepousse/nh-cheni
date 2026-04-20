//! `cheni self-update` command.
//!
//! Updates the cheni flake input, verifies the new release's signature,
//! then rebuilds the system so the new version is available in the PATH.

use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use colored::Colorize;

use crate::nix::config;
use crate::release;

/// Run `cheni self-update`.
pub async fn run(allow_unsigned: bool) -> Result<()> {
    let nix_config = config::detect()?;
    let config_path = nix_config
        .flake_dir
        .to_str()
        .context("Config path is not valid UTF-8")?;

    println!("{}\n", "=== cheni self-update ===".bold());

    println!("{} Updating cheni flake input...", "[1/3]".dimmed());
    run_flake_update(&nix_config.flake_dir)?;

    println!("\n{} Verifying release signature...", "[2/3]".dimmed());
    enforce_signature(&nix_config.flake_dir, allow_unsigned).await?;

    println!("\n{} Rebuilding system to install new cheni...\n", "[3/3]".dimmed());
    run_nh_switch(config_path)?;

    println!("\n{} cheni updated successfully!", "✓".green());
    println!(
        "  Open a new shell, then run '{}' to see the new build.",
        "cheni --version".bold()
    );

    Ok(())
}

/// Step 1 — refresh the `cheni` flake input via `nix flake update`.
fn run_flake_update(flake_dir: &Path) -> Result<()> {
    let status = Command::new("nix")
        .args(["flake", "update", "cheni"])
        .current_dir(flake_dir)
        .status()
        .map_err(|e| crate::nix::tools::tool_error("nix", e))?;
    if !status.success() {
        anyhow::bail!(
            "nix flake update cheni failed.\n\
             Is 'cheni' declared as a flake input in your flake.nix?"
        );
    }
    Ok(())
}

/// Step 2 — enforce that the new release is signed. Returns `Ok(())`
/// when the signature verifies or the user explicitly opted out with
/// `--allow-unsigned`. Any other outcome bails so we never reach the
/// `nh os switch` with an unverified release.
async fn enforce_signature(flake_dir: &Path, allow_unsigned: bool) -> Result<()> {
    let tag = match read_cheni_tag(flake_dir) {
        Ok(t) => t,
        Err(e) if allow_unsigned => {
            println!(
                "  {} Cannot determine release tag ({}). Proceeding with --allow-unsigned.",
                "⚠".yellow(),
                e
            );
            return Ok(());
        }
        Err(e) => {
            return Err(e).context(
                "unable to determine cheni tag from flake.lock. \
                 Pin the input to a tag (e.g. `gitlab:harrael/cheni/v0.2.0`) \
                 or re-run with --allow-unsigned.",
            );
        }
    };

    match release::verify_release(&tag).await {
        Ok(report) => {
            println!(
                "  {} Signature verified for {} ({})",
                "✓".green(),
                report.tag.bold(),
                report.trusted_comment.dimmed()
            );
            Ok(())
        }
        Err(e) if allow_unsigned => {
            println!(
                "  {} Signature check skipped for {} (--allow-unsigned): {}",
                "⚠".yellow(),
                tag.bold(),
                e
            );
            Ok(())
        }
        Err(e) => Err(anyhow!(
            "Signature verification failed for {}:\n  {}\n\n\
             Refusing to rebuild with an unverified release.\n\
             Re-run with --allow-unsigned only if you have confirmed the release \
             out-of-band (e.g. with `minisign -Vm` against a known-good tarball).",
            tag,
            e
        )),
    }
}

/// Step 3 — rebuild the system so the new cheni lands in `$PATH`.
///
/// Uses the merged-pipe streamer so store-path noise is stripped live,
/// and feeds the raw buffer to the diagnose pattern library on failure
/// so the user gets actionable hints instead of a wall of text.
fn run_nh_switch(config_path: &str) -> Result<()> {
    let out = crate::output::stream::run_streaming(
        "nh",
        &["os", "switch", config_path],
        None,
    )?;
    if !out.status.success() {
        crate::cmd::diagnose::print_hints_for(&out.raw_buffer);
        anyhow::bail!("System rebuild failed. Run 'cheni build' to see the error.");
    }
    Ok(())
}

/// Parse the user's `flake.lock` and return the `ref` (tag) pinned for
/// the `cheni` input.
fn read_cheni_tag(flake_dir: &Path) -> Result<String> {
    let lock_path = flake_dir.join("flake.lock");
    let content = std::fs::read_to_string(&lock_path)
        .with_context(|| format!("reading {}", lock_path.display()))?;
    extract_cheni_tag(&content)
}

/// Pure core of `read_cheni_tag` — takes the flake.lock contents and
/// extracts the `ref` under the `cheni` node. Pulled out of the IO so
/// it can be tested against hand-written fixtures.
pub(crate) fn extract_cheni_tag(flake_lock: &str) -> Result<String> {
    let lock: serde_json::Value =
        serde_json::from_str(flake_lock).context("parsing flake.lock as JSON")?;
    let node = lock
        .get("nodes")
        .and_then(|n| n.get("cheni"))
        .ok_or_else(|| anyhow!("no 'cheni' input found in flake.lock"))?;
    let tag = node
        .get("original")
        .and_then(|o| o.get("ref"))
        .or_else(|| node.get("locked").and_then(|l| l.get("ref")))
        .and_then(|r| r.as_str())
        .ok_or_else(|| {
            anyhow!(
                "cheni input has no 'ref' — pin to a tag with \
                 `gitlab:harrael/cheni/vX.Y.Z` to enable signature verification"
            )
        })?;
    Ok(tag.to_string())
}

#[cfg(test)]
#[path = "tests/self_update.rs"]
mod tests;
