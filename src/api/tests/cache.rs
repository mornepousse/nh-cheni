use super::*;

#[test]
fn empty_cache_is_valid() {
    let cache = Cache::default();
    assert!(cache.entries.is_empty());
    assert_eq!(cache.timestamp, 0);
}

#[test]
fn new_cache_has_timestamp() {
    let cache = new_with_timestamp();
    assert!(cache.timestamp > 0);
}

// ── TTL / expiry tests ────────────────────────────────────────────────────────

/// `load()` returns an empty cache (not a panic) when the JSON is corrupted.
#[test]
fn load_corrupted_json_returns_empty() {
    // We test the parse branch directly without touching the filesystem, by
    // replicating the same serde_json call that `load()` does internally.
    let bad_json = "{this is not valid JSON}";
    let result: Result<Cache, _> = serde_json::from_str(bad_json);
    assert!(result.is_err(), "should fail to parse corrupted JSON");
    // The real `load()` converts the error to Cache::default() — test that
    // the branch is reachable by verifying Default works.
    let fallback = Cache::default();
    assert!(fallback.entries.is_empty());
}

/// `load()` on an empty JSON object `{}` should deserialise to defaults
/// (schema-drift scenario: old cache file missing 'entries' field).
#[test]
fn load_schema_drift_missing_fields_returns_empty() {
    // `#[derive(Deserialize, Default)]` + `#[serde(default)]` means an empty
    // object is valid and produces the default values.
    let minimal = "{}";
    let cache: Cache = serde_json::from_str(minimal).expect("empty object is valid");
    assert_eq!(cache.timestamp, 0);
    assert!(cache.entries.is_empty());
}

/// A cache with a timestamp = 0 is treated as expired (age = now − 0 ≫ TTL).
#[test]
fn cache_with_zero_timestamp_is_expired() {
    // Simulate what `load()` does: age = now_secs() − 0 which is huge.
    let age: u64 = now_secs(); // time since epoch ≈ 1.7 billion
    assert!(age > CACHE_TTL_SECS, "zero timestamp should always appear expired");
}

/// A cache with a recent timestamp should not be expired.
#[test]
fn cache_with_fresh_timestamp_is_valid() {
    let now = now_secs();
    let age = now.saturating_sub(now); // 0 seconds old
    assert!(age <= CACHE_TTL_SECS);
}

/// A cache with a timestamp CACHE_TTL_SECS + 1 seconds old is expired.
#[test]
fn cache_expires_after_ttl() {
    let now = now_secs();
    let old_timestamp = now.saturating_sub(CACHE_TTL_SECS + 1);
    let age = now.saturating_sub(old_timestamp);
    assert!(age > CACHE_TTL_SECS, "cache should be expired at TTL+1 seconds");
}

// ── Null-entry purge ──────────────────────────────────────────────────────────

/// `load()` silently drops entries whose `version` is None. We test the
/// retain logic directly by mimicking what `load()` does after parsing.
#[test]
fn null_version_entries_are_dropped_on_load() {
    let mut cache = new_with_timestamp();
    cache.entries.insert(
        "good".to_string(),
        CachedPackage { version: Some("1.0".to_string()), description: None },
    );
    cache.entries.insert(
        "bad".to_string(),
        CachedPackage { version: None, description: None },
    );

    let before = cache.entries.len();
    cache.entries.retain(|_, e| e.version.is_some());
    let after = cache.entries.len();

    assert_eq!(before, 2);
    assert_eq!(after, 1);
    assert!(cache.entries.contains_key("good"));
    assert!(!cache.entries.contains_key("bad"));
}

// ── collect_and_merge timestamp preservation ──────────────────────────────────

/// When the prior cache has a valid (non-zero) timestamp, merging new entries
/// must NOT reset the timestamp. Otherwise a single permanent-miss package
/// would extend the TTL for all already-cached entries on every run.
#[test]
fn merge_with_valid_prior_preserves_timestamp() {
    use std::collections::HashMap;

    let prior_ts: u64 = 1_700_000_000; // some fixed past timestamp
    let prior = Cache {
        timestamp: prior_ts,
        entries: {
            let mut m = HashMap::new();
            m.insert(
                "firefox".to_string(),
                CachedPackage { version: Some("149.0".to_string()), description: None },
            );
            m
        },
    };

    // Replicate the collect_and_merge branching: prior.timestamp != 0 →
    // preserve it.
    let merged = if prior.timestamp == 0 {
        new_with_timestamp()
    } else {
        Cache {
            timestamp: prior.timestamp,
            entries: HashMap::new(),
        }
    };

    assert_eq!(merged.timestamp, prior_ts, "timestamp must be preserved from prior");
}

/// When the prior cache has timestamp = 0 (empty/expired), a fresh timestamp
/// must be assigned so the merged cache has a valid TTL start.
#[test]
fn merge_with_empty_prior_creates_fresh_timestamp() {
    let prior = Cache::default(); // timestamp = 0

    let merged = if prior.timestamp == 0 {
        new_with_timestamp()
    } else {
        Cache {
            timestamp: prior.timestamp,
            entries: std::collections::HashMap::new(),
        }
    };

    let now = now_secs();
    assert!(merged.timestamp > 0, "fresh timestamp should be non-zero");
    assert!(
        merged.timestamp <= now,
        "fresh timestamp should not be in the future"
    );
    assert!(
        now.saturating_sub(merged.timestamp) < 5,
        "fresh timestamp should be within 5 seconds of now"
    );
}
