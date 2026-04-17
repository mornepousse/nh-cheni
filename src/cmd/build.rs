//! `nixup build` command.
//!
//! Wraps `nh os switch` and parses Nix build errors into
//! human-readable messages with hints for fixing them.

use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::debug;

use crate::nix::config;

/// A parsed build error with a human-readable explanation.
struct ParsedError {
    /// What failed (package name or eval path).
    what: String,
    /// Error category for display.
    category: &'static str,
    /// Human-readable explanation.
    message: String,
    /// Suggested fix.
    hint: Option<String>,
}

/// Run `nixup build`.
///
/// Wraps `nh os switch` and, if it fails, parses the error output
/// to provide human-readable error messages and fix suggestions.
pub fn run() -> Result<()> {
    let nix_config = config::detect()?;
    let config_path = nix_config.flake_dir.to_str()
        .context("Config path is not valid UTF-8")?;

    println!(
        "{}\n",
        "=== nixup build ===".bold()
    );

    // Run nh os switch and capture stderr
    let output = Command::new("nh")
        .args(["os", "switch", config_path])
        .stdout(Stdio::inherit()) // Pass stdout through for progress
        .stderr(Stdio::piped())   // Capture stderr for error parsing
        .output()
        .context("Failed to run 'nh os switch'. Is nh installed?")?;

    if output.status.success() {
        println!("\n{} Build successful!", "✓".green());
        return Ok(());
    }

    // Build failed — parse the error output
    let stderr = String::from_utf8_lossy(&output.stderr);
    debug!("Build failed, parsing {} bytes of stderr", stderr.len());

    let errors = parse_errors(&stderr);

    if errors.is_empty() {
        // Couldn't parse the error — show raw output
        println!("\n{} Build failed.\n", "✗".red());
        eprintln!("{}", stderr);
        return Ok(());
    }

    // Show parsed errors
    println!("\n{} Build failed with {} error(s):\n", "✗".red(), errors.len());

    for (i, error) in errors.iter().enumerate() {
        println!(
            "  {}  {} — {}",
            format!("[{}]", i + 1).red(),
            error.category.yellow(),
            error.what.bold()
        );
        println!("      {}", error.message);
        if let Some(hint) = &error.hint {
            println!("      {} {}", "Hint:".cyan(), hint);
        }
        println!();
    }

    Ok(())
}

/// Parse Nix build errors from stderr output.
fn parse_errors(stderr: &str) -> Vec<ParsedError> {
    let mut errors = Vec::new();

    // Process line by line, looking for error patterns
    let lines: Vec<&str> = stderr.lines().collect();

    for (i, line) in lines.iter().enumerate() {
        // Pattern 1: Hash mismatch
        if line.contains("hash mismatch") || line.contains("sha256 mismatch") {
            if let Some(error) = parse_hash_mismatch(&lines, i) {
                errors.push(error);
            }
        }

        // Pattern 2: Unfree package
        if line.contains("is not free") || line.contains("unfree") && line.contains("refused") {
            if let Some(error) = parse_unfree(&lines, i) {
                errors.push(error);
            }
        }

        // Pattern 3: Broken package
        if line.contains("is marked as broken") {
            if let Some(error) = parse_broken(&lines, i) {
                errors.push(error);
            }
        }

        // Pattern 4: Eval error — undefined variable
        if line.contains("undefined variable") {
            if let Some(error) = parse_undefined_var(&lines, i) {
                errors.push(error);
            }
        }

        // Pattern 5: Eval error — infinite recursion
        if line.contains("infinite recursion") {
            errors.push(ParsedError {
                what: "Nix evaluation".to_string(),
                category: "Infinite recursion",
                message: "The configuration caused an infinite recursion during evaluation.".to_string(),
                hint: Some("Check overlays for circular references. Use '--show-trace' for details.".to_string()),
            });
        }

        // Pattern 6: File not found (git staging)
        if line.contains("path") && line.contains("does not exist") {
            if let Some(error) = parse_path_not_found(&lines, i) {
                errors.push(error);
            }
        }

        // Pattern 7: Builder failed
        if line.contains("builder for") && line.contains("failed") {
            if let Some(error) = parse_builder_failed(&lines, i) {
                errors.push(error);
            }
        }

        // Pattern 8: Collision between packages
        if line.contains("collision between") || (line.contains("collides with") && line.contains("nix/store")) {
            if let Some(error) = parse_collision(&lines, i) {
                errors.push(error);
            }
        }

        // Pattern 9: cargoHash out of date
        if line.contains("cargoHash") && line.contains("out of date") {
            errors.push(ParsedError {
                what: "Cargo hash".to_string(),
                category: "Hash mismatch",
                message: "Cargo.lock changed but cargoHash in the derivation is outdated.".to_string(),
                hint: Some("Set cargoHash = \"\" in the derivation, rebuild to get the new hash, then update it.".to_string()),
            });
        }
    }

    // Deduplicate by category + what
    errors.dedup_by(|a, b| a.category == b.category && a.what == b.what);
    errors
}

