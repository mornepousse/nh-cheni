//! Friendly error messages for missing external tools.
//!
//! cheni shells out to several binaries (`nh`, `nix`, `nvd`, `git`, `sudo`).
//! When one of them isn't in PATH, the OS returns a generic ENOENT that
//! translates to "No such file or directory" at the call site — which
//! doesn't tell a new user what to do.
//!
//! `tool_error` converts an `io::Error` from a `Command::spawn`/`output`
//! into a targeted `anyhow::Error` with an install hint for the well-known
//! tools, and falls back to a generic message for anything else.

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
        other => anyhow!(
            "'{}' is not installed or not in PATH.",
            other
        ),
    }
}
