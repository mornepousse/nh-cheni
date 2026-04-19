//! Read installed packages from the nix store.
//!
//! Parses the output of `nix-store -qR /run/current-system/sw` to extract
//! package names and versions from store paths.
//!
//! Store paths look like: /nix/store/<hash>-<name>-<version>
//! The challenge is splitting name from version, since names can contain
//! hyphens and numbers (e.g. "gtk+3-3.24.51").

use std::collections::HashMap;
use std::process::Command;

use anyhow::{Context, Result};
use tracing::{debug, trace};

/// A package found in the nix store.
#[derive(Debug, Clone)]
pub struct StorePackage {
    pub name: String,
    pub version: String,
}

/// Suffixes that indicate a sub-output, not a real package.
/// These store paths are filtered out.
const IGNORED_SUFFIXES: &[&str] = &[
    "-terminfo", "-data", "-completions", "-bash-completions",
    "-zsh-completions", "-fish-completions", "-icon-theme",
    "-vim", "-emacs", "-nano", "-out",
    "-x86_64-unknown-linux-gnu", "-aarch64-unknown-linux-gnu",
    "-init", "-host", "-man", "-doc", "-dev", "-info",
    ".svg", ".png", ".desktop",
    // Source archive downloads — pulled into the store next to the
    // real package, but they're not the package itself. Without this
    // filter `displaylink-620.zip` was getting parsed as version
    // "620.zip" and shadowing the actual `displaylink-6.2.0-30`.
    ".zip", ".tar.gz", ".tar.bz2", ".tar.xz", ".tar.zst",
    ".tgz", ".tbz2", ".txz",
];

/// Prefixes that indicate an internal/system package.
/// These are not interesting to the user.
const IGNORED_PREFIXES: &[&str] = &[
    "lib", "gcc-", "glibc", "bash-", "perl-", "perl5",
    "python3.", "python3-", "nix-", "hook", "wrap", "setup",
    "env-", "profile", "system-path", "nixos-", "stdenv",
    "binutils-", "coreutils-", "patchelf-", "patch-",
    "attr-", "acl-", "audit-", "xz-", "zlib-", "bzip2-",
    "expand-response", "gnu", "linux-headers", "man-pages",
    "tzdata-", "mailcap-", "mime-types", "strip", "compress",
    "move-docs", "move-lib64", "move-sbin", "multiple-outputs",
    "make-symlinks", "patch-shebangs", "audit-tmpdir",
    "prune-libtool", "reproducible-builds", "set-source-date",
    "update-autotools", "fixup-",
];

/// Read all installed packages from the nix store.
///
/// Runs `nix-store -qR /run/current-system/sw`, parses store paths,
/// filters out noise, and returns deduplicated packages.
pub fn read_installed_packages() -> Result<Vec<StorePackage>> {
    debug!("Reading installed packages from nix store");

    let output = Command::new("nix-store")
        .args(["-qR", "/run/current-system/sw"])
        .output()
        .map_err(|e| crate::nix::tools::tool_error("nix-store", e))?;

    if !output.status.success() {
        anyhow::bail!("nix-store exited with status {}", output.status);
    }

    let stdout = String::from_utf8(output.stdout)
        .context("nix-store output is not valid UTF-8")?;

    // Same package name may appear in the store under several derivations
    // (e.g. mesa shows up as `mesa-24.3.2-osmesa` AND `mesa-26.0.4` when
    // an older variant is still pulled by another closure). We collect
    // every version we see and pick the highest below — otherwise the
    // first one encountered (driven by store iteration order) could be
    // a stale or sub-variant entry, leading to bogus "update available"
    // reports against the wrong installed version.
    let mut versions: HashMap<String, (String, Vec<String>)> = HashMap::new();

    for line in stdout.lines() {
        let path = line.trim();
        if path.is_empty() {
            continue;
        }

        let store_name = match extract_store_name(path) {
            Some(name) => name,
            None => {
                trace!("Skipping malformed store path: {}", path);
                continue;
            }
        };

        let (name, version) = match split_name_version(store_name) {
            Some(pair) => pair,
            None => continue,
        };

        if is_ignored(&name) {
            trace!("Filtered out: {}", name);
            continue;
        }

        let lower_name = name.to_lowercase();
        let entry = versions
            .entry(lower_name)
            .or_insert_with(|| (name.clone(), Vec::new()));
        entry.1.push(version);
    }

    let mut result: Vec<StorePackage> = Vec::with_capacity(versions.len());
    for (_, (display_name, vers)) in versions {
        let chosen = pick_highest_version(&vers);
        debug!("Resolved {}: {:?} → {}", display_name, vers, chosen);
        result.push(StorePackage {
            name: display_name,
            version: chosen,
        });
    }

    let count = result.len();
    debug!("Found {} packages in store", count);
    result.sort_by_key(|a| a.name.to_lowercase());
    Ok(result)
}

