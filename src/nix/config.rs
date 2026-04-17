//! NixOS configuration detection.
//!
//! Finds the user's flake.nix and determines the hostname
//! to use for nixosConfigurations.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use tracing::{debug, trace, warn};

/// Detected NixOS configuration.
#[derive(Debug, Clone)]
pub struct NixConfig {
    /// Path to the directory containing flake.nix.
    pub flake_dir: PathBuf,
    /// System hostname (matched against nixosConfigurations).
    pub hostname: String,
}

/// Detect the NixOS configuration.
///
/// Searches for flake.nix in standard locations, then detects the hostname.
pub fn detect() -> Result<NixConfig> {
    let flake_dir = find_flake_dir()
        .context("Could not find a NixOS flake configuration.\nHint: run nixup from your config directory, or set $NIXUP_CONFIG")?;

    debug!("Config found: {}", flake_dir.display());

    let hostname = detect_hostname()?;
    debug!("Hostname: {}", hostname);

    Ok(NixConfig { flake_dir, hostname })
}

/// Search for flake.nix in standard locations.
///
/// Priority order:
/// 1. $NIXUP_CONFIG environment variable
/// 2. Current directory
/// 3. ~/nixos-config
/// 4. /etc/nixos
fn find_flake_dir() -> Option<PathBuf> {
    // 1. Environment variable
    if let Ok(env_path) = std::env::var("NIXUP_CONFIG") {
        let path = PathBuf::from(&env_path);
        if has_flake(&path) {
            debug!("Using $NIXUP_CONFIG: {}", path.display());
            return Some(path);
        }
        warn!("$NIXUP_CONFIG is set to '{}' but no flake.nix found there", env_path);
    }

    // 2. Current directory
    let cwd = std::env::current_dir().ok()?;
    if has_flake(&cwd) {
        debug!("Using current directory: {}", cwd.display());
        return Some(cwd);
    }

    // 3. ~/nixos-config
    if let Some(home) = dirs::home_dir() {
        let nixos_config = home.join("nixos-config");
        if has_flake(&nixos_config) {
            debug!("Using ~/nixos-config");
            return Some(nixos_config);
        }
    }

    // 4. /etc/nixos
    let etc_nixos = PathBuf::from("/etc/nixos");
    if has_flake(&etc_nixos) {
        debug!("Using /etc/nixos");
        return Some(etc_nixos);
    }

    None
}

/// Check if a directory contains a flake.nix.
fn has_flake(dir: &Path) -> bool {
    dir.join("flake.nix").exists()
}

/// Detect the system hostname.
fn detect_hostname() -> Result<String> {
    let output = Command::new("hostname")
        .output()
        .context("Failed to run 'hostname' command")?;

    let hostname = String::from_utf8(output.stdout)
        .context("Hostname is not valid UTF-8")?
        .trim()
        .to_string();

    if hostname.is_empty() {
        anyhow::bail!("Hostname is empty");
    }

    Ok(hostname)
}

/// List the module directories under modules/.
///
/// Returns directory names like ["apps", "desktop", "dev", "hardware"].
/// These become the valid values for --<category> filters.
pub fn list_module_categories(flake_dir: &Path) -> Vec<String> {
    let modules_dir = flake_dir.join("modules");

    if !modules_dir.exists() {
        debug!("No modules/ directory found in {}", flake_dir.display());
        return Vec::new();
    }

    let mut categories = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&modules_dir) {
        for entry in entries.flatten() {
            if entry.path().is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    categories.push(name.to_string());
                }
            }
        }
    }

    categories.sort();
    debug!("Module categories: {:?}", categories);
    categories
}

/// List all .nix files in a module category directory.
///
/// For example, `list_module_files(flake_dir, "dev")` returns all .nix files
/// under `modules/dev/`.
pub fn list_module_files(flake_dir: &Path, category: &str) -> Vec<PathBuf> {
    let dir = flake_dir.join("modules").join(category);
    list_nix_files_recursive(&dir)
}

/// Recursively list all .nix files in a directory.
fn list_nix_files_recursive(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();

    if !dir.exists() {
        return files;
    }

    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(list_nix_files_recursive(&path));
            } else if path.extension().is_some_and(|e| e == "nix") {
                files.push(path);
            }
        }
    }

    files
}

/// Extract package names from .nix files.
///
/// Looks for common patterns in NixOS module files:
/// - Bare package names inside `with pkgs; [ ... ]` blocks.
/// - `pkgs.name` references.
pub fn extract_package_names(nix_files: &[PathBuf]) -> Vec<String> {
    let mut names = Vec::new();

    let pkgs_re = regex::Regex::new(r"pkgs\.([a-zA-Z][a-zA-Z0-9_-]*)").unwrap();
    let pkg_line_re = regex::Regex::new(
        r"(?m)^\s+([a-zA-Z][a-zA-Z0-9_-]*(?:\.[a-zA-Z][a-zA-Z0-9_-]*)*)\s*(?:#.*)?$"
    ).unwrap();

    for file_path in nix_files {
        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let in_packages_block = content.contains("systemPackages")
            || content.contains("home.packages")
            || content.contains("plugins");

        // Extract pkgs.name references
        for cap in pkgs_re.captures_iter(&content) {
            let name = &cap[1];
            if !is_nix_keyword(name) {
                trace!("Found pkgs.{} in {}", name, file_path.display());
                names.push(name.to_string());
            }
        }

        // Extract bare package names in package blocks
        if in_packages_block {
            for cap in pkg_line_re.captures_iter(&content) {
                let name = &cap[1];
                if is_nix_keyword(name) || name.contains("..") {
                    continue;
                }

                // Handle namespaced names (kdePackages.elisa → elisa)
                let final_name = match name.rfind('.') {
                    Some(pos) => &name[pos + 1..],
                    None => name,
                };

                if !final_name.is_empty() && !is_nix_keyword(final_name) {
                    trace!("Found {} in {}", final_name, file_path.display());
                    names.push(final_name.to_string());
                }
            }
        }
    }

    // Deduplicate
    names.sort();
    names.dedup();
    names
}

/// Check if a word is a Nix keyword or builtin (not a package name).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nix_keywords_detected() {
        assert!(is_nix_keyword("enable"));
        assert!(is_nix_keyword("pkgs"));
        assert!(is_nix_keyword("mkDerivation"));
    }

    #[test]
    fn package_names_not_keywords() {
        assert!(!is_nix_keyword("firefox"));
        assert!(!is_nix_keyword("legcord"));
        assert!(!is_nix_keyword("kicad"));
    }
}
