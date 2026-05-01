//! `nh os check` — query each pin/freeze against current nixpkgs.
//!
//! For every pin and freeze, evaluates the `.version` attribute of the
//! package against:
//!   - the rev locked in `flake.lock`'s `nixpkgs` input (what your
//!     system would resolve to if no overlay touched it)
//!   - the rev locked in `nixpkgs-latest` (where pins point to)
//!
//! Then classifies:
//!   - **pin still useful** : nixpkgs-latest > nixpkgs (the pin is
//!     getting you a newer version than the base channel would)
//!   - **pin obsolete** : nixpkgs has caught up — you can drop the pin
//!   - **freeze still relevant** : nixpkgs has moved past your freeze
//!     (you're holding the package back on purpose)
//!   - **freeze obsolete** : nixpkgs is at-or-below your freeze rev
//!     (the freeze isn't holding anything back, you can drop it)
//!
//! Talks to `nix eval --raw` via a content-addressed `fetchTree`, so
//! no flake registry tweaks are needed and the eval is pure (no
//! `--impure` flag). The KDE 6 namespace fallback (`kdePackages.<name>`)
//! is tried automatically when the bare name doesn't resolve.

use std::{path::Path, process::Command};

use color_eyre::eyre::{Context, Result, eyre};
use serde_json::Value;
use tracing::debug;

use crate::{
  args::OsCheckArgs,
  freezes,
  pins,
  versioning::{VersionDiff, compare_versions, parse_version},
};

/// (rev, narHash) pair for a flake input. Both are required by
/// `fetchTree` to keep the eval pure and content-addressed.
#[derive(Debug, Clone)]
pub struct LockedInput {
  pub rev: String,
  pub nar_hash: String,
}

/// Read `<flake-dir>/flake.lock` and return the locked rev + narHash
/// for the input named `input_name`.
///
/// Looks up `nodes.root.inputs.<input_name>` to find the node alias
/// (handles cases where the user's flake uses a different name for
/// the same upstream), then reads `nodes.<node>.locked.{rev, narHash}`.
///
/// # Errors
///
/// Returns an error if `flake.lock` is missing/unparseable or doesn't
/// declare an input under `input_name`.
pub fn read_input_locked(
  flake_dir: &Path,
  input_name: &str,
) -> Result<LockedInput> {
  let lock_path = flake_dir.join("flake.lock");
  let content = std::fs::read_to_string(&lock_path).with_context(|| {
    format!(
      "reading {} — did you `nix flake update` at least once?",
      lock_path.display()
    )
  })?;
  let lock: Value = serde_json::from_str(&content)
    .with_context(|| format!("parsing {} as JSON", lock_path.display()))?;

  let root_inputs = lock
    .pointer("/nodes/root/inputs")
    .and_then(Value::as_object)
    .ok_or_else(|| {
      eyre!("{} has no /nodes/root/inputs object", lock_path.display())
    })?;
  let node_name = root_inputs
    .get(input_name)
    .and_then(|v| v.as_str())
    .unwrap_or(input_name);
  let locked = lock
    .pointer(&format!("/nodes/{node_name}/locked"))
    .ok_or_else(|| {
      eyre!(
        "Input '{input_name}' is not declared in {} (looked under \
         /nodes/{node_name}/locked).",
        lock_path.display()
      )
    })?;
  let rev = locked
    .get("rev")
    .and_then(Value::as_str)
    .ok_or_else(|| eyre!("input '{input_name}' has no locked rev"))?
    .to_string();
  let nar_hash = locked
    .get("narHash")
    .and_then(Value::as_str)
    .ok_or_else(|| {
      eyre!("input '{input_name}' has no locked narHash")
    })?
    .to_string();
  Ok(LockedInput { rev, nar_hash })
}

/// Query `pkgs.<pkg_name>.version` against a nixpkgs tree pinned at
/// `(rev, nar_hash)`. Tries the bare name first, then
/// `kdePackages.<pkg_name>` as a fallback (covers the KDE 6 migration
/// where many packages moved out of the top-level pkgs namespace).
///
/// Returns `None` on any failure (eval error, missing attribute, no
/// `.version` on the attr, validation rejection). The caller decides
/// whether absence is fatal; for `nh os check` it isn't.
#[must_use]
pub fn query_pkg_version(
  rev: &str,
  nar_hash: &str,
  pkg_name: &str,
) -> Option<String> {
  for attr in [pkg_name.to_string(), format!("kdePackages.{pkg_name}")] {
    if let Some(v) = query_one(rev, nar_hash, &attr) {
      return Some(v);
    }
  }
  None
}