/// Pick the "highest" version from a list of candidates for the same
/// package name. Uses the same parser/comparator as the rest of cheni
/// so semantic versions sort the way users expect (26.0.4 > 24.3.2,
/// even when one ends with a "-osmesa" sub-output suffix).
///
/// Returns an empty string on an empty input rather than panicking —
/// every current caller guarantees at least one element, but a defensive
/// guard keeps this function safely reusable.
fn pick_highest_version(versions: &[String]) -> String {
    use crate::version::compare::compare_versions;
    use crate::version::compare::VersionDiff;
    use crate::version::parse::parse_version;

    let Some(first) = versions.first() else {
        return String::new();
    };
    let mut best = first.clone();
    for v in &versions[1..] {
        // compare_versions(installed, available) returns:
        //   Newer       -> installed (best) is ahead of v   -> keep best
        //   Minor/Major -> v is ahead of best               -> switch to v
        //   Equal       -> tie                              -> keep best
        let cmp = compare_versions(&parse_version(&best), &parse_version(v));
        if matches!(cmp, VersionDiff::Minor | VersionDiff::Major) {
            best = v.clone();
        }
    }
    best
}

/// Extract the name part from a store path.
///
/// Input:  "/nix/store/abc123...-legcord-1.5.4"
/// Output: "legcord-1.5.4"
fn extract_store_name(path: &str) -> Option<&str> {
    // Format: /nix/store/<32 char hash>-<rest>
    let after_store = path.strip_prefix("/nix/store/")?;

    // The hash is 32 chars followed by a hyphen
    if after_store.len() < 34 {
        return None;
    }

    Some(&after_store[33..])
}

/// Split a store name into (package_name, version).
///
/// Heuristic: find the last hyphen followed by a digit.
///
/// Examples:
///   "legcord-1.5.4"              → ("legcord", "1.5.4")
///   "gtk+3-3.24.51"              → ("gtk+3", "3.24.51")
///   "xdg-desktop-portal-1.15.1"  → ("xdg-desktop-portal", "1.15.1")
fn split_name_version(store_name: &str) -> Option<(String, String)> {
    // First, reject sub-outputs
    for suffix in IGNORED_SUFFIXES {
        if store_name.ends_with(suffix) {
            trace!("Ignored suffix '{}' in: {}", suffix, store_name);
            return None;
        }
    }

    // Find the last hyphen followed by a digit
    let bytes = store_name.as_bytes();
    let mut split_pos = None;

    for i in (0..bytes.len()).rev() {
        if bytes[i] == b'-' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() {
            split_pos = Some(i);
            break;
        }
    }

    let pos = split_pos?;
    let name = &store_name[..pos];
    let version = &store_name[pos + 1..];

    if name.is_empty() || version.is_empty() {
        return None;
    }

    Some((name.to_string(), version.to_string()))
}

/// Check if a package name should be filtered out.
fn is_ignored(name: &str) -> bool {
    let lower = name.to_lowercase();

    // Check prefixes
    for prefix in IGNORED_PREFIXES {
        if lower.starts_with(prefix) {
            return true;
        }
    }

    // Check patterns that indicate internal components
    lower.contains("-hook")
        || lower.contains("-wrapper")
        || lower.ends_with(".drv")
}

#[cfg(test)]
#[path = "tests/store.rs"]
mod tests;
