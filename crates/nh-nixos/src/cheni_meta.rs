//! Helpers for reading the cheni-fork's version components at runtime.
//!
//! The workspace version follows option B:
//! `<nh-base>+cheni.<cheni-layer>` (e.g. `4.3.2+cheni.0.1.0`).
//! Code that needs to display or report just one of the two halves
//! goes through these helpers instead of re-parsing
//! `CARGO_PKG_VERSION` at every call site.
//!
//! For the `--version` CLI output, see `crates/nh/build.rs` which
//! composes the full pretty string at build time. This module is for
//! the *runtime* consumers (bug-report, doctor, anything that wants
//! to mention the cheni layer specifically without the nh base).

const FULL: &str = env!("CARGO_PKG_VERSION");

/// The nh-upstream version we forked from (e.g. `4.3.2`).
///
/// Returns the whole `CARGO_PKG_VERSION` if the `+cheni.<x>` build
/// metadata marker is missing — that happens only if someone edits
/// the workspace version away from option B by mistake; the function
/// degrades gracefully rather than panicking.
#[must_use]
pub fn nh_base_version() -> &'static str {
    match FULL.split_once("+cheni.") {
        Some((base, _)) => base,
        None => FULL,
    }
}

/// The cheni layer version (e.g. `0.1.0`). Returns `"?"` when the
/// `+cheni.<x>` marker is absent, mirroring the build.rs fallback.
#[must_use]
pub fn cheni_layer_version() -> &'static str {
    match FULL.split_once("+cheni.") {
        Some((_, layer)) => layer,
        None => "?",
    }
}

/// The verbatim workspace version (`<nh-base>+cheni.<layer>`).
/// Useful when you need the exact value Cargo sees, e.g. for an
/// error message that has to reproduce a Cargo lookup.
#[must_use]
pub fn full_version() -> &'static str {
    FULL
}

#[cfg(test)]
#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
mod tests {
    use super::*;

    #[test]
    fn nh_base_extracted_from_workspace_version() {
        // The workspace version follows option B; this test fails
        // loudly if someone bumps the workspace version away from
        // the `<base>+cheni.<layer>` shape without updating this
        // module's contract.
        assert!(nh_base_version().chars().any(|c| c.is_ascii_digit()));
        assert!(!nh_base_version().contains("+cheni."));
    }

    #[test]
    fn cheni_layer_extracted_from_workspace_version() {
        let layer = cheni_layer_version();
        assert_ne!(
            layer, "?",
            "workspace.package.version is missing the +cheni.<x> marker"
        );
        assert!(layer.chars().any(|c| c.is_ascii_digit()));
    }

    #[test]
    fn full_version_includes_both_parts() {
        let full = full_version();
        assert!(full.contains("+cheni."));
        assert_eq!(
            format!("{}+cheni.{}", nh_base_version(), cheni_layer_version()),
            full
        );
    }
}
