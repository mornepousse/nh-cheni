//! `nh os self-update` — pull the latest cheni-fork commit into the
//! user's flake, optionally chain into a rebuild.
//!
//! Mono-user fork = no signing layer (cf. wrapper-era's
//! `minisign-verify`). The threat model that justified signing in the
//! wrapper era doesn't apply here: Mae publishes to her own GitLab
//! repo and trusts her own credentials. The `nix flake update` is
//! the entire self-update mechanism — this module just wraps it with
//! useful reporting (before/after rev, diff URL hint) and an
//! optional `--switch` that chains into the rebuild.
//!
//! Records a `self-update` event in the timeline so `nh os events`
//! shows when the fork moved.

use std::{path::Path, process::Command};

use color_eyre::eyre::{Context, Result, bail};
use serde_json::Value;
use tracing::debug;

use crate::{
  args::OsSelfUpdateArgs,
  pins,
  timeline,
};

impl OsSelfUpdateArgs {
  /// Run `nh os self-update`.
  ///
  /// # Errors
  ///
  /// Returns an error when the flake-dir can't be resolved, the
  /// `cheni` (or `--input`) input isn't declared in `flake.lock`,
  /// or `nix flake update` fails.
  pub fn run(self) -> Result<()> {
    let flake_dir =
      pins::resolve_flake_dir(self.flake_dir.as_deref())?;
    let input = self.input.as_deref().unwrap_or("cheni");

    let before = read_input_rev(&flake_dir, input).with_context(|| {
      format!(
        "Cannot read the locked rev of '{input}' in {}/flake.lock. \
         Is '{input}' declared as a flake input?",
        flake_dir.display()
      )
    })?;
    let before_short =
      before.chars().take(12).collect::<String>();
    println!(
      "Updating flake input '{input}' in {}…",
      flake_dir.display()
    );
    println!("  current: {before_short}");

    let status = Command::new("nix")
      .args(["flake", "update", input])
      .current_dir(&flake_dir)
      .status()
      .with_context(|| {
        format!("spawning `nix flake update {input}`")
      })?;
    if !status.success() {
      bail!(
        "`nix flake update {input}` exited with {}",
        status.code().map_or("signal".to_string(), |c| c.to_string())
      );
    }

    let after = read_input_rev(&flake_dir, input)?;
    let after_short = after.chars().take(12).collect::<String>();

    if before == after {
      println!("  → already at latest commit ({before_short}).");
      return Ok(());
    }

    println!("  new:     {after_short}");
    println!(
      "  diff:    https://gitlab.com/harrael/cheni/-/compare/{before_short}...{after_short}"
    );

    timeline::record(
      "self-update",
      None,
      serde_json::json!({
        "input": input,
        "before": before_short,
        "after": after_short,
      }),
    );

    if self.switch {
      println!("\nChaining into `nh os switch`…");
      let nh = std::env::current_exe()
        .context("locating the running nh binary")?;
      let status = Command::new(&nh)
        .args(["os", "switch"])
        .arg(&flake_dir)
        .status()
        .context("spawning `nh os switch`")?;
      if !status.success() {
        bail!(
          "`nh os switch` exited with {}",
          status.code().map_or("signal".to_string(), |c| c.to_string())
        );
      }
    } else {
      println!(
        "\nRun `nh os switch {}` (or rerun with --switch) to apply.",
        flake_dir.display()
      );
    }

    Ok(())
  }
}

/// Read the locked rev of `input_name` from `<flake-dir>/flake.lock`.
///
/// Returns the full hex rev. The lock structure mirrors what
/// `check::read_input_locked` parses, but we only need the rev here
/// (no narHash) so a smaller helper avoids pulling check.rs as a dep.
pub fn read_input_rev(
  flake_dir: &Path,
  input_name: &str,
) -> Result<String> {
  let lock_path = flake_dir.join("flake.lock");
  let content = std::fs::read_to_string(&lock_path).with_context(|| {
    format!("reading {}", lock_path.display())
  })?;
  let lock: Value = serde_json::from_str(&content)
    .with_context(|| format!("parsing {} as JSON", lock_path.display()))?;
  let root_inputs = lock
    .pointer("/nodes/root/inputs")
    .and_then(Value::as_object)
    .ok_or_else(|| {
      color_eyre::eyre::eyre!(
        "{} has no /nodes/root/inputs object",
        lock_path.display()
      )
    })?;
  let node_name = root_inputs
    .get(input_name)
    .and_then(|v| v.as_str())
    .unwrap_or(input_name);
  let rev = lock
    .pointer(&format!("/nodes/{node_name}/locked/rev"))
    .and_then(Value::as_str)
    .ok_or_else(|| {
      color_eyre::eyre::eyre!(
        "no /nodes/{node_name}/locked/rev in flake.lock"
      )
    })?
    .to_string();
  debug!("self-update: '{input_name}' locked at {rev}");
  Ok(rev)
}

#[cfg(test)]
#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
mod tests {
  use super::*;
  use tempfile::TempDir;

  #[test]
  fn read_input_rev_works_on_minimal_lock() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
      dir.path().join("flake.lock"),
      br#"{
        "nodes": {
          "root": { "inputs": { "cheni": "cheni" } },
          "cheni": {
            "locked": {
              "rev": "deadbeefcafebabe1234567890abcdef12345678"
            }
          }
        }
      }"#,
    )
    .unwrap();
    let rev = read_input_rev(dir.path(), "cheni").unwrap();
    assert_eq!(rev, "deadbeefcafebabe1234567890abcdef12345678");
  }

  #[test]
  fn read_input_rev_handles_aliased_input_name() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
      dir.path().join("flake.lock"),
      br#"{
        "nodes": {
          "root": { "inputs": { "cheni": "cheni-alias" } },
          "cheni-alias": {
            "locked": { "rev": "0123456789abcdef0123456789abcdef01234567" }
          }
        }
      }"#,
    )
    .unwrap();
    let rev = read_input_rev(dir.path(), "cheni").unwrap();
    assert_eq!(rev, "0123456789abcdef0123456789abcdef01234567");
  }

  #[test]
  fn read_input_rev_errors_when_input_absent() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
      dir.path().join("flake.lock"),
      br#"{ "nodes": { "root": { "inputs": {} } } }"#,
    )
    .unwrap();
    assert!(read_input_rev(dir.path(), "cheni").is_err());
  }

  #[test]
  fn read_input_rev_errors_when_lock_missing() {
    let dir = TempDir::new().unwrap();
    assert!(read_input_rev(dir.path(), "cheni").is_err());
  }
}
