use std::collections::HashMap;
use std::process::Command;

use anyhow::{Context, Result};
use regex::Regex;

use crate::types::Package;

/// Store path suffixes to ignore (sub-outputs, not real packages)
const IGNORED_SUFFIXES: &[&str] = &[
    "-terminfo", "-data", "-completions", "-bash-completions",
    "-zsh-completions", "-fish-completions", "-icon-theme",
    "-vim", "-emacs", "-nano", "-out",
    "-x86_64-unknown-linux-gnu", "-aarch64-unknown-linux-gnu",
    "-init", "-host", "-man", "-doc", "-dev", "-info",
    ".svg", ".png", ".desktop",
];

/// Read installed packages by cross-referencing NixOS config with the store
pub fn read_installed_packages() -> Result<Vec<Package>> {
    // 1. Read all store paths with versions
    let store_packages = read_store_paths()?;

    // 2. Read package names from NixOS config
    let config_names = read_config_package_names();

    // 3. Cross-reference: keep only packages that are in the config
    let mut packages: Vec<Package> = Vec::new();
    let mut seen_names: std::collections::HashSet<String> = std::collections::HashSet::new();

    for config_name in &config_names {
        let lower = config_name.to_lowercase();
        // Look up in the store by name (flexible match)
        if let Some(store_pkg) = store_packages.get(&lower) {
            if seen_names.insert(lower.clone()) {
                packages.push(Package::new(
                    config_name.clone(),
                    store_pkg.clone(),
                ));
            }
        }
        // If not found in the store -> not installed, skip
    }

    packages.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(packages)
}

/// Read store paths and return a name -> version map
fn read_store_paths() -> Result<HashMap<String, String>> {
    let output = Command::new("nix-store")
        .args(["-qR", "/run/current-system/sw"])
        .output()
        .context("Unable to run nix-store")?;

    let stdout = String::from_utf8(output.stdout)
        .context("Invalid nix-store output (UTF-8)")?;

    let store_path_re = Regex::new(r"^/nix/store/[a-z0-9]{32}-(.+)$")
        .context("Invalid regex")?;

    let mut packages: HashMap<String, String> = HashMap::new();

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let captures = match store_path_re.captures(trimmed) {
            Some(c) => c,
            None => continue,
        };
        let store_name = &captures[1];

        let (name, version) = match split_name_version(store_name) {
            Some(pair) => pair,
            None => continue,
        };

        // Keep the first version found (the shortest/cleanest)
        let lower_name = name.to_lowercase();
        packages.entry(lower_name).or_insert(version);
    }

    Ok(packages)
}

/// Read package names from NixOS config files
fn read_config_package_names() -> Vec<String> {
    let config_dir = dirs::home_dir()
        .map(|h| h.join("nixos-config"))
        .unwrap_or_default();

    let mut names = Vec::new();

    // Scan all .nix files in modules/ and home/
    let dirs_to_scan = [
        config_dir.join("modules"),
        config_dir.join("home"),
        config_dir.join("hosts"),
    ];

    for dir in &dirs_to_scan {
        if let Ok(entries) = glob_nix_files(dir) {
            for file_path in entries {
                if let Ok(content) = std::fs::read_to_string(&file_path) {
                    extract_package_names(&content, &mut names);
                }
            }
        }
    }

    // Deduplicate
    names.sort();
    names.dedup();
    names
}

/// Recursively scan a directory for .nix files
fn glob_nix_files(dir: &std::path::Path) -> Result<Vec<std::path::PathBuf>> {
    let mut files = Vec::new();

    if !dir.exists() {
        return Ok(files);
    }

    let entries = std::fs::read_dir(dir)
        .context("Unable to read directory")?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(glob_nix_files(&path)?);
        } else if path.extension().is_some_and(|e| e == "nix") {
            files.push(path);
        }
    }

    Ok(files)
}

