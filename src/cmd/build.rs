//! `cheni build` command.
//!
//! Wraps `nh os switch` and parses Nix build errors into
//! human-readable messages with hints for fixing them.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::debug;

use crate::nix::{config, freezes, pins};

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
    let started = Instant::now();
    let nix_config = config::detect()?;
    let config_path = nix_config
        .flake_dir
        .to_str()
        .context("Config path is not valid UTF-8")?;

    println!("{}\n", "=== cheni build ===".bold());

    // Pre-flight: surface any active pin/freeze policy so the user knows
    // *what* is about to be applied before the rebuild kicks off. Silent
    // when there's nothing in either file — common case stays unchanged.
    print_active_policy(&nix_config.flake_dir);

    println!("{}", "Building...".dimmed());

    let (status, captured_stderr) = run_nh_capturing_stderr(config_path)?;

    if status.success() {
        println!(
            "\n{} Build successful in {}.",
            "✓".green().bold(),
            format_elapsed(started.elapsed()).dimmed()
        );
        return Ok(());
    }

    debug!("Parsing {} bytes of captured stderr", captured_stderr.len());
    let errors = parse_errors(&captured_stderr);

    if errors.is_empty() {
        // Couldn't parse the error — show raw output as a fallback.
        println!(
            "\n{} Build failed after {}.\n",
            "✗".red(),
            format_elapsed(started.elapsed()).dimmed()
        );
        eprintln!("{}", captured_stderr);
        return Ok(());
    }

    print_parsed_errors(&errors, started.elapsed());
    Ok(())
}

/// Render the active pin/freeze block before the rebuild, when either
/// list is non-empty. Names are listed up to a small cap with "(+N more)"
/// for longer lists — so a user with a single pin sees it without
/// scrolling, and a user with 50 pins doesn't drown the build header.
///
/// Reads are non-fatal: a corrupt pins.json shouldn't block a build.
/// We log at DEBUG and skip the section if either read fails.
fn print_active_policy(flake_dir: &Path) {
    let pins_list = match pins::read(flake_dir) {
        Ok(v) => v,
        Err(e) => {
            debug!("skipping pre-flight: pins read failed: {}", e);
            return;
        }
    };
    let freezes_map = match freezes::read(flake_dir) {
        Ok(m) => m,
        Err(e) => {
            debug!("skipping pre-flight: freezes read failed: {}", e);
            return;
        }
    };

    if pins_list.is_empty() && freezes_map.is_empty() {
        return;
    }

    println!("  {}", "Active policy:".bold());
    if !pins_list.is_empty() {
        println!(
            "    {} ({}): {}",
            "pinned".cyan(),
            pins_list.len(),
            cap_names(&pins_list, 5).dimmed(),
        );
    }
    if !freezes_map.is_empty() {
        let frozen: Vec<String> = freezes_map
            .iter()
            .map(|(name, entry)| {
                if entry.version.is_empty() {
                    name.clone()
                } else {
                    format!("{}@{}", name, entry.version)
                }
            })
            .collect();
        println!(
            "    {} ({}): {}",
            "frozen".cyan(),
            frozen.len(),
            cap_names(&frozen, 5).dimmed(),
        );
    }
    println!();
}

/// Join `items` with ", ", showing only the first `cap` and appending
/// "(+N more)" when truncated. Matches the convention used by the
/// `cheni history` annotation so the visual style stays consistent.
fn cap_names(items: &[String], cap: usize) -> String {
    if items.len() <= cap {
        return items.join(", ");
    }
    let head: Vec<&str> = items[..cap].iter().map(String::as_str).collect();
    format!("{} (+{} more)", head.join(", "), items.len() - cap)
}

/// Format `Duration` as `MmSs` or `Ss`. Matches the helper used by
/// `cheni upgrade` / `update` / `self-update`.
fn format_elapsed(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    }
}

