//! Thin wrapper around the fetchTree-based nix eval for querying package
//! versions.
//!
//! Used by the nix-native version lookup: given the locked `rev` and
//! `narHash` of a nixpkgs-like flake input, evaluates `pkgs.<name>.version`
//! against that content-addressed tree.
//!
//! The previous `nix eval <input>#<attr>` approach required the input to be
//! globally registered in the flake registry, which `nixpkgs-latest` is not.
//! The `fetchTree`-based approach delegates to
//! `flake::query_pkg_version_at_rev`, which is already used by `cheni freeze`
//! and handles input validation + pure eval without needing a registry.
//!
//! A package that has no `.version` is not an error — it's normal for some
//! attributes (e.g. shell environments, bare scripts). The caller decides
//! what to do with `None`.

use anyhow::Result;

use crate::nix::version_cache::VersionCache;

/// Query the `.version` attribute of a package in a nixpkgs tree pinned at
/// `rev` + `nar_hash`.
///
/// Delegates to [`crate::nix::flake::query_pkg_version_at_rev`] which uses
/// `builtins.fetchTree { type = "github"; owner = "NixOS"; repo = "nixpkgs";
/// rev; narHash; }` — pure, content-addressed, no flake registry needed.
///
/// Returns:
/// - `Ok(Some(version))` — attribute exists and has a non-empty version string.
/// - `Ok(None)` — attribute missing, has no `.version`, eval failed, or
///   `rev`/`nar_hash` failed validation. Logged at `debug` level only.
pub fn eval_version(rev: &str, nar_hash: &str, pkg_name: &str) -> Result<Option<String>> {
    Ok(crate::nix::flake::query_pkg_version_at_rev(rev, nar_hash, pkg_name))
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
///
/// Only used in tests. Kept as a testable unit so the parsing rules stay
/// independently verifiable without shelling out to `nix`.
#[cfg_attr(not(test), allow(dead_code))]
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

/// Returns the version for `(input_name, rev, pkg_name)`, consulting the
/// cache first.
///
/// Cache hit → returns immediately without any subprocess.
/// Cache miss → calls [`eval_version`] (which uses the fetchTree-based eval),
/// stores the result on success, and returns it.
///
/// The caller is responsible for calling `cache.save(path)` once the batch
/// of lookups is complete. We don't save per-call to avoid disk thrash.
///
/// The cache key uses `pkg_name` (e.g. `"firefox"`) rather than the full
/// attr path (`"legacyPackages.x86_64-linux.firefox"`) because the
/// `fetchTree`-based eval constructs the full path internally and callers
/// only have the short name available.
pub fn lookup_or_eval(
    cache: &mut VersionCache,
    input_name: &str,
    rev: &str,
    nar_hash: &str,
    pkg_name: &str,
) -> Result<Option<String>> {
    if let Some(v) = cache.lookup(input_name, rev, pkg_name) {
        return Ok(Some(v));
    }
    let evaluated = eval_version(rev, nar_hash, pkg_name)?;
    if let Some(ref v) = evaluated {
        cache.store(input_name, rev, pkg_name, v);
    }
    Ok(evaluated)
}

#[cfg(test)]
#[path = "tests/eval.rs"]
mod tests;
