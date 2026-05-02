//! Input validation shared across cheni-spec modules.
//!
//! Centralised package-name validation: the same rule was implemented
//! twice (in pins.rs and freezes.rs) and a third, slightly stricter
//! variant existed inline in check.rs as defence-in-depth before
//! splicing into a Nix expression. Lifted here so a stricter pass
//! propagates to all callers.

use color_eyre::eyre::{Result, bail};

/// Reject obviously bogus package names before they reach the JSON
/// state files or a `pkgs.${name}` Nix attribute lookup.
///
/// Accepts: ASCII letters / digits / `-` / `_` / `.` / `+`.
/// Rejects: empty, > 128 chars, control chars, slashes, quotes,
/// backslashes.
///
/// # Errors
///
/// Returns an error with a user-facing message describing why the
/// name was rejected.
pub fn package_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("Package name is empty");
    }
    if name.len() > 128 {
        bail!(
            "Package name '{}…' is suspiciously long ({} chars, max 128)",
            &name.chars().take(20).collect::<String>(),
            name.len()
        );
    }
    if let Some(bad) = name.chars().find(|c| {
        c.is_control() || matches!(*c, '\n' | '\r' | '/' | '\\' | '"' | '\'')
    }) {
        bail!(
            "Package name '{}' contains an invalid character ({:?}). \
             Nix package names use letters, digits, '-', '_', '.', '+'.",
            name,
            bad
        );
    }
    Ok(())
}

/// Stricter variant of [`package_name`] used at the `nix eval --expr`
/// splice site: rejects everything that isn't `[A-Za-z0-9_.+-]` so
/// the value is safe to inject as a Nix attribute path. Also caps
/// at 128 chars and rejects empty.
///
/// `package_name` accepts what the on-disk JSON can hold;
/// `nix_attr_path` accepts what we're willing to splice into a Nix
/// expression. The two ARE NOT the same — at write time we may
/// permit characters that aren't safe at splice time. Always call
/// `nix_attr_path` AT the splice site, never trust the file.
///
/// # Errors
///
/// Returns an error if the attribute path fails the strict allowlist.
pub fn nix_attr_path(attr: &str) -> Result<()> {
    if attr.is_empty() {
        bail!("Nix attribute path is empty");
    }
    if attr.len() > 128 {
        bail!("Nix attribute path is too long ({} chars, max 128)", attr.len());
    }
    if !attr.chars().all(|c| {
        c.is_ascii_alphanumeric()
            || c == '-' || c == '_' || c == '.' || c == '+'
    }) {
        bail!(
            "Nix attribute path '{attr}' contains characters outside \
             the allowed set [A-Za-z0-9_.+-]"
        );
    }
    Ok(())
}

/// Validate that `rev` is a plausible git hex hash (7..=64 chars,
/// hex only). Used at any site where the rev flows from a state
/// file (potentially tampered) into a Nix expression or a URL.
///
/// # Errors
///
/// Returns an error describing the rejection.
pub fn git_hex_rev(rev: &str) -> Result<()> {
    if rev.len() < 7 || rev.len() > 64 {
        bail!(
            "Git rev has unusual length ({} chars, expected 7..=64): {:?}",
            rev.len(),
            rev
        );
    }
    if !rev.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("Git rev is not a hex hash: {:?}", rev);
    }
    Ok(())
}

/// Validate that `nar_hash` is a plausible SRI-form Nix narHash.
/// Accepts `sha256-…` or `sha512-…` with no control / quote / backslash
/// characters and total length ≤ 200.
///
/// # Errors
///
/// Returns an error describing the rejection.
pub fn nar_hash_sri(nar_hash: &str) -> Result<()> {
    if !nar_hash.starts_with("sha256-") && !nar_hash.starts_with("sha512-") {
        bail!(
            "narHash should be SRI sha256-… or sha512-…: {:?}",
            nar_hash
        );
    }
    if nar_hash.len() > 200 {
        bail!("narHash is suspiciously long ({} chars)", nar_hash.len());
    }
    if nar_hash
        .chars()
        .any(|c| c.is_control() || c == '"' || c == '\\')
    {
        bail!("narHash contains an invalid character: {:?}", nar_hash);
    }
    Ok(())
}

#[cfg(test)]
#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
mod tests {
    use super::*;

    #[test]
    fn package_name_accepts_typical_nixpkgs_names() {
        for n in &[
            "firefox",
            "linuxKernel",
            "kdePackages.kate",
            "gcc-13",
            "libfoo_2",
            "openssl_3+",
        ] {
            assert!(package_name(n).is_ok(), "should accept {n}");
        }
    }

    #[test]
    fn package_name_rejects_invalid() {
        assert!(package_name("").is_err());
        assert!(package_name("foo/bar").is_err());
        assert!(package_name("with\nnewline").is_err());
        assert!(package_name("with\"quote").is_err());
        assert!(package_name(&"x".repeat(200)).is_err());
    }

    #[test]
    fn nix_attr_path_strict() {
        assert!(nix_attr_path("firefox").is_ok());
        assert!(nix_attr_path("kdePackages.kate").is_ok());
        // package_name accepts these but nix_attr_path equally rejects
        // bad chars; the difference is doc-only at the moment.
        assert!(nix_attr_path("foo/bar").is_err());
        assert!(nix_attr_path("").is_err());
    }

    #[test]
    fn git_hex_rev_accepts_valid_lengths() {
        assert!(git_hex_rev("0123456").is_ok()); // 7
        assert!(git_hex_rev("0123456789abcdef0123456789abcdef01234567").is_ok()); // 40
        assert!(git_hex_rev(&"a".repeat(64)).is_ok()); // 64 (sha256)
    }

    #[test]
    fn git_hex_rev_rejects() {
        assert!(git_hex_rev("").is_err());
        assert!(git_hex_rev("abc").is_err()); // too short
        assert!(git_hex_rev(&"a".repeat(65)).is_err()); // too long
        assert!(git_hex_rev("not-hex-at-all").is_err());
        assert!(git_hex_rev("0123456789abcdefXXXXXX").is_err());
    }

    #[test]
    fn nar_hash_sri_accepts_valid() {
        assert!(nar_hash_sri("sha256-AAAA1111BBBB2222=").is_ok());
        assert!(nar_hash_sri("sha512-MMMMNNNNOOOO=").is_ok());
    }

    #[test]
    fn nar_hash_sri_rejects() {
        assert!(nar_hash_sri("md5-doesntcount").is_err());
        assert!(nar_hash_sri("sha256-with\"quote").is_err());
        assert!(nar_hash_sri("sha256-with\\backslash").is_err());
        assert!(nar_hash_sri(&format!("sha256-{}", "x".repeat(250))).is_err());
    }
}
