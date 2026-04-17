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
    /// Last modified timestamp (unix seconds)
    #[allow(dead_code)]
    pub last_modified: u64,
    /// Short git revision hash (from flake.lock)
    pub rev: String,
    /// How many days since last update
    #[allow(dead_code)]
    pub days_old: u64,
    /// Installed version (from the nix store, if found)
    pub installed_version: Option<String>,
    /// Repository type ("github" or "gitlab")
    pub repo_type: Option<String>,
    /// Repository owner
    pub repo_owner: Option<String>,
    /// Repository name
    pub repo_name: Option<String>,
    /// Whether the remote has newer commits
    pub has_update: Option<bool>,
    /// Human-readable age of the latest remote commit (e.g. "today", "3 days ago")
    pub remote_age: Option<String>,
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

/// Mapping from flake input names to store package names.
/// Used to find the installed version of a flake input package.
/// Input name → store name prefix (matched case-insensitively).
const INPUT_STORE_MAPPINGS: &[(&str, &str)] = &[
    ("claude-code", "claude-code"),
    ("zen-browser", "zen-browser"),
    ("affinity-nix", "Affinity-Designer"),
    ("kesp-controller", "kesp-controller"),
];

/// Store paths to scan for installed versions.
const STORE_PATHS: &[&str] = &[
    "/run/current-system/sw",
    "/etc/profiles/per-user/mae",  // TODO: detect username dynamically
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

        // Try to find the installed version from the store
        let installed_version = find_store_version(input_name);

        // Extract repo info from the "original" field
        let original = node.get("original");
        let repo_type = original.and_then(|o| o.get("type")).and_then(|v| v.as_str()).map(|s| s.to_string());
        let repo_owner = original.and_then(|o| o.get("owner")).and_then(|v| v.as_str()).map(|s| s.to_string());
        let repo_name = original.and_then(|o| o.get("repo")).and_then(|v| v.as_str()).map(|s| s.to_string());

        debug!(
            "Flake input: {} v{} ({}d old, rev {}, {}/{})",
            input_name,
            installed_version.as_deref().unwrap_or("?"),
            days_old,
            rev,
            repo_owner.as_deref().unwrap_or("?"),
            repo_name.as_deref().unwrap_or("?"),
        );

        result.push(FlakeInput {
            name: input_name.clone(),
            last_modified,
            rev,
            days_old,
            installed_version,
            repo_type,
            repo_owner,
            repo_name,
            has_update: None,
            remote_age: None,
        });
    }

    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}

/// Try to find the installed version of a flake input from the nix store.
///
/// Uses the INPUT_STORE_MAPPINGS table to find the store package name,
/// then looks it up in the store output.
fn find_store_version(input_name: &str) -> Option<String> {
    // Find the store name for this input
    let store_prefix = INPUT_STORE_MAPPINGS.iter()
        .find(|(input, _)| *input == input_name)
        .map(|(_, store)| *store)?;

    // Scan all store paths (system + user profile)
    for store_path in STORE_PATHS {
        if let Some(version) = scan_store_for_version(store_path, store_prefix) {
            return Some(version);
        }
    }

    None
}

/// Scan a single store path for a package version.
fn scan_store_for_version(store_path: &str, store_prefix: &str) -> Option<String> {
    let output = std::process::Command::new("nix-store")
        .args(["-qR", store_path])
        .output()
        .ok()?;

    let stdout = String::from_utf8(output.stdout).ok()?;

    for line in stdout.lines() {
        // Extract store name: /nix/store/<hash>-<name>-<version>
        let store_name = line.strip_prefix("/nix/store/")?;
        if store_name.len() < 34 {
            continue;
        }
        let name_version = &store_name[33..];

        // Check if it matches our prefix (case-insensitive)
        if name_version.to_lowercase().starts_with(&store_prefix.to_lowercase()) {
            // Extract version: everything after "prefix-"
            let after_prefix = &name_version[store_prefix.len()..];
            if let Some(version) = after_prefix.strip_prefix('-') {
                // Skip sub-outputs
                if version.contains("-man") || version.contains("-doc")
                    || version.ends_with(".desktop") || version.ends_with(".svg") {
                    continue;
                }
                return Some(version.to_string());
            }
        }
    }

    None
}

/// Info about the latest remote commit.
struct RemoteCommitInfo {
    rev: String,
    date: Option<String>,
}

/// Check flake inputs for available updates by comparing the locked rev
/// with the latest commit on the default branch via GitHub/GitLab API.
pub fn check_flake_updates(inputs: &mut [FlakeInput]) {
    let client = reqwest::blocking::Client::builder()
        .user_agent("nixup/0.1")
        .timeout(std::time::Duration::from_secs(5))
        .build();

    let client = match client {
        Ok(c) => c,
        Err(_) => return,
    };

    for input in inputs.iter_mut() {
        let (repo_type, owner, repo) = match (&input.repo_type, &input.repo_owner, &input.repo_name) {
            (Some(t), Some(o), Some(r)) => (t.as_str(), o.as_str(), r.as_str()),
            _ => continue,
        };

        let info = match repo_type {
            "github" => {
                let url = format!(
                    "https://api.github.com/repos/{}/{}/commits?per_page=1",
                    owner, repo
                );
                fetch_commit_info(&client, &url, |json| {
                    let commit = json.as_array()?.first()?;
                    let rev = commit.get("sha")?.as_str().map(|s| s[..12].to_string())?;
                    // GitHub date is in commit.commit.committer.date
                    let date = commit.get("commit")
                        .and_then(|c| c.get("committer"))
                        .and_then(|c| c.get("date"))
                        .and_then(|d| d.as_str())
                        .map(|s| s[..10].to_string()); // "2026-04-15T..."  → "2026-04-15"
                    Some(RemoteCommitInfo { rev, date })
                })
            }
            "gitlab" => {
                let url = format!(
                    "https://gitlab.com/api/v4/projects/{}%2F{}/repository/commits?per_page=1",
                    owner, repo
                );
                fetch_commit_info(&client, &url, |json| {
                    let commit = json.as_array()?.first()?;
                    let rev = commit.get("id")?.as_str().map(|s| s[..12].to_string())?;
                    let date = commit.get("committed_date")
                        .and_then(|d| d.as_str())
                        .map(|s| s[..10].to_string());
                    Some(RemoteCommitInfo { rev, date })
                })
            }
            _ => None,
        };

        if let Some(info) = info {
            let is_outdated = info.rev != input.rev[..info.rev.len().min(input.rev.len())];
            input.has_update = Some(is_outdated);
            if is_outdated {
                input.remote_age = info.date.map(|d| format!("latest: {}", d));
                debug!("Flake input {} has update: {} → {}", input.name, input.rev, info.rev);
            }
        }
    }
}

/// Fetch commit info from a Git hosting API.
fn fetch_commit_info<F>(client: &reqwest::blocking::Client, url: &str, extract: F) -> Option<RemoteCommitInfo>
where
    F: Fn(&serde_json::Value) -> Option<RemoteCommitInfo>,
{
    let response = client.get(url).send().ok()?;
    if !response.status().is_success() {
        debug!("API request failed: {} → {}", url, response.status());
        return None;
    }
    let json: serde_json::Value = response.json().ok()?;
    extract(&json)
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
