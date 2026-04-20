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
    /// Short git revision hash (from flake.lock).
    pub rev: String,
    /// How many days since the last update. Read by `cheni doctor` to
    /// flag stale inputs.
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

/// Validate that a username is safe to splice into a filesystem path.
///
/// Accepts the POSIX-compatible character set (ASCII alphanumerics,
/// `_`, `-`) with a length cap. Rejects anything that would let
/// `$USER=../foo` escape the `/etc/profiles/per-user/` prefix or
/// otherwise point at something we shouldn't be reading.
fn sanitize_username(raw: &str) -> Option<String> {
    if raw.is_empty() || raw.len() > 32 {
        return None;
    }
    if raw
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        Some(raw.to_string())
    } else {
        None
    }
}

/// Build the list of store paths to scan for installed versions.
///
/// Always includes the system profile. The user profile path depends on
/// the current user — $USER (set by most login shells) or $LOGNAME, then
/// $HOME as a last resort. Without a known, valid username we fall back
/// to the system profile alone.
fn store_paths() -> Vec<String> {
    let mut paths = vec!["/run/current-system/sw".to_string()];

    let username = std::env::var("USER")
        .ok()
        .or_else(|| std::env::var("LOGNAME").ok())
        .or_else(|| {
            dirs::home_dir()
                .as_ref()
                .and_then(|h| h.file_name())
                .and_then(|n| n.to_str())
                .map(String::from)
        })
        .and_then(|raw| sanitize_username(&raw));

    if let Some(user) = username {
        paths.push(format!("/etc/profiles/per-user/{}", user));
    }

    paths
}

