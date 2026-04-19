//! Version string parsing.
//!
//! Nix store paths contain version strings in many formats:
//! - Simple: "1.2.3"
//! - With pre-release: "1.2.3-beta1"
//! - With platform suffix: "1.94.1-x86_64-unknown-linux-gnu"
//!
//! This module extracts the numeric parts for comparison.

/// Extract numeric version parts from a version string.
///
/// Only keeps the leading sequence of numbers separated by dots.
/// Stops at the first non-numeric, non-dot character.
///
/// # Examples
/// ```
/// use cheni::version::parse::parse_version;
/// assert_eq!(parse_version("1.2.3"), vec![1, 2, 3]);
/// assert_eq!(parse_version("1.94.1-x86_64"), vec![1, 94, 1]);
/// assert_eq!(parse_version("0.17.0"), vec![0, 17, 0]);
/// ```
pub fn parse_version(version: &str) -> Vec<u64> {
    // Split on dots to get version segments: "1.94.1-x86_64" → ["1", "94", "1-x86_64"]
    // For each segment, take only the leading digits.
    // Stop at the first segment that doesn't start with a digit.
    version
        .split('.')
        .map(|segment| {
            // Extract leading digits from this segment
            let digits: String = segment.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits
        })
        .take_while(|digits| !digits.is_empty())
        .filter_map(|digits| digits.parse::<u64>().ok())
        .collect()
}

/// Detect a pre-release / unstable version marker.
///
/// Returns true when the version string contains a recognised pre-release
/// suffix: alpha (`a`/`alpha`), beta (`b`/`beta`), release candidate (`rc`),
/// development snapshot (`dev`/`pre`/`unstable`), or pep440-style markers
/// (`3.15.0a7` → alpha 7).
///
/// Used by `cheni check` to avoid suggesting a python `3.15.0a7` as the
/// "latest" when the user is on a stable `3.14.3`.
///
/// # Examples
/// ```
/// use cheni::version::parse::is_prerelease;
/// assert!(is_prerelease("3.15.0a7"));
/// assert!(is_prerelease("2.0.0-beta1"));
/// assert!(is_prerelease("1.0-rc2"));
/// assert!(is_prerelease("0.17.0-unstable"));
/// assert!(!is_prerelease("3.14.3"));
/// assert!(!is_prerelease("2026.04.01"));
/// ```
pub fn is_prerelease(version: &str) -> bool {
    /// PEP440-ish marker: a digit, then `a` / `b` / `rc`, then another digit.
    /// "3.15.0a7" matches; "1.0-build42" doesn't (the `b` isn't preceded
    /// by a digit), "alacritty-0.17.0" doesn't (the leading `a` isn't
    /// preceded by a digit either).
    static PEP440_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\d(a|b|rc)\d").expect("valid regex"));

    /// Word-style markers we look for as substrings. Both `-alpha` and
    /// `_alpha` shapes are common; bare `alpha` / `beta` covers the rare
    /// "1.0alpha" / "2.0beta" cases without needing a separator.
    const SUFFIX_MARKERS: &[&str] = &[
        "-alpha", "-beta", "-rc", "-pre", "-dev", "-unstable", "-snapshot",
        "_alpha", "_beta", "_rc", "_pre", "_dev",
        "alpha", "beta",
    ];

    let lower = version.to_lowercase();
    PEP440_RE.is_match(&lower) || SUFFIX_MARKERS.iter().any(|m| lower.contains(m))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_version() {
        assert_eq!(parse_version("1.2.3"), vec![1, 2, 3]);
    }

    #[test]
    fn two_part_version() {
        assert_eq!(parse_version("3.28"), vec![3, 28]);
    }

    #[test]
    fn single_number() {
        assert_eq!(parse_version("42"), vec![42]);
    }

    #[test]
    fn version_with_suffix() {
        assert_eq!(parse_version("1.94.1-x86_64-unknown-linux-gnu"), vec![1, 94, 1]);
    }

    #[test]
    fn version_with_pre_release() {
        assert_eq!(parse_version("2.0.0-beta1"), vec![2, 0, 0]);
    }

    #[test]
    fn version_with_unstable_suffix() {
        assert_eq!(parse_version("0.17.0-unstable"), vec![0, 17, 0]);
    }

    #[test]
    fn empty_string() {
        assert_eq!(parse_version(""), Vec::<u64>::new());
    }

    #[test]
    fn no_digits() {
        assert_eq!(parse_version("alpha"), Vec::<u64>::new());
    }

    #[test]
    fn detects_pep440_alpha() {
        // The python case that motivated the helper: 3.15.0a7 must NOT
        // appear as a stable update for a user on 3.14.3.
        assert!(is_prerelease("3.15.0a7"));
        assert!(is_prerelease("2.0b1"));
        assert!(is_prerelease("1.0rc3"));
    }

    #[test]
    fn detects_dash_suffixes() {
        assert!(is_prerelease("2.0.0-beta1"));
        assert!(is_prerelease("1.0-rc2"));
        assert!(is_prerelease("0.17.0-unstable"));
        assert!(is_prerelease("4.5-pre"));
        assert!(is_prerelease("0.1-dev"));
    }

    #[test]
    fn stable_versions_not_flagged() {
        assert!(!is_prerelease("3.14.3"));
        assert!(!is_prerelease("1.0.0"));
        // Calver dates must not trip the heuristic.
        assert!(!is_prerelease("2026.04.01"));
        assert!(!is_prerelease("20240301"));
        // Words containing 'a' or 'b' but not as a pre-release marker.
        assert!(!is_prerelease("1.0-build42"));
        assert!(!is_prerelease("alacritty-0.17.0"));
    }
}
