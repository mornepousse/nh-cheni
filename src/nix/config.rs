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
        .context("Could not find a NixOS flake configuration.\nHint: run cheni from your config directory, or set $CHENI_CONFIG")?;

    debug!("Config found: {}", flake_dir.display());

    let hostname = detect_hostname()?;
    debug!("Hostname: {}", hostname);

    Ok(NixConfig { flake_dir, hostname })
}

/// Search for flake.nix in standard locations.
///
/// Priority order:
/// 1. $CHENI_CONFIG environment variable (any flake.nix accepted, user chose it)
/// 2. Current directory — only if it looks like a NixOS-config flake
///    (must declare `nixosConfigurations`). This avoids picking up unrelated
///    flakes such as cheni's own source tree when the user happens to be in it.
/// 3. ~/nixos-config
/// 4. /etc/nixos
fn find_flake_dir() -> Option<PathBuf> {
    // 1. Environment variable
    if let Ok(env_path) = std::env::var("CHENI_CONFIG") {
        let path = PathBuf::from(&env_path);
        if has_flake(&path) {
            debug!("Using $CHENI_CONFIG: {}", path.display());
            return Some(path);
        }
        warn!("$CHENI_CONFIG is set to '{}' but no flake.nix found there", env_path);
    }

    // 2. Current directory — but only if it's a NixOS-config flake.
    let cwd = std::env::current_dir().ok()?;
    if is_nixos_config_flake(&cwd) {
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

/// Check whether a directory contains a flake.nix that defines a NixOS
/// configuration (looks for the `nixosConfigurations` attribute).
///
/// This is a textual check — it reads the file but doesn't evaluate it.
/// Good enough to distinguish "the user's NixOS config flake" from
/// random package/library flakes (like cheni's own).
fn is_nixos_config_flake(dir: &Path) -> bool {
    let flake_path = dir.join("flake.nix");
    match std::fs::read_to_string(&flake_path) {
        Ok(content) => content.contains("nixosConfigurations"),
        Err(_) => false,
    }
}

/// Walk `imports = [ ... ];` recursively from the host (and home-manager)
/// entry points and return the set of `.nix` files actually pulled into
/// the build for `hostname`. Skips commented-out imports.
///
/// Used by `cheni check` so that the "in <file>" annotation only points
/// to modules that are really active — without this, a commented-out
/// `modules/dev/lpc40.nix` could be reported as the source for `nspr`.
///
/// Returns `None` if no entry point can be found (very exotic layouts);
/// callers should then fall back to scanning every module file.
pub fn list_active_modules(flake_dir: &Path, hostname: &str) -> Option<Vec<PathBuf>> {
    let mut visited: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

    // Entry 1: hosts/<hostname>/default.nix (system-level imports)
    let host_entry = flake_dir.join("hosts").join(hostname).join("default.nix");
    let mut found_any = false;
    if host_entry.exists() {
        walk_imports(&host_entry, &mut visited);
        found_any = true;
    }

    // Entry 2: home-manager file referenced from flake.nix
    if let Some(home_user) = find_home_manager_user(flake_dir) {
        let home_entry = flake_dir.join("home").join(format!("{}.nix", home_user));
        if home_entry.exists() {
            walk_imports(&home_entry, &mut visited);
            found_any = true;
        }
    }

    if !found_any {
        return None;
    }
    Some(visited.into_iter().collect())
}

/// Look for `home-manager.users.<NAME> = import ./home/<NAME>.nix` in
/// flake.nix. Returns the first user name found, or None.
fn find_home_manager_user(flake_dir: &Path) -> Option<String> {
    let content = std::fs::read_to_string(flake_dir.join("flake.nix")).ok()?;
    let re = regex::Regex::new(r"home-manager\.users\.([a-zA-Z_][a-zA-Z0-9_-]*)").ok()?;
    re.captures(&content)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// Maximum recursion depth for `walk_imports`.
///
/// `canonicalize` + the `visited` HashSet already make a true cycle
/// terminate at the second visit. This bound exists for the rarer
/// pathological case where a hand-crafted (or generated) flake nests
/// imports really deeply through ever-changing canonical paths.
/// 64 is well above any realistic NixOS config (typical: 5–10).
const MAX_IMPORT_DEPTH: usize = 64;

/// Recursively follow `imports = [...]` from a starting `.nix` file,
/// inserting each visited file into `visited`. Commented-out lines are
/// skipped, and only relative paths (`./foo.nix`, `../bar`) are followed
/// — `inputs.something.nixosModules.x` is opaque so we leave it alone.
fn walk_imports(file: &Path, visited: &mut std::collections::HashSet<PathBuf>) {
    walk_imports_inner(file, visited, 0);
}

fn walk_imports_inner(
    file: &Path,
    visited: &mut std::collections::HashSet<PathBuf>,
    depth: usize,
) {
    if depth >= MAX_IMPORT_DEPTH {
        debug!(
            "walk_imports: depth limit ({}) reached at {}",
            MAX_IMPORT_DEPTH,
            file.display()
        );
        return;
    }

    let canon = match file.canonicalize() {
        Ok(c) => c,
        Err(_) => return,
    };
    if !visited.insert(canon.clone()) {
        return; // already walked
    }

    let content = match std::fs::read_to_string(&canon) {
        Ok(c) => c,
        Err(_) => return,
    };

    let parent = match canon.parent() {
        Some(p) => p,
        None => return,
    };

    for import in parse_imports(&content) {
        // Resolve relative to the current file's parent
        let resolved = parent.join(&import);
        let target = if resolved.is_dir() {
            resolved.join("default.nix")
        } else if resolved.exists() {
            resolved
        } else {
            // Try adding .nix extension
            let with_ext = resolved.with_extension("nix");
            if with_ext.exists() {
                with_ext
            } else {
                continue;
            }
        };
        walk_imports_inner(&target, visited, depth + 1);
    }
}

/// Pull relative `.nix` paths out of an `imports = [ ... ];` block.
/// Strips line and inline comments, ignores `inputs.*` references.
fn parse_imports(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut idx = 0;

    // Walk every `imports = [` occurrence — there can be more than one
    // (e.g. specialisations, conditional sub-blocks).
    while let Some(pos) = content[idx..].find("imports") {
        let abs = idx + pos;
        let after = &content[abs + "imports".len()..];
        // Expect "= [" (with optional whitespace), otherwise it might be
        // a comment / different identifier — skip.
        let trimmed = after.trim_start();
        if !trimmed.starts_with('=') {
            idx = abs + 1;
            continue;
        }
        let after_eq = trimmed[1..].trim_start();
        // Skip "with lib;" prefix if present
        let after_with = if let Some(rest) = after_eq.strip_prefix("with") {
            // skip until ';'
            match rest.find(';') {
                Some(semi) => rest[semi + 1..].trim_start(),
                None => after_eq,
            }
        } else {
            after_eq
        };
        if !after_with.starts_with('[') {
            idx = abs + 1;
            continue;
        }
        // Find matching close bracket (no nested handling — imports blocks
        // are flat in practice).
        let block_start_in_after = after_with.as_ptr() as usize - content.as_ptr() as usize + 1;
        let close = match content[block_start_in_after..].find(']') {
            Some(c) => block_start_in_after + c,
            None => break,
        };
        let block = &content[block_start_in_after..close];
        for raw_line in block.lines() {
            let mut line = raw_line.trim();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if let Some(hash) = line.find('#') {
                line = line[..hash].trim();
            }
            if line.is_empty() {
                continue;
            }
            // The block may have multiple imports on one line separated by space
            for token in line.split_whitespace() {
                if token.starts_with("./") || token.starts_with("../") {
                    out.push(token.trim_end_matches(';').to_string());
                }
            }
        }
        idx = close + 1;
    }
    out
}

/// Has `cheni init` been run? Looks for `nixpkgs-latest` in the flake
/// (either the source flake.nix or the lock file). Used by gateway
/// commands to surface a friendly message before failing in surprising
/// ways further down the line.
pub fn is_initialized(flake_dir: &Path) -> bool {
    let flake_text = std::fs::read_to_string(flake_dir.join("flake.nix")).unwrap_or_default();
    if flake_text.contains("nixpkgs-latest") {
        return true;
    }
    // Fall back to the lock file in case the input is declared via include
    let lock_text = std::fs::read_to_string(flake_dir.join("flake.lock")).unwrap_or_default();
    lock_text.contains("\"nixpkgs-latest\"")
}

/// Detect the system hostname. Falls back to reading /etc/hostname
/// if the `hostname` binary isn't in PATH, so cheni keeps working on
/// a minimal environment (rescue shell, container, etc.).
fn detect_hostname() -> Result<String> {
    if let Ok(output) = Command::new("hostname").output() {
        if let Ok(s) = String::from_utf8(output.stdout) {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
    }

    // Fallback: read /etc/hostname directly
    if let Ok(content) = std::fs::read_to_string("/etc/hostname") {
        let trimmed = content.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }

    // Last resort: $HOSTNAME env var
    if let Ok(env_host) = std::env::var("HOSTNAME") {
        if !env_host.is_empty() {
            return Ok(env_host);
        }
    }

    anyhow::bail!(
        "Could not determine hostname: 'hostname' binary not found, \
         /etc/hostname empty or missing, and $HOSTNAME not set."
    )
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
    let mut names: Vec<String> = extract_package_names_with_files(nix_files)
        .into_keys()
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Same as [`extract_package_names`] but also tracks which file(s) declared
/// each package. Used by `cheni check` to point users at the right .nix
/// file for an outdated package without a separate `cheni why` round-trip.
pub fn extract_package_names_with_files(
    nix_files: &[PathBuf],
) -> std::collections::HashMap<String, Vec<PathBuf>> {
    // Regex patterns are hardcoded and unit-tested — compile once.
    // Lazy static so a 'cheni check' loop running over ~100 files only
    // pays the compile cost the first time the extractor is called.
    static PKGS_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"pkgs\.([a-zA-Z][a-zA-Z0-9_-]*)").expect("valid regex")
    });
    static PKG_LINE_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(?m)^\s+([a-zA-Z][a-zA-Z0-9_-]*(?:\.[a-zA-Z][a-zA-Z0-9_-]*)*)\s*(?:#.*)?$",
        )
        .expect("valid regex")
    });

    let mut by_name: std::collections::HashMap<String, Vec<PathBuf>> =
        std::collections::HashMap::new();
    for file_path in nix_files {
        let Ok(content) = std::fs::read_to_string(file_path) else { continue };
        scan_one_file(&content, file_path, &PKGS_RE, &PKG_LINE_RE, &mut by_name);
    }
    by_name
}

