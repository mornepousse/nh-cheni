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
    let lower = version.to_lowercase();

    // PEP440 / Python style: a digit immediately followed by a/b/rc.
    // Captures "3.15.0a7", "2.0b1", "1.0rc3".
    let mut chars = lower.chars().peekable();
    let mut prev_digit = false;
    while let Some(c) = chars.next() {
        if prev_digit {
            // Look at the current char and the following ones for a/b/rc.
            if c == 'a' || c == 'b' {
                // Make sure it's not "abi" or "build" — only accept if the
                // very next char is a digit (real PEP440 alpha/beta marker).
                if chars.peek().map(|n| n.is_ascii_digit()).unwrap_or(false) {
                    return true;
                }
            } else if c == 'r' && chars.peek() == Some(&'c') {
                return true;
            }
        }
        prev_digit = c.is_ascii_digit();
    }

    // Common suffix words separated by - or _.
    for marker in [
        "-alpha", "-beta", "-rc", "-pre", "-dev", "-unstable", "-snapshot",
        "_alpha", "_beta", "_rc", "_pre", "_dev",
        "alpha", "beta",  // bare suffixes (e.g. "1.0alpha")
    ] {
        if lower.contains(marker) {
            return true;
        }
    }

    false
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
