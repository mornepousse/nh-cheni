//! `cheni build` command.
//!
//! Wraps `nh os switch` and parses Nix build errors into
//! human-readable messages with hints for fixing them.

use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::debug;

use crate::nix::config;

/// A parsed build error with a human-readable explanation.
#[derive(Debug)]
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

/// Run `cheni build`.
///
/// Wraps `nh os switch` and, if it fails, parses the error output
/// to provide human-readable error messages and fix suggestions.
pub fn run() -> Result<()> {
    let nix_config = config::detect()?;
    let config_path = nix_config.flake_dir.to_str()
        .context("Config path is not valid UTF-8")?;

    println!(
        "{}\n",
        "=== cheni build ===".bold()
    );

    // nh sends everything (progress + errors) to stderr.
    // We capture stderr while letting it pass through to the terminal via tee,
    // so the user sees progress AND we can parse errors.
    println!("{}", "Building...".dimmed());

    use std::io::{BufRead, BufReader};

    let mut child = Command::new("nh")
        .args(["os", "switch", config_path])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| crate::nix::tools::tool_error("nh", e))?;

    // Read stderr in real time: display to user AND capture for parsing.
    // stderr was piped above, so this is always Some — expect() makes the
    // invariant explicit for any future reader.
    let stderr_pipe = child
        .stderr
        .take()
        .expect("stderr was set to piped, must be Some");
    let reader = BufReader::new(stderr_pipe);
    let mut captured_stderr = String::new();

    // Stream line-by-line so the user sees nh output as it happens
    // (long rebuilds would otherwise print nothing until completion).
    // On a read/UTF-8 error we log + skip the line instead of breaking,
    // so a single bad byte in an embedded build log doesn't silently
    // truncate the captured stderr and break the error parser below.
    for line in reader.lines() {
        match line {
            Ok(line) => {
                eprintln!("{}", line);
                captured_stderr.push_str(&line);
                captured_stderr.push('\n');
            }
            Err(e) => {
                tracing::debug!("skipped unreadable stderr line: {}", e);
                continue;
            }
        }
    }

    let status = child.wait()
        .context("Failed to wait for build process")?;

    if status.success() {
        println!("\n{} Build successful!", "✓".green());
        return Ok(());
    }

    let stderr = &captured_stderr;
    debug!("Parsing {} bytes of captured stderr", stderr.len());

    let errors = parse_errors(stderr);

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

