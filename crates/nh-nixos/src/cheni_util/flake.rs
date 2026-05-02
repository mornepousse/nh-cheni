//! `flake.lock` introspection — read the locked rev (and optionally
//! narHash) of any input.
//!
//! Three near-identical readers existed across `freezes.rs`,
//! `check.rs`, and `self_update.rs`. Lifted here so a future change
//! to the `flake.lock` schema (rare but possible — see the 2022
//! schema bump) is fixed in one place.

use std::{fs, path::Path};

use color_eyre::eyre::{Context, Result, eyre};
use serde_json::Value;

/// Locked input from `flake.lock`. `nar_hash` is `None` for callers
/// that only need the rev (e.g. `self-update` reading the cheni
/// input's HEAD). `Some(...)` when the caller needs to drive
/// `builtins.fetchTree` purely.
#[derive(Debug, Clone)]
pub struct LockedInput {
    pub rev: String,
    pub nar_hash: Option<String>,
}

/// Read the locked rev (and narHash) of `input_name` from
/// `<flake-dir>/flake.lock`.
///
/// Looks up `nodes.root.inputs.<input_name>` to find the node alias
/// (handles user-side renames where the input is declared under one
/// name but the node is stored under another), then reads
/// `nodes.<node>.locked.{rev, narHash}`.
///
/// # Errors
///
/// Returns an error if `flake.lock` is missing/unparseable, doesn't
/// declare an input under `input_name`, or the locked block has no
/// `rev` field. Missing `narHash` is non-fatal: returns
/// `LockedInput { nar_hash: None, .. }`.
pub fn read_input_locked(
    flake_dir: &Path,
    input_name: &str,
) -> Result<LockedInput> {
    let lock_path = flake_dir.join("flake.lock");
    let content = fs::read_to_string(&lock_path).with_context(|| {
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
        .map(str::to_string);
    Ok(LockedInput { rev, nar_hash })
}

#[cfg(test)]
#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn minimal_lock_with_rev_and_narhash() {
        let dir = TempDir::new().unwrap();
        fs::write(
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
        assert_eq!(l.nar_hash.as_deref(), Some("sha256-AAAA="));
    }

    #[test]
    fn handles_aliased_input_name() {
        let dir = TempDir::new().unwrap();
        fs::write(
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
        let l = read_input_locked(dir.path(), "cheni").unwrap();
        assert_eq!(l.rev, "0123456789abcdef0123456789abcdef01234567");
        assert!(l.nar_hash.is_none());
    }

    #[test]
    fn errors_when_input_absent() {
        let dir = TempDir::new().unwrap();
        fs::write(
            dir.path().join("flake.lock"),
            br#"{ "nodes": { "root": { "inputs": {} } } }"#,
        )
        .unwrap();
        assert!(read_input_locked(dir.path(), "nixpkgs").is_err());
    }

    #[test]
    fn errors_when_lock_missing() {
        let dir = TempDir::new().unwrap();
        assert!(read_input_locked(dir.path(), "nixpkgs").is_err());
    }
}