fn query_one(rev: &str, nar_hash: &str, attr: &str) -> Option<String> {
  // Defence in depth: validate inputs before splicing into the Nix
  // expression. The pins/freezes modules already validate at write
  // time, but this function is reachable from `read_input_locked`
  // output (file content, possibly tampered) so we revalidate.
  if rev.is_empty()
    || rev.len() > 64
    || !rev.chars().all(|c| c.is_ascii_hexdigit())
  {
    return None;
  }
  if !(nar_hash.starts_with("sha256-") || nar_hash.starts_with("sha512-"))
    || nar_hash.len() > 200
    || nar_hash.chars().any(|c| c.is_control() || c == '"' || c == '\\')
  {
    return None;
  }
  if attr.is_empty()
    || attr.len() > 128
    || !attr.chars().all(|c| {
      c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '+'
    })
  {
    return None;
  }
  let system = target_system();

  let expr = format!(
    "let pkgs = import (builtins.fetchTree {{ \
type = \"github\"; owner = \"NixOS\"; repo = \"nixpkgs\"; \
rev = \"{rev}\"; narHash = \"{nar_hash}\"; \
}}) {{ system = \"{system}\"; config.allowUnfree = true; }}; \
in pkgs.{attr}.version"
  );

  let out = match Command::new("nix")
    .args(["eval", "--raw", "--expr", &expr])
    .output()
  {
    Ok(o) => o,
    Err(e) => {
      debug!("nix eval spawn failed: {e}");
      return None;
    },
  };
  if !out.status.success() {
    debug!(
      "nix eval failed for '{}' at rev {}: {}",
      attr,
      &rev[..rev.len().min(12)],
      String::from_utf8_lossy(&out.stderr).trim()
    );
    return None;
  }
  let v = String::from_utf8(out.stdout).ok()?.trim().to_string();
  if v.is_empty() { None } else { Some(v) }
}

fn target_system() -> &'static str {
  // The nh fork ships for these four pre-defined systems (matching
  // flake.nix). We resolve at compile time so the eval expression
  // stays pure and reproducible.
  if cfg!(all(target_arch = "x86_64", target_os = "linux")) {
    "x86_64-linux"
  } else if cfg!(all(target_arch = "aarch64", target_os = "linux")) {
    "aarch64-linux"
  } else if cfg!(all(target_arch = "x86_64", target_os = "macos")) {
    "x86_64-darwin"
  } else if cfg!(all(target_arch = "aarch64", target_os = "macos")) {
    "aarch64-darwin"
  } else {
    // Fallback — unlikely in practice since the workspace doesn't
    // ship for other systems, but Nix will simply fail to eval and
    // query_one returns None gracefully.
    "x86_64-linux"
  }
}

// ── Subcommand entry point ─────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PinStatus {
  StillUseful,    // nixpkgs-latest is newer than nixpkgs
  Obsolete,       // nixpkgs caught up to (or past) nixpkgs-latest
  Unresolvable,   // couldn't query at least one side
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FreezeStatus {
  StillRelevant,  // nixpkgs is past the frozen version
  Obsolete,       // nixpkgs is at or below the frozen version
  Unresolvable,
}