/// Extract package names from a .nix file
/// Looks for common patterns: `pkgs.name`, `name` in systemPackages/home.packages
fn extract_package_names(content: &str, names: &mut Vec<String>) {
    // Pattern 1: lines with just a package name (indentation + name)
    // Inside a `with pkgs; [ ... ]` or `environment.systemPackages` block
    let pkg_line_re = Regex::new(
        r"(?m)^\s+([a-zA-Z][a-zA-Z0-9_-]*(?:\.[a-zA-Z][a-zA-Z0-9_-]*)*)\s*(?:#.*)?$"
    ).unwrap();

    // Pattern 2: pkgs.name
    let pkgs_re = Regex::new(r"pkgs\.([a-zA-Z][a-zA-Z0-9_-]*)").unwrap();

    let in_packages_block = content.contains("systemPackages")
        || content.contains("home.packages")
        || content.contains("plugins");

    if in_packages_block {
        for cap in pkg_line_re.captures_iter(content) {
            let name = &cap[1];
            // Filter out Nix keywords and things that aren't packages
            if !is_nix_keyword(name) && !name.contains("..") {
                // Handle namespaced names (kdePackages.elisa -> elisa)
                let final_name = if let Some(pos) = name.rfind('.') {
                    &name[pos + 1..]
                } else {
                    name
                };
                if !final_name.is_empty() && !is_nix_keyword(final_name) {
                    names.push(final_name.to_string());
                }
            }
        }
    }

    for cap in pkgs_re.captures_iter(content) {
        let name = &cap[1];
        if !is_nix_keyword(name) {
            names.push(name.to_string());
        }
    }
}

/// Check if a word is a Nix keyword/builtin (not a package)
fn is_nix_keyword(name: &str) -> bool {
    const KEYWORDS: &[&str] = &[
        "with", "let", "in", "if", "then", "else", "inherit", "rec",
        "true", "false", "null", "import", "builtins", "config",
        "pkgs", "lib", "inputs", "outputs", "self", "super",
        "enable", "default", "options", "mkIf", "mkOption",
        "mkDefault", "mkForce", "mkMerge", "mkOverride",
        "environment", "services", "programs", "system",
        "systemPackages", "home", "hostname", "fetchFromGitHub",
        "stdenv", "mkDerivation", "buildFHSEnv", "writeShellScriptBin",
        "symlinkJoin", "makeWrapper", "wrapProgram",
        "buildInputs", "nativeBuildInputs", "propagatedBuildInputs",
        "postBuild", "buildPhase", "installPhase",
        "name", "version", "src", "owner", "repo", "rev", "hash",
        "pname", "meta", "description", "license", "homepage",
        "platforms", "maintainers", "broken",
        "pathsToLink", "sessionVariables",
        "extraRules", "extraConfig", "text",
    ];
    KEYWORDS.contains(&name)
}

/// Split a store name into (package_name, version)
fn split_name_version(store_name: &str) -> Option<(String, String)> {
    // Ignore sub-outputs
    for suffix in IGNORED_SUFFIXES {
        if store_name.ends_with(suffix) {
            return None;
        }
    }

    // Find the last hyphen followed by a digit
    let bytes = store_name.as_bytes();
    let mut split_pos = None;

    for i in (0..bytes.len()).rev() {
        let is_hyphen = bytes[i] == b'-';
        let next_is_digit = i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit();

        if is_hyphen && next_is_digit {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_name_version_simple() {
        let result = split_name_version("legcord-1.5.4");
        assert_eq!(result, Some(("legcord".to_string(), "1.5.4".to_string())));
    }

    #[test]
    fn test_split_name_version_with_plus() {
        let result = split_name_version("gtk+3-3.24.51");
        assert_eq!(result, Some(("gtk+3".to_string(), "3.24.51".to_string())));
    }

    #[test]
    fn test_split_terminfo_ignored() {
        let result = split_name_version("alacritty-0.17.0-terminfo");
        assert_eq!(result, None);
    }

    #[test]
    fn test_split_platform_ignored() {
        let result = split_name_version("cargo-1.94.1-x86_64-unknown-linux-gnu");
        assert_eq!(result, None);
    }

    #[test]
    fn test_is_not_keyword() {
        assert!(!is_nix_keyword("firefox"));
        assert!(!is_nix_keyword("legcord"));
    }

    #[test]
    fn test_is_keyword() {
        assert!(is_nix_keyword("enable"));
        assert!(is_nix_keyword("pkgs"));
        assert!(is_nix_keyword("config"));
    }
}