/// Read all non-infrastructure flake inputs from flake.lock.
///
/// Returns inputs like zen-browser, claude-code, kesp-controller, etc.
/// Excludes nixpkgs, home-manager, and other toolchain inputs.
pub fn read_flake_inputs(flake_dir: &Path) -> Result<Vec<FlakeInput>> {
    let lock_path = flake_dir.join("flake.lock");
    let content = std::fs::read_to_string(&lock_path).context("Failed to read flake.lock")?;
    let lock: serde_json::Value =
        serde_json::from_str(&content).context("Failed to parse flake.lock")?;

    let nodes = lock
        .get("nodes")
        .and_then(|n| n.as_object())
        .context("No 'nodes' in flake.lock")?;
    let Some(root_inputs) = nodes
        .get("root")
        .and_then(|r| r.get("inputs"))
        .and_then(|i| i.as_object())
    else {
        debug!("No root inputs found in flake.lock");
        return Ok(Vec::new());
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let mut result = Vec::new();
    for input_name in root_inputs.keys() {
        if INFRASTRUCTURE_INPUTS.contains(&input_name.as_str()) {
            continue;
        }
        if let Some(input) = read_one_input(input_name, root_inputs, nodes, now) {
            result.push(input);
        }
    }
    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}

/// Build a `FlakeInput` for a single root input.
///
/// Returns None when the lock entry is missing or malformed (logged at
/// debug level — these aren't errors, just non-trivial inputs we don't
/// have anything useful to report on, e.g. inlined `path:` inputs).
///
/// `root_inputs` is the `nodes.root.inputs` map; the value at
/// `root_inputs[name]` may be a string pointing to another node (the
/// flake.lock indirection format), so we resolve through it before
/// looking the node up in `nodes`.
fn read_one_input(
    input_name: &str,
    root_inputs: &serde_json::Map<String, serde_json::Value>,
    nodes: &serde_json::Map<String, serde_json::Value>,
    now: u64,
) -> Option<FlakeInput> {
    let node_name = root_inputs
        .get(input_name)
        .and_then(|v| v.as_str())
        .unwrap_or(input_name);

    let node = nodes.get(node_name).or_else(|| {
        debug!("Input '{}' not found in nodes", input_name);
        None
    })?;
    let locked = node.get("locked").or_else(|| {
        debug!("Input '{}' has no locked info", input_name);
        None
    })?;

    let last_modified = locked.get("lastModified").and_then(|v| v.as_u64()).unwrap_or(0);
    let rev: String = locked
        .get("rev")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .chars()
        .take(12)
        .collect();
    let days_old = now.saturating_sub(last_modified) / 86400;
    let installed_version = find_store_version(input_name);

    let original = node.get("original");
    let repo_type = original.and_then(|o| o.get("type")).and_then(|v| v.as_str()).map(String::from);
    let repo_owner = original.and_then(|o| o.get("owner")).and_then(|v| v.as_str()).map(String::from);
    let repo_name = original.and_then(|o| o.get("repo")).and_then(|v| v.as_str()).map(String::from);

    debug!(
        "Flake input: {} v{} ({}d old, rev {}, {}/{})",
        input_name,
        installed_version.as_deref().unwrap_or("?"),
        days_old,
        rev,
        repo_owner.as_deref().unwrap_or("?"),
        repo_name.as_deref().unwrap_or("?"),
    );

    Some(FlakeInput {
        name: input_name.to_string(),
        rev,
        days_old,
        installed_version,
        repo_type,
        repo_owner,
        repo_name,
        has_update: None,
        remote_age: None,
    })
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
    // Soft probe: we return `None` instead of bubbling errors up because
    // callers treat "no version found" as a neutral signal. Log via
    // `tool_error` formatting so a missing `nix-store` still surfaces
    // a helpful hint in debug logs.
    let output = match std::process::Command::new("nix-store")
        .args(["-qR", store_path])
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            tracing::debug!(
                "scan_store_for_version: {}",
                crate::nix::tools::tool_error("nix-store", e),
            );
            return None;
        }
    };

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
    let Ok(client) = reqwest::blocking::Client::builder()
        .user_agent("cheni/0.1")
        .timeout(crate::http::http_timeout())
        .build()
    else {
        return;
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
                scope.spawn(move || check_one_input(&client, repo_type, repo_owner, repo_name, &local_rev))
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

/// Single-input update check. Returns (has_update, remote_age):
/// - `(None, None)` when we can't query (missing repo info, network error).
/// - `(Some(false), None)` when the input is up to date.
/// - `(Some(true), Some("latest: YYYY-MM-DD"))` when the remote is ahead.
fn check_one_input(
    client: &reqwest::blocking::Client,
    repo_type: Option<String>,
    repo_owner: Option<String>,
    repo_name: Option<String>,
    local_rev: &str,
) -> (Option<bool>, Option<String>) {
    let (Some(repo_type), Some(owner), Some(repo)) = (repo_type, repo_owner, repo_name) else {
        return (None, None);
    };
    let Some(info) = fetch_remote_info(client, &repo_type, &owner, &repo) else {
        return (None, None);
    };
    let is_outdated = is_revision_outdated(&info.rev, local_rev);
    let age = is_outdated.then(|| info.date.map(|d| format!("latest: {}", d))).flatten();
    (Some(is_outdated), age)
}

/// Dispatch to the right host API and parse the latest commit metadata.
fn fetch_remote_info(
    client: &reqwest::blocking::Client,
    repo_type: &str,
    owner: &str,
    repo: &str,
) -> Option<RemoteCommitInfo> {
    match repo_type {
        "github" => {
            let url = format!(
                "https://api.github.com/repos/{}/{}/commits?per_page=1",
                owner, repo
            );
            fetch_commit_info(client, &url, |json| {
                let commit = json.as_array()?.first()?;
                let rev = commit.get("sha")?.as_str().map(short_hash)?;
                let date = commit
                    .get("commit")
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
            fetch_commit_info(client, &url, |json| {
                let commit = json.as_array()?.first()?;
                let rev = commit.get("id")?.as_str().map(short_hash)?;
                let date = commit
                    .get("committed_date")
                    .and_then(|d| d.as_str())
                    .map(short_date);
                Some(RemoteCommitInfo { rev, date })
            })
        }
        // No API client for other repo types (sourcehut, git+https:, tarball,
        // path, ...). Log it so -v users can tell the difference between
        // "the probe ran and got nothing" and "we skipped it entirely".
        other => {
            debug!("No remote-update probe for repo_type={:?} ({}/{})", other, owner, repo);
            None
        }
    }
}

/// Compare two truncated git revs. Char-based slicing so a malformed
/// API response (anything not pure-hex) can't trigger a mid-codepoint
/// panic — git hashes are hex in practice, so this is equivalent.
fn is_revision_outdated(remote_rev: &str, local_rev: &str) -> bool {
    let prefix_len = remote_rev.chars().count().min(local_rev.chars().count());
    let local_prefix: String = local_rev.chars().take(prefix_len).collect();
    remote_rev != local_prefix
}

/// Fetch commit info from a Git hosting API.
///
/// Honors a `429 Too Many Requests` with a single retry, waiting for
/// the server-supplied `Retry-After` when present and falling back to
/// `RATE_LIMIT_RETRY_SECS` otherwise. GitHub's anonymous quota (60
/// req/h) is the common offender; a brief back-off is usually enough
/// to land on the next hour boundary.
fn fetch_commit_info<F>(client: &reqwest::blocking::Client, url: &str, extract: F) -> Option<RemoteCommitInfo>
where
    F: Fn(&serde_json::Value) -> Option<RemoteCommitInfo>,
{
    let response = send_with_retry_on_429(client, url)?;
    if !response.status().is_success() {
        debug!("API request failed: {} → {}", url, response.status());
        return None;
    }
    if let Err(e) = crate::http::check_content_length(
        response.content_length(),
        crate::http::MAX_BODY_BYTES,
    ) {
        debug!("{}: {}", url, e);
        return None;
    }
    let body = response.bytes().ok()?;
    if let Err(e) = crate::http::verify_body_size(body.len(), crate::http::MAX_BODY_BYTES) {
        debug!("{}: {}", url, e);
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(&body).ok()?;
    extract(&json)
}

/// Send a GET request with a single retry on HTTP 429.
///
/// On the first response, if the status is 429 we read the
/// `Retry-After` header (capped/defaulted by
/// `crate::http::parse_retry_after`), sleep for that many
/// seconds, and issue exactly one more GET. Any other status is
/// returned as-is to the caller.
fn send_with_retry_on_429(
    client: &reqwest::blocking::Client,
    url: &str,
) -> Option<reqwest::blocking::Response> {
    let first = client.get(url).send().ok()?;
    if first.status() != reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Some(first);
    }

    let retry_after = first
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok());
    let wait = crate::http::parse_retry_after(retry_after);
    debug!("429 from {}, retrying in {}s", url, wait);
    std::thread::sleep(std::time::Duration::from_secs(wait));

    client.get(url).send().ok()
}

/// Take up to the first 12 characters of a Git hash — char-based rather
/// than byte-based so a malformed response can't trigger a panic.
pub(crate) fn short_hash(s: &str) -> String {
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

/// Read the full git rev of the `nixpkgs` input from `flake.lock`.
///
/// Used by `cheni freeze` to pin a package to the nixpkgs commit the
/// system is currently running from. Returns the full 40-char rev,
/// unlike `FlakeInput::rev` which is truncated to 12 for display.
pub fn read_nixpkgs_rev(flake_dir: &Path) -> Result<String> {
    let lock_path = flake_dir.join("flake.lock");
    let content = std::fs::read_to_string(&lock_path)
        .with_context(|| format!("Failed to read {}", lock_path.display()))?;
    let lock: serde_json::Value =
        serde_json::from_str(&content).context("Failed to parse flake.lock")?;
    extract_root_input_rev(&lock, "nixpkgs")
        .context("Could not find the 'nixpkgs' input in flake.lock")
}

/// Extract the full `rev` of a root-level input from a parsed flake.lock.
/// Resolves the root indirection (root.inputs[name] may be a string that
/// names the real node) before reading `locked.rev`.
fn extract_root_input_rev(lock: &serde_json::Value, input_name: &str) -> Option<String> {
    let nodes = lock.get("nodes")?.as_object()?;
    let root_inputs = nodes.get("root")?.get("inputs")?.as_object()?;
    let node_name = root_inputs
        .get(input_name)?
        .as_str()
        .unwrap_or(input_name);
    let locked = nodes.get(node_name)?.get("locked")?;
    Some(locked.get("rev")?.as_str()?.to_string())
}

/// Prefetch a github:NixOS/nixpkgs/<rev> tarball and return its
/// narHash (SRI form — `sha256-...`).
///
/// Shells out to `nix flake prefetch --json <url>`, which both downloads
/// the tarball into the store *and* returns the hash in a single call.
/// The returned narHash is stable regardless of whether flake metadata
/// re-computation kicks in on later evals.
pub fn prefetch_nixpkgs_rev(rev: &str) -> Result<String> {
    // Defence in depth: the caller already validates `rev` against
    // freezes::validate_entry, but this function is public and the rev
    // flows into a URL passed to `nix` via `Command::args`. A strict
    // hex check keeps the surface airtight even if a future caller
    // forgets.
    if rev.is_empty() || rev.len() > 64 || !rev.chars().all(|c| c.is_ascii_hexdigit()) {
        anyhow::bail!("Refusing to prefetch a non-hex git rev: {:?}", rev);
    }
    let url = format!("github:NixOS/nixpkgs/{}", rev);
    let output = std::process::Command::new("nix")
        .args(["flake", "prefetch", "--json", &url])
        .output()
        .map_err(|e| crate::nix::tools::tool_error("nix", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "`nix flake prefetch {}` failed.\n  \
             This usually means the rev doesn't exist on github:NixOS/nixpkgs,\n  \
             or the network is unavailable.\n\n\
             nix stderr:\n{}",
            url,
            stderr.trim()
        );
    }

    let stdout = std::str::from_utf8(&output.stdout)
        .context("`nix flake prefetch` produced non-UTF-8 output")?;
    let parsed: serde_json::Value = serde_json::from_str(stdout)
        .with_context(|| format!("Could not parse `nix flake prefetch` JSON output: {}", stdout))?;
    let hash = parsed
        .get("hash")
        .and_then(|v| v.as_str())
        .context("`nix flake prefetch` JSON had no 'hash' field")?;
    Ok(hash.to_string())
}

/// Eval `nixpkgs` pinned at `rev` + `nar_hash` and return the `.version`
/// attribute of a named package. Used by the freeze-refresh pass to
/// learn "what version of kicad does today's nixpkgs actually ship?"
/// without going through the user's overlay (which would return the
/// *frozen* version, defeating the point).
///
/// Returns `None` when the eval fails for any reason — missing
/// attribute, `.version` not a string, a malformed rev. Treated by the
/// caller as "unknown, don't touch this entry".
pub fn query_pkg_version_at_rev(rev: &str, nar_hash: &str, pkg_name: &str) -> Option<String> {
    // Defence in depth — same sanity checks as elsewhere before
    // splicing values into the `nix eval --expr` string.
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
    // Package names splice into a Nix attribute path; reject anything
    // that could escape the attribute lookup.
    if pkg_name.is_empty()
        || pkg_name.len() > 128
        || !pkg_name.chars().all(|c| {
            c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == '+'
        })
    {
        return None;
    }

    let expr = format!(
        "let pkgs = import (builtins.fetchTree {{ \
type = \"github\"; owner = \"NixOS\"; repo = \"nixpkgs\"; \
rev = \"{rev}\"; narHash = \"{nar_hash}\"; \
}}) {{ system = builtins.currentSystem; config.allowUnfree = true; }}; \
in pkgs.{pkg_name}.version",
        rev = rev,
        nar_hash = nar_hash,
        pkg_name = pkg_name
    );

    let output = std::process::Command::new("nix")
        .args(["eval", "--impure", "--raw", "--expr", &expr])
        .output()
        .ok()?;
    if !output.status.success() {
        debug!(
            "nix eval failed for '{}' at rev {}: {}",
            pkg_name,
            &rev[..rev.len().min(12)],
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return None;
    }
    let version = String::from_utf8(output.stdout).ok()?;
    let version = version.trim().to_string();
    if version.is_empty() {
        None
    } else {
        Some(version)
    }
}

#[cfg(test)]
#[path = "tests/flake.rs"]
mod tests;
