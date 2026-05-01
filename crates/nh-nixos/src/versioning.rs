//! Version-string parsing and comparison.
//!
//! Nix store paths and `meta.version` strings come in many shapes:
//!   - Pure semver: `1.2.3`
//!   - With pre-release: `1.2.3-beta1`, `2.0-rc2`
//!   - PEP440-ish: `3.15.0a7`
//!   - Platform suffix: `1.94.1-x86_64-unknown-linux-gnu`
//!   - Calver: `2026.04.01`
//!
//! [`parse_version`] extracts the leading numeric parts and stops at
//! the first non-numeric segment. [`compare_versions`] is calver-aware
//! (a calver "major" is not treated as a breaking bump). [`is_prerelease`]
//! flags pre-release / unstable markers so callers can avoid suggesting
//! `3.15.0a7` as the "latest" when the user is on stable `3.14.3`.
//!
//! Ported as-is from wrapper-era `src/version/parse.rs` and
//! `src/version/compare.rs`. The wrapper had ~600 tests for these
//! modules; this port re-runs the meaningful subset directly inline.

use std::cmp::Ordering;

/// Extract numeric version parts from a version string.
///
/// Splits on `.` and keeps only the leading-digits prefix of each
/// segment. Stops at the first segment that doesn't start with a
/// digit. Empty input or non-numeric input returns an empty Vec.
#[must_use]
pub fn parse_version(version: &str) -> Vec<u64> {
  version
    .split('.')
    .map(|segment| {
      segment
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
    })
    .take_while(|digits| !digits.is_empty())
    .filter_map(|digits| digits.parse::<u64>().ok())
    .collect()
}

/// Detect a pre-release / unstable version marker.
///
/// Returns true for alpha/beta/rc/pre/dev/unstable suffixes (with
/// `-` or `_` separator) and PEP440-style markers (`3.15.0a7`).
#[must_use]
pub fn is_prerelease(version: &str) -> bool {
  // PEP440-ish: a digit, then `a` / `b` / `rc`, then another digit.
  // "3.15.0a7" matches; "1.0-build42" doesn't (the b isn't preceded
  // by a digit), "alacritty-0.17.0" doesn't either.
  static PEP440_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| {
      regex::Regex::new(r"\d(a|b|rc)\d").expect("valid regex")
    });

  const SUFFIX_MARKERS: &[&str] = &[
    "-alpha",
    "-beta",
    "-rc",
    "-pre",
    "-dev",
    "-unstable",
    "-snapshot",
    "_alpha",
    "_beta",
    "_rc",
    "_pre",
    "_dev",
    "alpha",
    "beta",
  ];

  let lower = version.to_lowercase();
  PEP440_RE.is_match(&lower)
    || SUFFIX_MARKERS.iter().any(|m| lower.contains(m))
}

/// Result of comparing two parsed version vectors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionDiff {
  /// `1.2.3 == 1.2.3`.
  Equal,
  /// Minor update (first segment unchanged): `1.2.3 → 1.3.0`.
  Minor,
  /// Major update (first segment incremented): `9.0.2 → 10.0.1`.
  /// Calver versions are NOT classified as Major even when the first
  /// segment differs (e.g. `2025.04 → 2026.05` stays Minor).
  Major,
  /// Installed is newer than available (ahead of upstream).
  Newer,
}

/// Compare two parsed version vectors. Returns the relationship
/// between `installed` and `available`.
#[must_use]
pub fn compare_versions(installed: &[u64], available: &[u64]) -> VersionDiff {
  match version_ordering(installed, available) {
    Ordering::Equal => VersionDiff::Equal,
    Ordering::Greater => VersionDiff::Newer,
    Ordering::Less => {
      let installed_major = installed.first().copied().unwrap_or(0);
      let available_major = available.first().copied().unwrap_or(0);
      if available_major > installed_major
        && !is_calver(installed_major)
        && !is_calver(available_major)
      {
        VersionDiff::Major
      } else {
        VersionDiff::Minor
      }
    },
  }
}

/// Element-wise comparison; missing slots are treated as 0 so
/// `[1, 2] == [1, 2, 0]`.
fn version_ordering(a: &[u64], b: &[u64]) -> Ordering {
  let max_len = a.len().max(b.len());
  for i in 0..max_len {
    let va = a.get(i).copied().unwrap_or(0);
    let vb = b.get(i).copied().unwrap_or(0);
    match va.cmp(&vb) {
      Ordering::Equal => continue,
      other => return other,
    }
  }
  Ordering::Equal
}

/// `2000+` looks like a calendar year and gets calver treatment.
fn is_calver(major: u64) -> bool {
  major >= 2000
}

#[cfg(test)]
#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
mod tests {
  use super::*;

  // ── parse_version ────────────────────────────────────────────────