/// Strip ANSI escape codes from a string.
fn strip_ansi(s: &str) -> String {
    // Compiled once at first use, reused for every line of build output
    // on the hot path — recompiling per call showed up in profiles.
    static ANSI_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\x1b\[[0-9;]*m").expect("valid regex"));
    ANSI_RE.replace_all(s, "").to_string()
}

/// Find file location near an error line (looks both before and after).
fn find_location(lines: &[&str], idx: usize) -> Option<String> {
    let start = idx.saturating_sub(5);
    let end = (idx + 5).min(lines.len());

    for line in &lines[start..end] {
        let clean = strip_ansi(line);
        // Match patterns like "at /path/to/file.nix:16:5"
        if let Some(pos) = clean.find("at /") {
            let path_part = &clean[pos + 3..];
            // Extract just the path:line:col
            let end_pos = path_part.find(|c: char| c.is_whitespace() || c == ':')
                .and_then(|first_colon| {
                    // Keep path + line + col (two colons after path)
                    let after_first = &path_part[first_colon + 1..];
                    after_first.find(':').map(|second_colon| {
                        let after_second = &after_first[second_colon + 1..];
                        let col_end = after_second.find(|c: char| !c.is_ascii_digit())
                            .unwrap_or(after_second.len());
                        first_colon + 1 + second_colon + 1 + col_end
                    })
                })
                .unwrap_or(path_part.len());
            return Some(path_part[..end_pos].trim().to_string());
        }
        // Match "from `/path/to/file.nix':" or similar
        if clean.contains("definitions from") && clean.contains(".nix") {
            if let Some(start) = clean.find('`') {
                if let Some(end) = clean[start + 1..].find('\'') {
                    let path = &clean[start + 1..start + 1 + end];
                    if path.contains(".nix") {
                        return Some(path.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Find source context lines near an error (lines starting with line numbers).
fn find_source_context(lines: &[&str], idx: usize) -> Option<String> {
    let start = idx.saturating_sub(2);
    let end = (idx + 8).min(lines.len());
    let mut context_lines = Vec::new();

    for line in &lines[start..end] {
        let clean = strip_ansi(line);
        let trimmed = clean.trim().trim_start_matches('┃').trim();
        // Match source lines like "15|     libreoffice-fresh"
        if trimmed.contains('|') {
            let parts: Vec<&str> = trimmed.splitn(2, '|').collect();
            if parts.len() == 2 && parts[0].trim().chars().all(|c| c.is_ascii_digit()) {
                context_lines.push(format!("      {}", trimmed));
            }
        }
    }

    if context_lines.is_empty() {
        None
    } else {
        Some(context_lines.join("\n"))
    }
}

/// Parse Nix build errors from stderr output.
fn parse_errors(stderr: &str) -> Vec<ParsedError> {
    let mut errors = Vec::new();

    // Strip ANSI codes for pattern matching
    let clean_stderr = strip_ansi(stderr);

    // Process line by line, looking for error patterns
    let lines: Vec<&str> = clean_stderr.lines().collect();

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

        // Pattern 10: "not supported for interpreter" (Python version mismatch)
        if line.contains("not supported for interpreter") {
            // Extract the actual error message (strip prefixes like "error:", "┃", etc.)
            let clean_msg = line.trim()
                .trim_start_matches("error:")
                .trim_start_matches('┃')
                .trim();
            // Extract package name: "sphinx-9.1.0 not supported..." → "sphinx-9.1.0"
            let pkg_name = clean_msg.split_whitespace().next().unwrap_or("?");
            // Only add once (skip if we already have this exact package)
            if !errors.iter().any(|e| e.category == "Incompatible package" && e.what == pkg_name) {
                errors.push(ParsedError {
                    what: pkg_name.to_string(),
                    category: "Incompatible package",
                    message: clean_msg.to_string(),
                    hint: Some("Use a different Python version, or remove this package.".to_string()),
                });
            }
        }

        // Pattern 11: Generic "error:" line not caught by other patterns
        // (fallback — only if no other errors were found yet for this line)
        if line.trim().starts_with("error:") && errors.is_empty() {
            let msg = line.trim().strip_prefix("error:").unwrap_or(line).trim();
            // Skip if it's just "error:" with nothing useful
            if !msg.is_empty() && msg.len() > 5 {
                let location = find_location(&lines, i);
                let context = find_source_context(&lines, i);
                let mut full_msg = msg.to_string();
                if let Some(loc) = &location {
                    full_msg.push_str(&format!("\n      File: {}", humanize_path(loc)));
                }
                if let Some(ctx) = &context {
                    full_msg.push_str(&format!("\n{}", ctx));
                }
                errors.push(ParsedError {
                    what: "Nix evaluation".to_string(),
                    category: "Eval error",
                    message: full_msg,
                    hint: None,
                });
            }
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
        hint: Some("Update the hash in your derivation. If using fetchFromGitHub, the upstream source may have changed.".to_string()),
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

    // Find file location and source context
    let location = find_location(lines, idx);
    let context = find_source_context(lines, idx);

    let mut message = format!("Variable '{}' is not defined.", var_name);
    if let Some(loc) = &location {
        message.push_str(&format!("\n      File: {}", humanize_path(loc)));
    }
    if let Some(ctx) = &context {
        message.push_str(&format!("\n{}", ctx));
    }

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

    let display_path = humanize_path(&path);

    Some(ParsedError {
        what: display_path,
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

/// Simplify a nix store path to a human-readable relative path.
///
/// Converts `/nix/store/abc123-source/modules/dev/test.nix:5:3`
/// to `modules/dev/test.nix:5:3`.
fn humanize_path(path: &str) -> String {
    // Strip /nix/store/<hash>-source/ prefix
    if let Some(pos) = path.find("-source/") {
        return path[pos + 8..].to_string();
    }
    // Strip /nix/store/<hash>-<name>/ prefix
    if path.starts_with("/nix/store/") && path.len() > 44 {
        return path[44..].to_string();
    }
    path.to_string()
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
    fn test_parse_cargohash_out_of_date() {
        // Real nh output when a Cargo dep is added without bumping cargoHash.
        // This one was the reason cheni itself failed to self-update once.
        let stderr = "\
cheni> ERROR: cargoHash or cargoSha256 is out of date
cheni> Cargo.lock is not the same in /build/cheni-0.1.0-vendor";
        let errors = parse_errors(stderr);
        assert!(
            errors.iter().any(|e| e.category == "Hash mismatch"),
            "expected Hash mismatch, got {:?}",
            errors
        );
    }

    #[test]
    fn test_parse_python_interpreter_mismatch() {
        let stderr = "error: sphinx-9.1.0 not supported for interpreter python3.13";
        let errors = parse_errors(stderr);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].category, "Incompatible package");
        assert_eq!(errors[0].what, "sphinx-9.1.0");
    }

    #[test]
    fn test_parse_python_mismatch_deduplicates() {
        // nh can repeat the same error across several builder retries —
        // the parser should only emit one entry.
        let stderr = "\
error: sphinx-9.1.0 not supported for interpreter python3.13
... some noise ...
error: sphinx-9.1.0 not supported for interpreter python3.13";
        let errors = parse_errors(stderr);
        let dups = errors
            .iter()
            .filter(|e| e.category == "Incompatible package" && e.what == "sphinx-9.1.0")
            .count();
        assert_eq!(dups, 1, "expected dedup, got {:?}", errors);
    }

    #[test]
    fn test_parse_multiple_errors() {
        // A rebuild can surface several independent errors — we should
        // collect them all, not just the first.
        let stderr = "\
error: undefined variable 'pkgss'
at /file.nix:1:1
error: Package 'mesa' is marked as broken.";
        let errors = parse_errors(stderr);
        assert!(errors.len() >= 2, "expected >=2, got {:?}", errors);
    }

    #[test]
    fn test_parse_empty_stderr() {
        // Truly empty stderr → no errors. Must not panic or return bogus
        // "error: " entries from the generic fallback.
        let errors = parse_errors("");
        assert!(errors.is_empty());
    }

    #[test]
    fn test_parse_generic_error_fallback() {
        // An error pattern we don't specifically recognise still shows up
        // as a generic entry so the user sees *something*, not silence.
        let errors = parse_errors("error: some novel upstream message we don't know yet");
        assert_eq!(errors.len(), 1);
        assert!(errors[0].message.contains("some novel"));
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
