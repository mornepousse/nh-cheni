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
/// use nixup::version::parse::parse_version;
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
}