/// Parse a hash mismatch error.
fn parse_hash_mismatch(lines: &[&str], idx: usize) -> Option<ParsedError> {
    let mut expected = String::new();
    let mut got = String::new();
    let mut pkg_name = String::new();

    // Look around the error line for expected/got values
    let start = idx.saturating_sub(5);
    let end = (idx + 10).min(lines.len());

    for line in &lines[start..end] {
        if line.contains("expected:") || line.contains("specified:") {
            if let Some(hash) = extract_hash(line) {
                expected = hash;
            }
        }
        if line.contains("got:") {
            if let Some(hash) = extract_hash(line) {
                got = hash;
            }
        }
        // Try to extract package name from store path
        if line.contains("/nix/store/") && line.contains(".drv") {
            if let Some(name) = extract_pkg_from_drv(line) {
                pkg_name = name;
            }
        }
    }

    if pkg_name.is_empty() {
        pkg_name = "unknown package".to_string();
    }

    let message = if !expected.is_empty() && !got.is_empty() {
        format!("Expected: {}\n      Got:      {}", expected, got)
    } else {
        "The downloaded source hash does not match the expected hash.".to_string()
    };

    Some(ParsedError {
        what: pkg_name,
        category: "Hash mismatch",
        message,
        hint: Some(format!(
            "Update the hash in your derivation. If using fetchFromGitHub, the upstream source may have changed."
        )),
    })
}

/// Parse an unfree package error.
fn parse_unfree(lines: &[&str], idx: usize) -> Option<ParsedError> {
    let line = lines[idx];
    // Try to extract package name
    let pkg_name = line.split('\'')
        .nth(1)
        .unwrap_or("unknown")
        .to_string();

    Some(ParsedError {
        what: pkg_name,
        category: "Unfree package",
        message: "This package has a non-free license and is blocked by default.".to_string(),
        hint: Some("Add 'nixpkgs.config.allowUnfree = true;' to your configuration.".to_string()),
    })
}

/// Parse a broken package error.
fn parse_broken(lines: &[&str], idx: usize) -> Option<ParsedError> {
    let line = lines[idx];
    let pkg_name = line.split('\'')
        .nth(1)
        .unwrap_or("unknown")
        .to_string();

    Some(ParsedError {
        what: pkg_name,
        category: "Broken package",
        message: "This package is marked as broken in nixpkgs and cannot be built.".to_string(),
        hint: Some("Remove it from your config, or override with 'meta.broken = false;' (at your own risk).".to_string()),
    })
}

/// Parse an undefined variable error.
fn parse_undefined_var(lines: &[&str], idx: usize) -> Option<ParsedError> {
    let line = lines[idx];
    // Extract variable name
    let var_name = line.split('\'')
        .nth(1)
        .unwrap_or("?")
        .to_string();

    // Look for file location
    let mut location = String::new();
    let start = idx.saturating_sub(3);
    for line in &lines[start..=idx] {
        if line.contains(".nix:") || line.contains("at /") {
            location = line.trim().to_string();
            break;
        }
    }

    let message = if location.is_empty() {
        format!("Variable '{}' is not defined.", var_name)
    } else {
        format!("Variable '{}' is not defined.\n      Location: {}", var_name, location)
    };

    Some(ParsedError {
        what: var_name,
        category: "Undefined variable",
        message,
        hint: Some("Check spelling, or add the variable to your function arguments.".to_string()),
    })
}

/// Parse a path-not-found error (usually missing git staging).
fn parse_path_not_found(lines: &[&str], idx: usize) -> Option<ParsedError> {
    let line = lines[idx];

    // Try to extract the path
    let path = line.split('\'')
        .find(|s| s.contains('/') || s.ends_with(".nix"))
        .unwrap_or("unknown file")
        .to_string();

    Some(ParsedError {
        what: path,
        category: "File not found",
        message: "A file referenced in the configuration does not exist.".to_string(),
        hint: Some("If this is a new file, stage it with 'git add <file>'. Flakes only see tracked files.".to_string()),
    })
}

/// Parse a builder failure error.
fn parse_builder_failed(lines: &[&str], idx: usize) -> Option<ParsedError> {
    let line = lines[idx];

    let pkg_name = extract_pkg_from_drv(line)
        .unwrap_or_else(|| "unknown".to_string());

    // Look for the last few log lines for context
    let mut log_lines = Vec::new();
    let start = idx.saturating_sub(10);
    for line in &lines[start..idx] {
        let trimmed = line.trim();
        if trimmed.starts_with('>') {
            log_lines.push(trimmed.trim_start_matches('>').trim().to_string());
        }
    }

    let message = if log_lines.is_empty() {
        "The package build process failed.".to_string()
    } else {
        let last_lines = log_lines.iter()
            .rev()
            .take(3)
            .rev()
            .map(|l| format!("      > {}", l))
            .collect::<Vec<_>>()
            .join("\n");
        format!("Build failed. Last log lines:\n{}", last_lines)
    };

    Some(ParsedError {
        what: pkg_name,
        category: "Build failure",
        message,
        hint: Some("Check the full build log with 'nix log /nix/store/<hash>.drv'.".to_string()),
    })
}

