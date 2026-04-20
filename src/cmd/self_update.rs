//! `cheni self-update` command.
//!
//! Updates the cheni flake input, verifies the new release's signature,
//! then rebuilds the system so the new version is available in the PATH.

use std::path::Path;
use std::process::Command;
use std::time::Instant;

use anyhow::{anyhow, Context, Result};
use colored::Colorize;

use crate::nix::config;
use crate::release;

/// Run `cheni self-update`.
pub async fn run(allow_unsigned: bool) -> Result<()> {
    let started = Instant::now();
    let nix_config = config::detect()?;
    let config_path = nix_config
        .flake_dir
        .to_str()
        .context("Config path is not valid UTF-8")?;

    println!("{}\n", "=== cheni self-update ===".bold());

    print_step(1, 3, "Updating cheni flake input");
    let before = read_cheni_timestamp(&nix_config.flake_dir);
    run_flake_update(&nix_config.flake_dir)?;
    let after = read_cheni_timestamp(&nix_config.flake_dir);
    let cheni_moved = before != after;
    print_separator();

    print_step(2, 3, "Verifying release signature");
    enforce_signature(&nix_config.flake_dir, allow_unsigned).await?;
    print_separator();

    if !cheni_moved {
        println!(
            "  {} {}",
            "⚠".yellow().bold(),
            "cheni flake input did not move — you are already on the latest signed release. \
             Rebuilding would be a no-op."
                .yellow()
        );
        println!(
            "{} {} in {} — already up to date (no rebuild).",
            "✓".green().bold(),
            "Self-update complete".bold(),
            format_elapsed(started.elapsed()).dimmed(),
        );
        return Ok(());
    }

    print_step(3, 3, "Rebuilding system to install new cheni");
    println!();
    run_nh_switch(config_path)?;
    print_separator();

    println!(
        "{} {} in {} — cheni rebuilt from a fresh flake input.",
        "✓".green().bold(),
        "Self-update complete".bold(),
        format_elapsed(started.elapsed()).dimmed()
    );
    println!(
        "  Open a new shell, then run '{}' to see the new build.",
        "cheni --version".bold()
    );

    Ok(())
}

/// Render `[N/total] Title` — matches the shape used by `cheni upgrade`.
fn print_step(n: usize, total: usize, title: &str) {
    println!("{} {}", format!("[{}/{}]", n, total).dimmed(), title.bold());
}

/// Horizontal rule between steps — matches `cheni upgrade`.
fn print_separator() {
    println!("{}", "───────────────────────────────────────────".dimmed());
}

/// Format `Duration` as `MmSs` or `Ss`. Matches the helper in `upgrade`
/// / `update` — kept local so each command stays self-contained.
fn format_elapsed(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    }
}

/// Read `cheni`'s `lastModified` timestamp from flake.lock. Returns 0
/// when the lock can't be read or the input isn't declared — callers
/// use this purely as a "did the input move?" signal, so the missing
/// case registers as "changed" on the second read, which keeps the
/// command from silently silently-skipping a real rebuild.
fn read_cheni_timestamp(flake_dir: &Path) -> u64 {
    let lock_path = flake_dir.join("flake.lock");
    let content = match std::fs::read_to_string(&lock_path) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let lock: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return 0,
    };
    get_input_timestamp(&lock, "cheni").unwrap_or(0)
}

/// Extract the lastModified timestamp for a flake input from flake.lock.
/// Mirrors the helper in `cmd::update` — duplicated on purpose to keep
/// each command self-contained (see `feedback_propre.md`).
fn get_input_timestamp(lock: &serde_json::Value, input_name: &str) -> Option<u64> {
    let root_input = lock
        .get("nodes")?
        .get("root")?
        .get("inputs")?
        .get(input_name)?;
    let node_name = match root_input.as_str() {
        Some(s) => s,
        None => input_name,
    };
    lock.get("nodes")?
        .get(node_name)?
        .get("locked")?
        .get("lastModified")?
        .as_u64()
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
