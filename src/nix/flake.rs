//! Flake input parsing.
//!
//! Reads flake.lock to identify non-nixpkgs inputs and their current
//! revision timestamps. Used to show flake input status in `nixup check`.

use std::path::Path;

use anyhow::{Context, Result};
use tracing::debug;

/// A flake input with its metadata from flake.lock.
#[derive(Debug, Clone)]
pub struct FlakeInput {
    /// Input name (e.g. "zen-browser", "claude-code")
    pub name: String,
    /// Last modified timestamp (unix seconds) — used for age calculation
    #[allow(dead_code)]
    pub last_modified: u64,
    /// Short git revision hash
    pub rev: String,
    /// How many days since last update
    pub days_old: u64,
}

/// Inputs that are infrastructure, not user-facing packages.
/// These are excluded from the flake input list.
const INFRASTRUCTURE_INPUTS: &[&str] = &[
    "nixpkgs",
    "nixpkgs-latest",
    "home-manager",
    "rust-overlay",
    "nixpkgs-esp-dev",
    "nixup",
];

/// Read all non-infrastructure flake inputs from flake.lock.
///
/// Returns inputs like zen-browser, claude-code, kesp-controller, etc.
/// Excludes nixpkgs, home-manager, and other toolchain inputs.
pub fn read_flake_inputs(flake_dir: &Path) -> Result<Vec<FlakeInput>> {
    let lock_path = flake_dir.join("flake.lock");
    let content = std::fs::read_to_string(&lock_path)
        .context("Failed to read flake.lock")?;

    let lock: serde_json::Value = serde_json::from_str(&content)
        .context("Failed to parse flake.lock")?;

    let nodes = lock.get("nodes")
        .and_then(|n| n.as_object())
        .context("No 'nodes' in flake.lock")?;

    // The root node lists the direct inputs
    let root_inputs = nodes.get("root")
        .and_then(|r| r.get("inputs"))
        .and_then(|i| i.as_object());

    let root_input_names: Vec<String> = match root_inputs {
        Some(inputs) => inputs.keys().cloned().collect(),
        None => {
            debug!("No root inputs found in flake.lock");
            return Ok(Vec::new());
        }
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let mut result = Vec::new();

    for input_name in &root_input_names {
        // Skip infrastructure inputs
        if INFRASTRUCTURE_INPUTS.contains(&input_name.as_str()) {
            continue;
        }

        // Get the locked info for this input
        // Some inputs use indirection (the value in root.inputs might be
        // a string pointing to another node)
        let node_name = root_inputs
            .and_then(|i| i.get(input_name))
            .and_then(|v| v.as_str())
            .unwrap_or(input_name);

        let node = match nodes.get(node_name) {
            Some(n) => n,
            None => {
                debug!("Input '{}' not found in nodes", input_name);
                continue;
            }
        };

        let locked = match node.get("locked") {
            Some(l) => l,
            None => {
                debug!("Input '{}' has no locked info", input_name);
                continue;
            }
        };

        let last_modified = locked.get("lastModified")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let rev = locked.get("rev")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .chars()
            .take(12)
            .collect::<String>();

        let days_old = now.saturating_sub(last_modified) / 86400;

        debug!("Flake input: {} ({}d old, rev {})", input_name, days_old, rev);

        result.push(FlakeInput {
            name: input_name.clone(),
            last_modified,
            rev,
            days_old,
        });
    }

    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}

/// Check if a name corresponds to a flake input (not a nixpkgs package).
pub fn is_flake_input(flake_dir: &Path, name: &str) -> bool {
    match read_flake_inputs(flake_dir) {
        Ok(inputs) => inputs.iter().any(|i| i.name == name),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infrastructure_inputs_excluded() {
        assert!(INFRASTRUCTURE_INPUTS.contains(&"nixpkgs"));
        assert!(INFRASTRUCTURE_INPUTS.contains(&"home-manager"));
        assert!(!INFRASTRUCTURE_INPUTS.contains(&"zen-browser"));
    }
}
