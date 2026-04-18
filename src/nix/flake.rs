//! Flake input parsing.
//!
//! Reads flake.lock to identify non-nixpkgs inputs and their current
//! revision timestamps. Used to show flake input status in `cheni check`.

use std::path::Path;

use anyhow::{Context, Result};
use tracing::debug;

/// A flake input with its metadata from flake.lock.
#[derive(Debug, Clone)]
pub struct FlakeInput {
    /// Input name (e.g. "zen-browser", "claude-code").
    pub name: String,
    /// Last modified timestamp (unix seconds).
    /// Stored for debug logging and potential future use by consumers.
    #[allow(dead_code)]
    pub last_modified: u64,
    /// Short git revision hash (from flake.lock).
    pub rev: String,
    /// How many days since the last update.
    /// Stored for debug logging and potential future use by consumers.
    #[allow(dead_code)]
    pub days_old: u64,
    /// Installed version (from the nix store, if found).
    pub installed_version: Option<String>,
    /// Repository type ("github" or "gitlab").
    pub repo_type: Option<String>,
    /// Repository owner.
    pub repo_owner: Option<String>,
    /// Repository name.
    pub repo_name: Option<String>,
    /// Whether the remote has newer commits.
    pub has_update: Option<bool>,
    /// Human-readable age of the latest remote commit (e.g. "today", "3 days ago").
    pub remote_age: Option<String>,
}

/// Inputs whose updates are handled globally via `cheni upgrade` rather
/// than listed alongside user-facing package flakes. Kept tight: only
/// what every NixOS flake user has. Optional toolchain flakes
/// (rust-overlay, nixpkgs-esp-dev, fenix, nix-darwin, ...) stay visible
/// because they're not universal — a user with `nixpkgs-esp-dev` very
/// likely wants to see when it has a new commit.
const INFRASTRUCTURE_INPUTS: &[&str] = &[
    "nixpkgs",
    "nixpkgs-latest",
    "home-manager",
    // cheni's own flake self-reference — always excluded so 'cheni check'
    // doesn't suggest updating cheni alongside user packages. Use
    // `cheni self-update` for that.
    "cheni",
];