/// Spawn `nh os switch <flake>`, stream stderr to the user line-by-line
/// (so they see progress on a long rebuild), and capture every line for
/// the error parser. UTF-8 read errors on stderr are logged at DEBUG and
/// skipped — losing one weird line is better than truncating the rest.
fn run_nh_capturing_stderr(
    config_path: &str,
) -> Result<(std::process::ExitStatus, String)> {
    use std::io::{BufRead, BufReader};

    let mut child = Command::new("nh")
        .args(["os", "switch", config_path])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| crate::nix::tools::tool_error("nh", e))?;

    // stderr was piped above, so .take() is guaranteed to return Some.
    let stderr_pipe = child
        .stderr
        .take()
        .expect("stderr was set to piped, must be Some");
    let reader = BufReader::new(stderr_pipe);
    let mut captured = String::new();

    for line in reader.lines() {
        match line {
            Ok(line) => {
                // Prettify for display (strip /nix/store/<hash>-) but
                // capture the raw line — the structured parser below
                // uses the raw store paths to extract package names.
                eprintln!("{}", crate::output::prettify::prettify_line(&line));
                captured.push_str(&line);
                captured.push('\n');
            }
            Err(e) => {
                tracing::debug!("skipped unreadable stderr line: {}", e);
                continue;
            }
        }
    }

    let status = child
        .wait()
        .context("Failed to wait for build process")?;
    Ok((status, captured))
}

