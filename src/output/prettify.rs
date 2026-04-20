//! Strip Nix-toolchain noise out of individual log lines.
//!
//! Only transformation today: remove the `/nix/store/<32-char-hash>-`
//! prefix from every store path in a line, leaving the human-meaningful
//! tail (`name-version.drv`, `name/lib/...`). The raw text is preserved
//! in any captured buffers — this helper exists purely for display.

use std::sync::LazyLock;

use regex::Regex;

/// Matches the fixed-length hash prefix of a Nix store path.
///
/// Nix hashes are 32 characters from a custom base32 alphabet
/// (0–9 a–z minus `e`, `o`, `u`, `t`). `[a-z0-9]{32}` matches a
/// superset, which is fine for a display-only transform — the only
/// risk is false positives, and a 32-char lowercase alphanumeric run
/// immediately after `/nix/store/` is effectively always a store hash.
static STORE_HASH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/nix/store/[a-z0-9]{32}-").expect("valid regex"));

/// Return a copy of `line` with every `/nix/store/<hash>-` prefix removed.
///
/// Idempotent: lines that don't contain a store path pass through
/// unchanged, and the function never panics.
pub fn prettify_line(line: &str) -> String {
    STORE_HASH_RE.replace_all(line, "").to_string()
}

#[cfg(test)]
#[path = "tests/prettify.rs"]
mod tests;
