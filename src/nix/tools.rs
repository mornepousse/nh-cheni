//! Friendly error messages for missing external tools.
//!
//! cheni shells out to several binaries (`nh`, `nix`, `nvd`, `git`, `sudo`,
//! `du`, `getconf`).
//! When one of them isn't in PATH, the OS returns a generic ENOENT that
//! translates to "No such file or directory" at the call site — which
//! doesn't tell a new user what to do.
//!
//! `tool_error` converts an `io::Error` from a `Command::spawn`/`output`
//! into a targeted `anyhow::Error` with an install hint for the well-known
//! tools, and falls back to a generic message for anything else.
//!
//! Higher-level helpers (`du_size`, `getconf_clk_tck`, `nh_version`) execute
//! the tool and return structured results so callers don't need to
//! re-implement parse + error-mapping logic.

use anyhow::{anyhow, Error};

/// Convert a failed Command execution error into a user-actionable message.
///
/// Use at call sites like:
/// ```ignore
/// Command::new("nh")
///     .args(["os", "switch"])
///     .status()
///     .map_err(|e| tool_error("nh", e))?;
/// ```
pub fn tool_error(program: &str, err: std::io::Error) -> Error {
    if err.kind() == std::io::ErrorKind::NotFound {
        return missing_tool_hint(program);
    }
    Error::new(err).context(format!("Failed to run '{}'", program))
}

/// Build the "tool is missing, here's how to install it" message.
/// Kept as a standalone function so `cheni doctor` can surface the same
/// wording without having to trigger an actual ENOENT first.
pub fn missing_tool_hint(program: &str) -> Error {
    match program {
        "nh" => anyhow!(
            "'nh' is not installed.\n  \
             cheni uses nh (nixos-helper) to wrap nixos-rebuild and parse errors.\n\n  \
             Add it to your NixOS config:\n    \
                 environment.systemPackages = [ pkgs.nh ];\n\n  \
             Then rebuild your system and try again."
        ),
        "nvd" => anyhow!(
            "'nvd' is not installed.\n  \
             nvd produces the prettier per-package diff used by 'cheni diff' and\n  \
             'cheni history --diff'. It's optional — cheni falls back to\n  \
             'nix store diff-closures' without it.\n\n  \
             To install:  environment.systemPackages = [ pkgs.nvd ];"
        ),
        "nix" | "nix-store" | "nix-env" => anyhow!(
            "'{}' is not installed or not in PATH.\n  \
             cheni runs on NixOS only. If you're on NixOS, check that\n  \
             /run/current-system/sw/bin is in your PATH (this is usually\n  \
             the case automatically, but rescue shells or container environments\n  \
             may ship a stripped-down PATH).",
            program
        ),
        "git" => anyhow!(
            "'git' is not installed.\n  \
             cheni uses git to detect uncommitted changes in the flake.\n\n  \
             Add it to your NixOS config:\n    \
                 environment.systemPackages = [ pkgs.git ];"
        ),
        "sudo" => anyhow!(
            "'sudo' is not installed or not in PATH.\n  \
             Privileged operations (rebuild, rollback, generation deletion)\n  \
             need sudo."
        ),
        "du" => anyhow!(
            "'du' is not installed or not in PATH.\n  \
             cheni uses du to estimate the Nix store size in 'cheni doctor'.\n\n  \
             On NixOS, du ships with GNU coreutils:\n    \
                 environment.systemPackages = [ pkgs.coreutils ];"
        ),
        "getconf" => anyhow!(
            "'getconf' is not installed or not in PATH.\n  \
             cheni uses getconf to read CLK_TCK (clock ticks per second) for\n  \
             process uptime display in 'cheni doctor'.\n\n  \
             On NixOS, getconf ships with glibc:\n    \
                 environment.systemPackages = [ pkgs.glibc ];"
        ),
        other => anyhow!(
            "'{}' is not installed or not in PATH.",
            other
        ),
    }
}

/// Run `du -sh <path>` and return the parsed size string (e.g. `"42G"`).
///
/// On ENOENT or non-zero exit, returns an `Err` with a `tool_error`-style
/// message so the caller can surface it consistently.
pub fn du_size(path: &std::path::Path) -> Result<String, anyhow::Error> {
    let output = std::process::Command::new("du")
        .args(["-sh", path.to_string_lossy().as_ref()])
        .output()
        .map_err(|e| tool_error("du", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let first_line = stderr.lines().next().unwrap_or("").trim();
        let detail = if first_line.is_empty() {
            match output.status.code() {
                Some(c) => format!("du exited with code {}", c),
                None => "du terminated without exit code".to_string(),
            }
        } else {
            first_line.strip_prefix("du: ").unwrap_or(first_line).to_string()
        };
        anyhow::bail!("{}", detail);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let size = stdout
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow::anyhow!("du returned empty output"))?
        .to_string();
    if size.is_empty() || size == "?" {
        anyhow::bail!("du returned an unparseable size");
    }
    Ok(size)
}

/// Run `getconf CLK_TCK` and return the value, or `None` if the tool is
/// absent, returns non-zero, or produces non-numeric output.
///
/// The caller is expected to fall back to a safe default (100) when this
/// returns `None`.
pub fn getconf_clk_tck() -> Option<u64> {
    let output = std::process::Command::new("getconf")
        .arg("CLK_TCK")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = std::str::from_utf8(&output.stdout).ok()?;
    let n = s.trim().parse::<u64>().ok()?;
    if n > 0 { Some(n) } else { None }
}

/// Run `nh --version` and return the trimmed version string, or `None` if
/// the tool is absent or returns non-zero.
pub fn nh_version() -> Option<String> {
    let output = std::process::Command::new("nh")
        .arg("--version")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}