/// Render the parsed error list as the human-readable failure summary.
fn print_parsed_errors(errors: &[ParsedError], elapsed: std::time::Duration) {
    println!(
        "\n{} Build failed after {} with {} error(s):\n",
        "✗".red(),
        format_elapsed(elapsed).dimmed(),
        errors.len()
    );
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

/// One row of the dispatch table used by `parse_errors`. `matches`
/// is a quick substring test on the current line; `handle` extracts
/// a structured ParsedError when the match is positive.
struct ErrorPattern {
    matches: fn(&str) -> bool,
    handle: fn(&[&str], usize) -> Option<ParsedError>,
}

/// Specific patterns we recognise, in priority order. Each one is
/// independent — a single nh stderr line can trigger several patterns
/// (e.g. cargoHash + a generic eval error around it).
const ERROR_PATTERNS: &[ErrorPattern] = &[
    ErrorPattern {
        // Activation-time refusal from `switch-to-configuration` /
        // `nh os switch`. The build itself succeeded — what failed is
        // a pre-flight that detected a critical-component change
        // (dbus → dbus-broker, sysvinit → systemd, pulseaudio →
        // pipewire, …) that can't safely happen at runtime. The new
        // generation is on disk and rollback-able; only the live
        // switch was blocked.
        matches: |l| {
            (l.contains("Pre-switch check") && l.contains("failed"))
                || l.contains("Pre-switch checks failed")
                || l.contains("Switching into this system is not recommended")
        },
        handle: parse_switch_inhibitor,
    },
    ErrorPattern {
        matches: |l| l.contains("hash mismatch") || l.contains("sha256 mismatch"),
        handle: parse_hash_mismatch,
    },
    ErrorPattern {
        matches: |l| l.contains("is not free") || (l.contains("unfree") && l.contains("refused")),
        handle: parse_unfree,
    },
    ErrorPattern {
        matches: |l| l.contains("is marked as broken"),
        handle: parse_broken,
    },
    ErrorPattern {
        // Nix prints: `error: Refusing to evaluate package 'PKG' in …
        // because it is marked as insecure`. `permittedInsecure` is an
        // extra safety net in case nixpkgs rewords the sentence but
        // keeps the knob name.
        matches: |l| {
            l.contains("is marked as insecure")
                || l.contains("marked as insecure")
                || (l.contains("Refusing to evaluate") && l.contains("insecure"))
                || l.contains("permittedInsecurePackages")
        },
        handle: parse_insecure,
    },
    ErrorPattern {
        matches: |l| l.contains("undefined variable"),
        handle: parse_undefined_var,
    },
    ErrorPattern {
        matches: |l| l.contains("infinite recursion"),
        handle: parse_infinite_recursion,
    },
    ErrorPattern {
        matches: |l| l.contains("path") && l.contains("does not exist"),
        handle: parse_path_not_found,
    },
    ErrorPattern {
        matches: |l| l.contains("builder for") && l.contains("failed"),
        handle: parse_builder_failed,
    },
    ErrorPattern {
        matches: |l| {
            l.contains("collision between")
                || (l.contains("collides with") && l.contains("nix/store"))
        },
        handle: parse_collision,
    },
    ErrorPattern {
        matches: |l| l.contains("cargoHash") && l.contains("out of date"),
        handle: parse_cargo_hash,
    },
    ErrorPattern {
        matches: |l| l.contains("not supported for interpreter"),
        handle: parse_python_interpreter,
    },
];

/// Parse Nix build errors from stderr output.
///
/// Two-pass strategy:
///   1. Scan every line against the specific ERROR_PATTERNS table.
///   2. If nothing matched, fall back to a single "generic eval error"
///      grabbed from the first `error:` line — better than printing
///      a wall of raw nh output when the pattern is one we haven't
///      taught cheni about yet.
fn parse_errors(stderr: &str) -> Vec<ParsedError> {
    let clean_stderr = strip_ansi(stderr);
    let lines: Vec<&str> = clean_stderr.lines().collect();
    let mut errors = Vec::new();

    for (i, line) in lines.iter().enumerate() {
        for pattern in ERROR_PATTERNS {
            if (pattern.matches)(line) {
                if let Some(err) = (pattern.handle)(&lines, i) {
                    push_unique(&mut errors, err);
                }
            }
        }
    }

    if errors.is_empty() {
        for (i, line) in lines.iter().enumerate() {
            if let Some(err) = parse_generic_error(&lines, i, line) {
                errors.push(err);
                break; // one fallback is enough
            }
        }
    }

    errors.dedup_by(|a, b| a.category == b.category && a.what == b.what);
    errors
}

/// Push an error into the list only if no entry with the same
/// (category, what) already exists. Several patterns can fire on the
/// same root cause (e.g. one line of "Python 3.13 not supported for
/// sphinx" repeated across nh's retry output).
fn push_unique(errors: &mut Vec<ParsedError>, err: ParsedError) {
    if !errors
        .iter()
        .any(|e| e.category == err.category && e.what == err.what)
    {
        errors.push(err);
    }
}

fn parse_infinite_recursion(_lines: &[&str], _idx: usize) -> Option<ParsedError> {
    Some(ParsedError {
        what: "Nix evaluation".to_string(),
        category: "Infinite recursion",
        message: "The configuration caused an infinite recursion during evaluation."
            .to_string(),
        hint: Some(
            "Check overlays for circular references. Use '--show-trace' for details."
                .to_string(),
        ),
    })
}

fn parse_cargo_hash(_lines: &[&str], _idx: usize) -> Option<ParsedError> {
    Some(ParsedError {
        what: "Cargo hash".to_string(),
        category: "Hash mismatch",
        message: "Cargo.lock changed but cargoHash in the derivation is outdated."
            .to_string(),
        hint: Some(
            "Set cargoHash = \"\" in the derivation, rebuild to get the new hash, then update it."
                .to_string(),
        ),
    })
}

fn parse_python_interpreter(lines: &[&str], idx: usize) -> Option<ParsedError> {
    let line = lines.get(idx)?;
    let clean_msg = line
        .trim()
        .trim_start_matches("error:")
        .trim_start_matches('┃')
        .trim();
    let pkg_name = clean_msg.split_whitespace().next().unwrap_or("?");
    Some(ParsedError {
        what: pkg_name.to_string(),
        category: "Incompatible package",
        message: clean_msg.to_string(),
        hint: Some("Use a different Python version, or remove this package.".to_string()),
    })
}

/// Last-resort matcher: anything starting with `error:` that the
/// specific patterns missed. We prefer a generic message with the
/// nearby file location to a raw stderr dump.
fn parse_generic_error(lines: &[&str], idx: usize, line: &str) -> Option<ParsedError> {
    let trimmed = line.trim();
    if !trimmed.starts_with("error:") {
        return None;
    }
    let msg = trimmed.strip_prefix("error:").unwrap_or(line).trim();
    if msg.is_empty() || msg.len() <= 5 {
        return None;
    }
    let mut full_msg = msg.to_string();
    if let Some(loc) = find_location(lines, idx) {
        full_msg.push_str(&format!("\n      File: {}", humanize_path(&loc)));
    }
    if let Some(ctx) = find_source_context(lines, idx) {
        full_msg.push_str(&format!("\n{}", ctx));
    }
    Some(ParsedError {
        what: "Nix evaluation".to_string(),
        category: "Eval error",
        message: full_msg,
        hint: None,
    })
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

/// Parse a `Pre-switch check '<name>' failed` error.
///
/// nh / nixos-rebuild runs activation pre-flights that refuse to
/// switch the *running* system when a critical component change is
/// detected — typically `dbus-implementation` (dbus → dbus-broker),
/// `init` (sysvinit → systemd), or sound stack swaps. The build is
/// already a success at this point: the new derivation is on disk,
/// addressable as a generation, and the bootloader can pick it up.
/// The error is purely about live switching being unsafe.
///
/// We extract the actual change (`<name> : <old> -> <new>`) when
/// present so the user sees exactly which critical bit moved, plus
/// the canonical "use `nh os boot` + reboot" recovery path.
fn parse_switch_inhibitor(lines: &[&str], idx: usize) -> Option<ParsedError> {
    // Scan a wider window — the detected-change line is typically
    // 3–8 lines before the `Pre-switch check ... failed` summary.
    let start = idx.saturating_sub(20);
    let end = (idx + 5).min(lines.len());
    let change = lines[start..end].iter().find_map(|l| {
        let clean = strip_ansi(l);
        let trimmed = clean.trim();
        // Activation prints lines like `dbus-implementation : dbus -> broker`.
        // The double `:` + ` -> ` separator is distinctive enough to
        // avoid grabbing arbitrary log output.
        if trimmed.contains(" : ") && trimmed.contains(" -> ") {
            Some(trimmed.to_string())
        } else {
            None
        }
    });

    let what = change
        .clone()
        .unwrap_or_else(|| "critical component change".to_string());
    let mut message = String::from(
        "The build succeeded — the new generation is on disk and rollback-able \
         from grub — but nixos-rebuild refused the live switch because a \
         critical system component is changing. Live-switching it would \
         likely break running services (dbus crashes, audio loss, …).",
    );
    if let Some(ch) = &change {
        message.push_str(&format!("\n      Detected: {}", ch));
    }

    Some(ParsedError {
        what,
        category: "Pre-switch check",
        message,
        hint: Some(
            "Stage the new generation for next boot, then reboot:\n      \
             sudo nh os boot ~/nixos-config\n      \
             sudo reboot\n      \
             Your previous generation stays bootable as a rollback target."
                .to_string(),
        ),
    })
}

/// Parse a `marked as insecure` error.
///
/// Nix phrasing: `error: Refusing to evaluate package 'PKG-X.Y.Z' in
/// … because it is marked as insecure`. nixpkgs attaches this marker
/// to packages with known unpatched CVEs or upstream-unmaintained
/// codebases. The user can either allow the specific pin or drop the
/// dependency — both paths are shown in the hint so the answer is
/// actionable without a second trip to the nixpkgs docs.
fn parse_insecure(lines: &[&str], idx: usize) -> Option<ParsedError> {
    // Scan a small window around the match — the "marked as insecure"
    // line may be the error body while the package name is on the
    // `Refusing to evaluate package 'PKG'` line a few lines earlier.
    let start = idx.saturating_sub(8);
    let end = (idx + 3).min(lines.len());
    let pkg_name = lines[start..end]
        .iter()
        .find_map(|l| {
            if !l.contains("Refusing to evaluate package") {
                return None;
            }
            l.split('\'').nth(1).map(|s| s.to_string())
        })
        .or_else(|| {
            // Fallback: pull a quoted token out of the matching line itself.
            lines[idx].split('\'').nth(1).map(|s| s.to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());

    Some(ParsedError {
        what: pkg_name.clone(),
        category: "Insecure package",
        message:
            "Marked as insecure in nixpkgs (known CVEs or upstream unmaintained). \
             Refused by default."
                .to_string(),
        hint: Some(format!(
            "Allow it explicitly with:\n      \
             {}\n      \
             Or drop the dependency (preferable if the package is unmaintained).",
            format_args!(
                "nixpkgs.config.permittedInsecurePackages = [ \"{}\" ];",
                pkg_name
            )
        )),
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
#[path = "tests/build.rs"]
mod tests;