/// Extract every package reference from a single .nix file. Two passes:
/// `pkgs.NAME` everywhere, plus bare names inside package blocks
/// (`systemPackages` / `home.packages` / `plugins`). The latter is
/// gated because outside those blocks bare identifiers can be anything
/// (option names, function args, …).
fn scan_one_file(
    content: &str,
    file_path: &Path,
    pkgs_re: &regex::Regex,
    pkg_line_re: &regex::Regex,
    by_name: &mut std::collections::HashMap<String, Vec<PathBuf>>,
) {
    let in_packages_block = content.contains("systemPackages")
        || content.contains("home.packages")
        || content.contains("plugins");

    let mut record = |name: String| {
        let entry = by_name.entry(name).or_default();
        if !entry.contains(&file_path.to_path_buf()) {
            entry.push(file_path.to_path_buf());
        }
    };

    for cap in pkgs_re.captures_iter(content) {
        let name = &cap[1];
        if !is_nix_keyword(name) {
            trace!("Found pkgs.{} in {}", name, file_path.display());
            record(name.to_string());
        }
    }

    if !in_packages_block {
        return;
    }
    for cap in pkg_line_re.captures_iter(content) {
        let name = &cap[1];
        if is_nix_keyword(name) || name.contains("..") {
            continue;
        }
        // Namespaced names (kdePackages.elisa → elisa) so the package-DB
        // lookup hits on the leaf name the user actually installed.
        let final_name = match name.rfind('.') {
            Some(pos) => &name[pos + 1..],
            None => name,
        };
        if !final_name.is_empty() && !is_nix_keyword(final_name) {
            trace!("Found {} in {}", final_name, file_path.display());
            record(final_name.to_string());
        }
    }
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
#[path = "tests/config.rs"]
mod tests;