  #[test]
  fn parse_pure_semver() {
    assert_eq!(parse_version("1.2.3"), vec![1, 2, 3]);
    assert_eq!(parse_version("0.17.0"), vec![0, 17, 0]);
    assert_eq!(parse_version("10.0.1"), vec![10, 0, 1]);
  }

  #[test]
  fn parse_truncates_at_non_digit_segment() {
    assert_eq!(parse_version("1.94.1-x86_64"), vec![1, 94, 1]);
    assert_eq!(parse_version("2.0.0-beta1"), vec![2, 0, 0]);
    assert_eq!(parse_version("1.0-rc2"), vec![1, 0]);
  }

  #[test]
  fn parse_handles_trailing_garbage() {
    assert_eq!(parse_version("128.0.1unknown"), vec![128, 0, 1]);
    assert_eq!(parse_version("1.2.foobar"), vec![1, 2]);
  }

  #[test]
  fn parse_empty_or_garbage() {
    assert_eq!(parse_version(""), Vec::<u64>::new());
    assert_eq!(parse_version("not-a-version"), Vec::<u64>::new());
  }

  #[test]
  fn parse_calver() {
    assert_eq!(parse_version("2026.04.01"), vec![2026, 4, 1]);
    assert_eq!(parse_version("25.05"), vec![25, 5]);
  }

  // ── is_prerelease ────────────────────────────────────────────────

  #[test]
  fn is_prerelease_word_markers() {
    assert!(is_prerelease("2.0.0-beta1"));
    assert!(is_prerelease("1.0-rc2"));
    assert!(is_prerelease("0.17.0-unstable"));
    assert!(is_prerelease("1.0-pre"));
    assert!(is_prerelease("2.0.0_alpha"));
  }

  #[test]
  fn is_prerelease_pep440() {
    assert!(is_prerelease("3.15.0a7"));
    assert!(is_prerelease("3.0.0b1"));
    assert!(is_prerelease("1.0rc1"));
  }

  #[test]
  fn is_prerelease_rejects_stable() {
    assert!(!is_prerelease("3.14.3"));
    assert!(!is_prerelease("2026.04.01"));
    assert!(!is_prerelease("1.0.0"));
    assert!(!is_prerelease("128.0"));
  }

  #[test]
  fn is_prerelease_avoids_false_positives_on_substrings() {
    // `alacritty` contains "a" but isn't preceded by a digit.
    assert!(!is_prerelease("alacritty-0.17.0"));
    // `build42` has a `b` but no PEP440 shape.
    assert!(!is_prerelease("1.0-build42"));
  }

  // ── compare_versions ─────────────────────────────────────────────

  #[test]
  fn compare_equal() {
    assert_eq!(
      compare_versions(&[1, 2, 3], &[1, 2, 3]),
      VersionDiff::Equal
    );
    // Trailing zeros equal absence.
    assert_eq!(
      compare_versions(&[1, 2], &[1, 2, 0]),
      VersionDiff::Equal
    );
  }

  #[test]
  fn compare_minor_update() {
    assert_eq!(
      compare_versions(&[1, 2, 0], &[1, 3, 0]),
      VersionDiff::Minor
    );
    assert_eq!(
      compare_versions(&[1, 2, 0], &[1, 2, 1]),
      VersionDiff::Minor
    );
  }

  #[test]
  fn compare_major_update() {
    assert_eq!(
      compare_versions(&[9, 0, 2], &[10, 0, 1]),
      VersionDiff::Major
    );
    assert_eq!(
      compare_versions(&[1, 99, 99], &[2, 0, 0]),
      VersionDiff::Major
    );
  }

  #[test]
  fn compare_newer() {
    assert_eq!(
      compare_versions(&[2, 0, 0], &[1, 9, 0]),
      VersionDiff::Newer
    );
    assert_eq!(
      compare_versions(&[1, 2, 1], &[1, 2, 0]),
      VersionDiff::Newer
    );
  }

  #[test]
  fn compare_calver_does_not_classify_as_major() {
    // 2025.04 → 2026.05 is a Minor update (calver), not Major.
    assert_eq!(
      compare_versions(&[2025, 4], &[2026, 5]),
      VersionDiff::Minor
    );
    assert_eq!(
      compare_versions(&[2026, 4, 1], &[2026, 5, 0]),
      VersionDiff::Minor
    );
  }

  #[test]
  fn compare_handles_empty_vectors() {
    // Empty installed treated as 0; jumping to major 1 = Major.
    assert_eq!(
      compare_versions(&[], &[1, 0, 0]),
      VersionDiff::Major
    );
    assert_eq!(compare_versions(&[], &[]), VersionDiff::Equal);
    // Empty installed vs same-zero available stays Equal (0 == 0).
    assert_eq!(
      compare_versions(&[], &[0, 0, 0]),
      VersionDiff::Equal
    );
  }
}