/// Parse a package collision error.
fn parse_collision(lines: &[&str], idx: usize) -> Option<ParsedError> {
    let line = lines[idx];
    let parts: Vec<&str> = line.split("/nix/store/").collect();

    let pkg1 = parts.get(1)
        .and_then(|p| p.split('/').next())
        .map(|s| s.chars().skip(33).collect::<String>())
        .unwrap_or_default();

    let pkg2 = parts.get(2)
        .and_then(|p| p.split('/').next())
        .map(|s| s.chars().skip(33).collect::<String>())
        .unwrap_or_default();

    Some(ParsedError {
        what: format!("{} vs {}", pkg1, pkg2),
        category: "Package collision",
        message: "Two packages provide the same file and conflict.".to_string(),
        hint: Some("Remove one of the conflicting packages, or use 'environment.systemPackages' with priority.".to_string()),
    })
}

/// Extract a hash value from a line containing "sha256-..." or "sha256:...".
fn extract_hash(line: &str) -> Option<String> {
    // Look for sha256-<base64> format
    if let Some(pos) = line.find("sha256-") {
        let hash = &line[pos..];
        let end = hash.find(|c: char| c.is_whitespace() || c == '\'' || c == '"')
            .unwrap_or(hash.len());
        return Some(hash[..end].to_string());
    }
    None
}

/// Extract a package name from a .drv store path.
fn extract_pkg_from_drv(line: &str) -> Option<String> {
    let drv_start = line.find("/nix/store/")?;
    let drv_path = &line[drv_start..];
    let drv_end = drv_path.find(".drv").unwrap_or(drv_path.len());
    let store_name = &drv_path["/nix/store/".len()..drv_end];

    // Skip the 32-char hash + hyphen
    if store_name.len() > 33 {
        Some(store_name[33..].to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_hash() {
        assert_eq!(
            extract_hash("  got: sha256-abc123def456"),
            Some("sha256-abc123def456".to_string())
        );
        assert_eq!(
            extract_hash("no hash here"),
            None
        );
    }

    #[test]
    fn test_extract_pkg_from_drv() {
        assert_eq!(
            extract_pkg_from_drv("builder for '/nix/store/abc12345678901234567890123456789-vivaldi-7.9.drv' failed"),
            Some("vivaldi-7.9".to_string())
        );
    }

    #[test]
    fn test_parse_hash_mismatch() {
        let lines = vec![
            "error: hash mismatch in fixed-output derivation",
            "  specified: sha256-aaaa",
            "  got: sha256-bbbb",
        ];
        let errors = parse_errors(&lines.join("\n"));
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].category, "Hash mismatch");
    }

    #[test]
    fn test_parse_unfree() {
        let lines = vec![
            "error: Package 'nvidia-x11' is not free and refused to install.",
        ];
        let errors = parse_errors(&lines.join("\n"));
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].category, "Unfree package");
        assert_eq!(errors[0].what, "nvidia-x11");
    }

    #[test]
    fn test_parse_broken() {
        let lines = vec![
            "error: Package 'python3.11-some-pkg' is marked as broken.",
        ];
        let errors = parse_errors(&lines.join("\n"));
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].category, "Broken package");
    }

    #[test]
    fn test_parse_undefined_var() {
        let lines = vec![
            "at /home/mae/nixos-config/modules/dev/test.nix:5:3:",
            "error: undefined variable 'pkgss'",
        ];
        let errors = parse_errors(&lines.join("\n"));
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].category, "Undefined variable");
        assert_eq!(errors[0].what, "pkgss");
    }

    #[test]
    fn test_parse_infinite_recursion() {
        let lines = vec![
            "error: infinite recursion encountered",
        ];
        let errors = parse_errors(&lines.join("\n"));
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].category, "Infinite recursion");
    }

    #[test]
    fn test_parse_path_not_found() {
        let lines = vec![
            "error: path '/nix/store/abc-source/modules/test.nix' does not exist",
        ];
        let errors = parse_errors(&lines.join("\n"));
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].category, "File not found");
    }

    #[test]
    fn test_parse_cargo_hash() {
        let lines = vec![
            "ERROR: cargoHash or cargoSha256 is out of date",
        ];
        let errors = parse_errors(&lines.join("\n"));
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].category, "Hash mismatch");
    }

    #[test]
    fn test_no_errors() {
        let errors = parse_errors("everything is fine");
        assert!(errors.is_empty());
    }
}
