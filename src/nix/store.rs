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
        .context("Failed to run nix-store. Is this a NixOS system?")?;

    if !output.status.success() {
        anyhow::bail!("nix-store exited with status {}", output.status);
    }

    let stdout = String::from_utf8(output.stdout)
        .context("nix-store output is not valid UTF-8")?;

    let mut packages: HashMap<String, StorePackage> = HashMap::new();

    for line in stdout.lines() {
        let path = line.trim();
        if path.is_empty() {
            continue;
        }

        // Extract the part after the hash: /nix/store/<hash>-<rest>
        let store_name = match extract_store_name(path) {
            Some(name) => name,
            None => {
                trace!("Skipping malformed store path: {}", path);
                continue;
            }
        };

        // Split into name + version
        let (name, version) = match split_name_version(store_name) {
            Some(pair) => pair,
            None => continue,
        };

        // Filter out internal packages
        if is_ignored(&name) {
            trace!("Filtered out: {}", name);
            continue;
        }

        // Deduplicate: keep the first version seen for each name
        let lower_name = name.to_lowercase();
        packages.entry(lower_name).or_insert_with(|| {
            debug!("Found package: {} {}", name, version);
            StorePackage { name, version }
        });
    }

    let count = packages.len();
    debug!("Found {} packages in store", count);

    let mut result: Vec<StorePackage> = packages.into_values().collect();
    result.sort_by_key(|a| a.name.to_lowercase());
    Ok(result)
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
mod tests {
    use super::*;

    #[test]
    fn extract_name_from_store_path() {
        let path = "/nix/store/abc12345678901234567890123456789-legcord-1.5.4";
        assert_eq!(extract_store_name(path), Some("legcord-1.5.4"));
    }

    #[test]
    fn split_simple() {
        assert_eq!(
            split_name_version("legcord-1.5.4"),
            Some(("legcord".into(), "1.5.4".into()))
        );
    }

    #[test]
    fn split_with_plus() {
        assert_eq!(
            split_name_version("gtk+3-3.24.51"),
            Some(("gtk+3".into(), "3.24.51".into()))
        );
    }

    #[test]
    fn split_complex_name() {
        assert_eq!(
            split_name_version("xdg-desktop-portal-1.15.1"),
            Some(("xdg-desktop-portal".into(), "1.15.1".into()))
        );
    }

    #[test]
    fn split_terminfo_ignored() {
        assert_eq!(split_name_version("alacritty-0.17.0-terminfo"), None);
    }

    #[test]
    fn split_platform_ignored() {
        assert_eq!(
            split_name_version("cargo-1.94.1-x86_64-unknown-linux-gnu"),
            None
        );
    }

    #[test]
    fn split_no_version() {
        assert_eq!(split_name_version("some-package-name"), None);
    }

    #[test]
    fn ignore_internal_packages() {
        assert!(is_ignored("libfoo"));
        assert!(is_ignored("gcc-13.2.0"));
        assert!(is_ignored("python3.11-pip"));
        assert!(is_ignored("nixos-rebuild"));
    }

    #[test]
    fn keep_user_packages() {
        assert!(!is_ignored("firefox"));
        assert!(!is_ignored("legcord"));
        assert!(!is_ignored("kicad"));
        assert!(!is_ignored("alacritty"));
    }
}