impl OsCheckArgs {
  /// Run `nh os check`.
  ///
  /// # Errors
  ///
  /// Returns an error when the flake-dir can't be resolved or when
  /// `nixpkgs` itself isn't declared in `flake.lock`. Per-package
  /// query failures are tolerated (reported as `Unresolvable`).
  pub fn run(self) -> Result<()> {
    let flake_dir =
      pins::resolve_flake_dir(self.flake_dir.as_deref())?;
    let pins_list = pins::read(&flake_dir)?;
    let freezes_map = freezes::read(&flake_dir)?;
    if pins_list.is_empty() && freezes_map.is_empty() {
      println!("No pins or freezes to check.");
      println!(
        "Add some with `nh os pin <pkg>` or `nh os freeze <pkg>`."
      );
      return Ok(());
    }

    println!("Reading flake.lock for input revs…");
    let nixpkgs = read_input_locked(&flake_dir, "nixpkgs")?;
    println!(
      "  nixpkgs:        {}",
      &nixpkgs.rev[..nixpkgs.rev.len().min(12)]
    );

    let nixpkgs_latest = match read_input_locked(&flake_dir, "nixpkgs-latest")
    {
      Ok(l) => {
        println!(
          "  nixpkgs-latest: {}",
          &l.rev[..l.rev.len().min(12)]
        );
        Some(l)
      },
      Err(e) => {
        println!(
          "  nixpkgs-latest: not declared in your flake ({e}).\n  \
           Pin status will be reported as Unresolvable. Freezes \
           still work."
        );
        None
      },
    };

    if !pins_list.is_empty() {
      println!("\nPins ({}):", pins_list.len());
      let mut still_useful = 0usize;
      let mut obsolete = 0usize;
      let mut unresolvable = 0usize;
      for name in &pins_list {
        let in_nixpkgs =
          query_pkg_version(&nixpkgs.rev, &nixpkgs.nar_hash, name);
        let in_latest = nixpkgs_latest
          .as_ref()
          .and_then(|l| query_pkg_version(&l.rev, &l.nar_hash, name));
        let status = classify_pin(in_nixpkgs.as_deref(), in_latest.as_deref());
        match status {
          PinStatus::StillUseful => still_useful += 1,
          PinStatus::Obsolete => obsolete += 1,
          PinStatus::Unresolvable => unresolvable += 1,
        }
        let mark = match status {
          PinStatus::StillUseful => "useful",
          PinStatus::Obsolete => "OBSOLETE — nixpkgs caught up",
          PinStatus::Unresolvable => "unresolvable",
        };
        println!(
          "  - {name}  nixpkgs={}  nixpkgs-latest={}  → {mark}",
          in_nixpkgs.as_deref().unwrap_or("?"),
          in_latest.as_deref().unwrap_or("?"),
        );
      }
      println!(
        "  ({still_useful} still useful, {obsolete} obsolete, \
         {unresolvable} unresolvable)"
      );
    }

    if !freezes_map.is_empty() {
      println!("\nFreezes ({}):", freezes_map.len());
      let mut still_relevant = 0usize;
      let mut obsolete = 0usize;
      let mut unresolvable = 0usize;
      for (name, entry) in &freezes_map {
        let in_nixpkgs =
          query_pkg_version(&nixpkgs.rev, &nixpkgs.nar_hash, name);
        let in_freeze = query_pkg_version(
          &entry.rev,
          &entry.nar_hash,
          name,
        );
        let status =
          classify_freeze(in_nixpkgs.as_deref(), in_freeze.as_deref());
        match status {
          FreezeStatus::StillRelevant => still_relevant += 1,
          FreezeStatus::Obsolete => obsolete += 1,
          FreezeStatus::Unresolvable => unresolvable += 1,
        }
        let mark = match status {
          FreezeStatus::StillRelevant => "still relevant",
          FreezeStatus::Obsolete => {
            "OBSOLETE — nixpkgs at-or-below freeze"
          },
          FreezeStatus::Unresolvable => "unresolvable",
        };
        println!(
          "  - {name}  nixpkgs={}  frozen-at={}  → {mark}",
          in_nixpkgs.as_deref().unwrap_or("?"),
          in_freeze.as_deref().unwrap_or("?"),
        );
      }
      println!(
        "  ({still_relevant} still relevant, {obsolete} obsolete, \
         {unresolvable} unresolvable)"
      );
    }
    Ok(())
  }
}

/// A pin routes <pkg> through nixpkgs-latest. It's still useful when
/// nixpkgs-latest has a STRICTLY NEWER version than nixpkgs. Equal
/// versions or nixpkgs >= latest = obsolete.
fn classify_pin(
  in_nixpkgs: Option<&str>,
  in_latest: Option<&str>,
) -> PinStatus {
  let (Some(np), Some(nl)) = (in_nixpkgs, in_latest) else {
    return PinStatus::Unresolvable;
  };
  let np = parse_version(np);
  let nl = parse_version(nl);
  match compare_versions(&np, &nl) {
    VersionDiff::Minor | VersionDiff::Major => PinStatus::StillUseful,
    VersionDiff::Equal | VersionDiff::Newer => PinStatus::Obsolete,
  }
}

/// A freeze holds <pkg> at a specific rev. It's still relevant when
/// nixpkgs has STRICTLY MOVED PAST the freeze (i.e. unfreezing would
/// pull a different version). Equal or nixpkgs <= freeze = obsolete.
fn classify_freeze(
  in_nixpkgs: Option<&str>,
  in_freeze: Option<&str>,
) -> FreezeStatus {
  let (Some(np), Some(fz)) = (in_nixpkgs, in_freeze) else {
    return FreezeStatus::Unresolvable;
  };
  let np = parse_version(np);
  let fz = parse_version(fz);
  match compare_versions(&fz, &np) {
    VersionDiff::Minor | VersionDiff::Major => FreezeStatus::StillRelevant,
    VersionDiff::Equal | VersionDiff::Newer => FreezeStatus::Obsolete,
  }
}

