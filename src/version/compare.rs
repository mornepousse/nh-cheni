//! Version comparison logic.
//!
//! Compares two parsed version vectors and determines:
//! - Whether an update is available
//! - Whether it's a major (breaking) or minor (safe) update

use std::cmp::Ordering;

/// Result of comparing two versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionDiff {
    /// Same version (e.g. 1.2.3 == 1.2.3).
    Equal,
    /// Minor update available (e.g. 1.2.3 -> 1.3.0).
    /// The first version number (major) is unchanged.
    Minor,
    /// Major update available (e.g. 9.0.2 -> 10.0.1).
    /// The first version number changed -- breaking changes possible.
    Major,
    /// Installed version is newer than available (ahead of nixpkgs).
    Newer,
}

/// Compare two parsed version vectors.
///
/// Returns the relationship between `installed` and `available`.
///
/// # Examples
/// ```
/// use cheni::version::compare::{compare_versions, VersionDiff};
/// assert_eq!(compare_versions(&[1, 2, 3], &[1, 2, 3]), VersionDiff::Equal);
/// assert_eq!(compare_versions(&[1, 2, 0], &[1, 3, 0]), VersionDiff::Minor);
/// assert_eq!(compare_versions(&[9, 0, 2], &[10, 0, 1]), VersionDiff::Major);
/// assert_eq!(compare_versions(&[2, 0, 0], &[1, 9, 0]), VersionDiff::Newer);
/// ```
pub fn compare_versions(installed: &[u64], available: &[u64]) -> VersionDiff {
    let ordering = version_ordering(installed, available);

    match ordering {
        Ordering::Equal => VersionDiff::Equal,
        Ordering::Greater => VersionDiff::Newer,
        Ordering::Less => {
            // The update is available — check if it's major or minor.
            // Major = first version number changed, unless it's calver.
            let installed_major = installed.first().copied().unwrap_or(0);
            let available_major = available.first().copied().unwrap_or(0);

            if available_major > installed_major && !is_calver(installed_major) && !is_calver(available_major) {
                VersionDiff::Major
            } else {
                VersionDiff::Minor
            }
        }
    }
}

/// Check if a version number looks like a calendar year (calver).
///
/// Versions like 2026.04.01 use dates instead of semver.
/// We don't treat these as major bumps (2.004 → 2026.04.01 is not "major").
fn is_calver(major: u64) -> bool {
    major >= 2000
}

/// Compare two version vectors element by element.
///
/// Missing elements are treated as 0 (e.g. [1, 2] == [1, 2, 0]).
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

#[cfg(test)]
#[path = "tests/compare.rs"]
mod tests;