/// Build the list of store paths to scan for installed versions.
///
/// Always includes the system profile. The user profile path depends on
/// the current user — $USER (set by most login shells) or $LOGNAME, then
/// $HOME as a last resort. Without a known username we fall back to the
/// glob-less `~` and rely on the system profile alone.
fn store_paths() -> Vec<String> {
    let mut paths = vec!["/run/current-system/sw".to_string()];

    if let Some(user) = std::env::var("USER")
        .ok()
        .or_else(|| std::env::var("LOGNAME").ok())
    {
        paths.push(format!("/etc/profiles/per-user/{}", user));
    } else if let Some(home) = dirs::home_dir() {
        if let Some(name) = home.file_name().and_then(|n| n.to_str()) {
            paths.push(format!("/etc/profiles/per-user/{}", name));
        }
    }

    paths
}

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
/// We scan `/run/current-system/sw` and the current user's profile for
/// a store entry whose name starts with the input name. This is a
/// heuristic — it works for flake inputs that publish a package with
/// the same name as the input (the common case), but not for flakes
/// that rename their output (e.g. an `affinity-nix` flake producing
/// `Affinity-Designer-*`). When no match is found we return None and
/// the UI shows "?" for the version — the user gets their update
/// notification either way, it's only the "current" column that suffers.
fn find_store_version(input_name: &str) -> Option<String> {
    for store_path in store_paths() {
        if let Some(version) = scan_store_for_version(&store_path, input_name) {
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

/// Metadata about the latest remote commit.
struct RemoteCommitInfo {
    rev: String,
    date: Option<String>,
}

/// Check flake inputs for available updates by comparing the locked
/// revision with the latest commit on the default branch via GitHub/GitLab API.
///
/// Each input is queried in its own thread (concurrently) so the wall-clock
/// time is roughly that of the slowest single API call.
pub fn check_flake_updates(inputs: &mut [FlakeInput]) {
    // Use the same configurable timeout as the Repology client — on a
    // slow link the GitHub/GitLab API commits call can be just as slow
    // as Repology, and a 5s hard cap frequently tripped in practice.
    let client = reqwest::blocking::Client::builder()
        .user_agent("cheni/0.1")
        .timeout(crate::api::net::http_timeout())
        .build();

    let client = match client {
        Ok(c) => c,
        Err(_) => return,
    };

    // Spawn one thread per input — APIs are independent, so we can fan out.
    std::thread::scope(|scope| {
        let handles: Vec<_> = inputs
            .iter()
            .map(|input| {
                let client = client.clone();
                let repo_type = input.repo_type.clone();
                let repo_owner = input.repo_owner.clone();
                let repo_name = input.repo_name.clone();
                let local_rev = input.rev.clone();

                scope.spawn(move || -> (Option<bool>, Option<String>) {
                    let (repo_type, owner, repo) = match (repo_type, repo_owner, repo_name) {
                        (Some(t), Some(o), Some(r)) => (t, o, r),
                        _ => return (None, None),
                    };

                    let info = match repo_type.as_str() {
                        "github" => {
                            let url = format!(
                                "https://api.github.com/repos/{}/{}/commits?per_page=1",
                                owner, repo
                            );
                            fetch_commit_info(&client, &url, |json| {
                                let commit = json.as_array()?.first()?;
                                let rev = commit.get("sha")?.as_str().map(short_hash)?;
                                let date = commit.get("commit")
                                    .and_then(|c| c.get("committer"))
                                    .and_then(|c| c.get("date"))
                                    .and_then(|d| d.as_str())
                                    .map(short_date);
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
                                let rev = commit.get("id")?.as_str().map(short_hash)?;
                                let date = commit.get("committed_date")
                                    .and_then(|d| d.as_str())
                                    .map(short_date);
                                Some(RemoteCommitInfo { rev, date })
                            })
                        }
                        _ => None,
                    };

                    match info {
                        Some(info) => {
                            // Compare the two truncated revs by char count, not
                            // by byte. Git hashes are hex so this is equivalent
                            // in practice but the explicit form avoids a
                            // mid-codepoint slice if the API ever returns
                            // something unexpected.
                            let prefix_len = info.rev.chars().count().min(local_rev.chars().count());
                            let local_prefix: String = local_rev.chars().take(prefix_len).collect();
                            let is_outdated = info.rev != local_prefix;
                            let age = if is_outdated {
                                info.date.map(|d| format!("latest: {}", d))
                            } else {
                                None
                            };
                            (Some(is_outdated), age)
                        }
                        None => (None, None),
                    }
                })
            })
            .collect();

        for (input, handle) in inputs.iter_mut().zip(handles) {
            if let Ok((has_update, remote_age)) = handle.join() {
                input.has_update = has_update;
                input.remote_age = remote_age;
                if has_update == Some(true) {
                    debug!("Flake input {} has update", input.name);
                }
            }
        }
    });
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

/// Take up to the first 12 characters of a Git hash — char-based rather
/// than byte-based so a malformed response can't trigger a panic.
fn short_hash(s: &str) -> String {
    s.chars().take(12).collect()
}

/// Keep only the `YYYY-MM-DD` prefix of an ISO-8601 timestamp. Char-based
/// for the same reason as `short_hash`.
fn short_date(s: &str) -> String {
    s.chars().take(10).collect()
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
        // Core infrastructure (always excluded)
        assert!(INFRASTRUCTURE_INPUTS.contains(&"nixpkgs"));
        assert!(INFRASTRUCTURE_INPUTS.contains(&"nixpkgs-latest"));
        assert!(INFRASTRUCTURE_INPUTS.contains(&"home-manager"));
        assert!(INFRASTRUCTURE_INPUTS.contains(&"cheni"));

        // User-facing package flakes (never excluded)
        assert!(!INFRASTRUCTURE_INPUTS.contains(&"zen-browser"));
        assert!(!INFRASTRUCTURE_INPUTS.contains(&"claude-code"));

        // Optional toolchain flakes should NOT be excluded — not every user
        // has them and those who do want update visibility.
        assert!(!INFRASTRUCTURE_INPUTS.contains(&"rust-overlay"));
        assert!(!INFRASTRUCTURE_INPUTS.contains(&"nixpkgs-esp-dev"));
        assert!(!INFRASTRUCTURE_INPUTS.contains(&"fenix"));
    }

    #[test]
    fn short_hash_handles_short_input() {
        // The API may return a hash shorter than 12 chars (rare, but a
        // byte-slice would panic). Char-based truncation returns as many
        // chars as exist without panicking.
        assert_eq!(short_hash("abc"), "abc");
        assert_eq!(short_hash(""), "");
    }

    #[test]
    fn short_hash_truncates_to_twelve() {
        assert_eq!(
            short_hash("abcdef1234567890"),
            "abcdef123456"
        );
    }

    #[test]
    fn short_hash_survives_non_ascii() {
        // Not expected in real Git output, but we parse external JSON
        // so we can't assume it. Must not panic at a multi-byte boundary.
        assert_eq!(short_hash("é🦀x"), "é🦀x");
    }

    #[test]
    fn short_date_handles_short_input() {
        assert_eq!(short_date("2026"), "2026");
        assert_eq!(short_date(""), "");
    }
}