#[cfg(test)]
#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
mod tests {
  use super::*;
  use tempfile::TempDir;

  #[test]
  fn read_input_locked_minimal_lock() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
      dir.path().join("flake.lock"),
      br#"{
        "nodes": {
          "root": { "inputs": { "nixpkgs": "nixpkgs" } },
          "nixpkgs": {
            "locked": {
              "rev": "deadbeefcafebabe1234567890abcdef12345678",
              "narHash": "sha256-AAAA="
            }
          }
        }
      }"#,
    )
    .unwrap();
    let l = read_input_locked(dir.path(), "nixpkgs").unwrap();
    assert!(l.rev.starts_with("deadbeef"));
    assert_eq!(l.nar_hash, "sha256-AAAA=");
  }

  #[test]
  fn read_input_locked_handles_aliased_input_name() {
    let dir = TempDir::new().unwrap();
    // root.inputs.nixpkgs-latest → "alias-node", body lives at alias-node.
    std::fs::write(
      dir.path().join("flake.lock"),
      br#"{
        "nodes": {
          "root": { "inputs": { "nixpkgs-latest": "alias-node" } },
          "alias-node": {
            "locked": {
              "rev": "0123456789abcdef0123456789abcdef01234567",
              "narHash": "sha256-BBBB="
            }
          }
        }
      }"#,
    )
    .unwrap();
    let l = read_input_locked(dir.path(), "nixpkgs-latest").unwrap();
    assert_eq!(l.rev, "0123456789abcdef0123456789abcdef01234567");
  }

  #[test]
  fn read_input_locked_errors_when_input_absent() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
      dir.path().join("flake.lock"),
      br#"{ "nodes": { "root": { "inputs": {} } } }"#,
    )
    .unwrap();
    assert!(read_input_locked(dir.path(), "nixpkgs-latest").is_err());
  }

  #[test]
  fn classify_pin_still_useful_when_latest_is_newer() {
    assert_eq!(
      classify_pin(Some("128.0"), Some("130.0")),
      PinStatus::StillUseful
    );
  }

  #[test]
  fn classify_pin_obsolete_when_equal() {
    assert_eq!(
      classify_pin(Some("128.0"), Some("128.0")),
      PinStatus::Obsolete
    );
  }

  #[test]
  fn classify_pin_obsolete_when_nixpkgs_ahead() {
    assert_eq!(
      classify_pin(Some("131.0"), Some("130.0")),
      PinStatus::Obsolete
    );
  }

  #[test]
  fn classify_pin_unresolvable_when_either_is_missing() {
    assert_eq!(
      classify_pin(None, Some("130.0")),
      PinStatus::Unresolvable
    );
    assert_eq!(
      classify_pin(Some("128.0"), None),
      PinStatus::Unresolvable
    );
    assert_eq!(classify_pin(None, None), PinStatus::Unresolvable);
  }

  #[test]
  fn classify_freeze_still_relevant_when_nixpkgs_ahead() {
    // Frozen at 10.0.1, nixpkgs at 10.1.0 → still holding it back.
    assert_eq!(
      classify_freeze(Some("10.1.0"), Some("10.0.1")),
      FreezeStatus::StillRelevant
    );
  }

  #[test]
  fn classify_freeze_obsolete_when_equal() {
    assert_eq!(
      classify_freeze(Some("10.0.1"), Some("10.0.1")),
      FreezeStatus::Obsolete
    );
  }

  #[test]
  fn classify_freeze_obsolete_when_nixpkgs_below() {
    // nixpkgs 9.9 < freeze 10.0 → freeze isn't holding anything back
    // *yet*, so it's effectively obsolete (nothing to constrain).
    assert_eq!(
      classify_freeze(Some("9.9"), Some("10.0")),
      FreezeStatus::Obsolete
    );
  }

  #[test]
  fn target_system_returns_known_value() {
    let s = target_system();
    assert!([
      "x86_64-linux",
      "aarch64-linux",
      "x86_64-darwin",
      "aarch64-darwin"
    ]
    .contains(&s));
  }
}
