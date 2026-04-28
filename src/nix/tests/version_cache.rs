use std::io::Write;

use tempfile::TempDir;

use crate::nix::version_cache::VersionCache;

/// Helper: creates a fresh isolated TempDir for each test.
fn tmp() -> TempDir {
    tempfile::tempdir().expect("tempdir must be creatable in tests")
}

/// A non-existent path should produce an empty cache without error.
#[test]
fn empty_when_file_missing() {
    let dir = tmp();
    let path = dir.path().join("does-not-exist.json");

    let cache = VersionCache::load(&path).expect("load on missing file must not error");
    assert!(
        cache.lookup("nixpkgs", "abc123", "legacyPackages.x86_64-linux.firefox").is_none(),
        "fresh cache must return None for any lookup"
    );
}

/// store → save → reload → lookup must return the original value.
#[test]
fn store_then_lookup_roundtrip() {
    let dir = tmp();
    let path = dir.path().join("version-cache.json");

    let mut cache = VersionCache::default();
    cache.store("nixpkgs", "rev1abc", "legacyPackages.x86_64-linux.ripgrep", "14.1.0");
    cache.save(&path).expect("save must succeed");

    let loaded = VersionCache::load(&path).expect("reload must succeed");
    let version = loaded.lookup("nixpkgs", "rev1abc", "legacyPackages.x86_64-linux.ripgrep");
    assert_eq!(version.as_deref(), Some("14.1.0"));
}

/// A value stored under rev1 must not be visible under rev2.
#[test]
fn rev_change_invalidates_lookup() {
    let dir = tmp();
    let path = dir.path().join("version-cache.json");

    let mut cache = VersionCache::default();
    cache.store("nixpkgs", "rev1abc", "legacyPackages.x86_64-linux.htop", "3.3.0");
    cache.save(&path).expect("save must succeed");

    let loaded = VersionCache::load(&path).expect("reload must succeed");
    assert_eq!(
        loaded.lookup("nixpkgs", "rev1abc", "legacyPackages.x86_64-linux.htop").as_deref(),
        Some("3.3.0"),
        "same rev must hit"
    );
    assert!(
        loaded.lookup("nixpkgs", "rev2xyz", "legacyPackages.x86_64-linux.htop").is_none(),
        "different rev must miss"
    );
}

/// Two entries with the same attr but different inputs must coexist.
#[test]
fn different_inputs_dont_collide() {
    let dir = tmp();
    let path = dir.path().join("version-cache.json");

    let mut cache = VersionCache::default();
    cache.store("nixpkgs", "revA", "legacyPackages.x86_64-linux.git", "2.44.0");
    cache.store("nixpkgs-latest", "revB", "legacyPackages.x86_64-linux.git", "2.45.0");
    cache.save(&path).expect("save must succeed");

    let loaded = VersionCache::load(&path).expect("reload must succeed");
    assert_eq!(
        loaded.lookup("nixpkgs", "revA", "legacyPackages.x86_64-linux.git").as_deref(),
        Some("2.44.0"),
        "nixpkgs entry must be retrievable"
    );
    assert_eq!(
        loaded.lookup("nixpkgs-latest", "revB", "legacyPackages.x86_64-linux.git").as_deref(),
        Some("2.45.0"),
        "nixpkgs-latest entry must be retrievable independently"
    );
}

/// A file with garbage content must not cause an error — just an empty cache.
#[test]
fn corrupt_file_treated_as_empty() {
    let dir = tmp();
    let path = dir.path().join("corrupt.json");

    {
        let mut f = std::fs::File::create(&path).expect("file creation must succeed");
        f.write_all(b"this is not json {{{{{").expect("write must succeed");
    }

    let cache = VersionCache::load(&path).expect("load on corrupt file must not error");
    assert!(
        cache.lookup("nixpkgs", "anyrev", "anyattr").is_none(),
        "corrupt file must produce empty cache"
    );
}
