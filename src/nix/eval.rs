//! Thin wrapper around `nix eval --raw` for querying package versions.
//!
//! Used by the nix-native version lookup that replaces the Repology HTTP
//! client: given a flake input reference and an attribute path, returns the
//! `.version` string if the attribute exists and is a derivation.
//!
//! A package that has no `.version` is not an error — it's normal for some
//! attributes (e.g. shell environments, bare scripts). The caller decides
//! what to do with `None`.

use std::process::Command;

use anyhow::Result;
use tracing::debug;

use crate::nix::tools::tool_error;
use crate::nix::version_cache::VersionCache;

/// Query the `.version` attribute of a package in a flake input.
///
/// `input` is a flake reference such as `nixpkgs` or
/// `github:NixOS/nixpkgs/nixos-unstable`.
/// `attr` is the attribute path within the input, e.g. `legacyPackages.x86_64-linux.firefox`.
///
/// Returns:
/// - `Ok(Some(version))` — attribute exists and has a non-empty version string.
/// - `Ok(None)` — attribute missing, has no `.version`, or eval produced an
///   error expression. This is normal and logged at `debug` level only.
/// - `Err(_)` — `nix` itself is not installed/in PATH (surfaces an install
///   hint via `tool_error`), or a genuine I/O failure.
pub fn eval_version(input: &str, attr: &str) -> Result<Option<String>> {
    let flake_attr = format!("{}#{}.version", input, attr);

    let output = Command::new("nix")
        .args([
            "eval",
            "--raw",
            "--extra-experimental-features",
            "nix-command flakes",
            &flake_attr,
        ])
        .output()
        .map_err(|e| tool_error("nix", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        debug!(
            "nix eval --raw {}: exit {:?} — {}",
            flake_attr,
            output.status.code(),
            stderr.trim()
        );
        return Ok(None);
    }

    let raw = String::from_utf8_lossy(&output.stdout);
    let result = parse_eval_output(&raw);

    if result.is_none() {
        debug!(
            "nix eval --raw {}: success but output unparseable: {:?}",
            flake_attr,
            raw.as_ref()
        );
    }

    Ok(result)
}

/// Parse the raw stdout of `nix eval --raw` into a clean version string.
///
/// Rules applied in order:
/// 1. Trim leading/trailing whitespace (including `\n`).
/// 2. Strip exactly one layer of surrounding double-quotes (some Nix attrs
///    produce quoted strings even with `--raw` in edge cases; be defensive).
/// 3. If the result is empty, return `None`.
/// 4. If the result starts with `error:`, return `None` — this means nix
///    printed an error message to stdout instead of a clean value.
pub(crate) fn parse_eval_output(raw: &str) -> Option<String> {
    let trimmed = raw.trim();

    // Strip one layer of surrounding double-quotes if present.
    let unquoted = if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };

    if unquoted.is_empty() {
        return None;
    }

    if unquoted.starts_with("error:") {
        return None;
    }

    Some(unquoted.to_string())
}

/// Returns the version for `(input, rev, attr)`, consulting the cache first.
///
/// Cache hit → returns immediately without any subprocess.
/// Cache miss → calls [`eval_version`], stores the result on success, and
/// returns it.
///
/// The caller is responsible for calling `cache.save(path)` once the batch
/// of lookups is complete. We don't save per-call to avoid disk thrash.
#[allow(dead_code)]
pub fn lookup_or_eval(
    cache: &mut VersionCache,
    input: &str,
    rev: &str,
    attr: &str,
) -> Result<Option<String>> {
    if let Some(v) = cache.lookup(input, rev, attr) {
        return Ok(Some(v));
    }
    let evaluated = eval_version(input, attr)?;
    if let Some(ref v) = evaluated {
        cache.store(input, rev, attr, v);
    }
    Ok(evaluated)
}

#[cfg(test)]
#[path = "tests/eval.rs"]
mod tests;
