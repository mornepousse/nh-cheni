//! On-disk cache for API results.
//!
//! Stores Repology query results in ~/.cache/cheni/versions.json
//! to avoid hitting the API on every run. Cache expires after 1 hour.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tracing::debug;

/// How long cached results stay valid (in seconds).
const CACHE_TTL_SECS: u64 = 3600; // 1 hour

/// Cached API results on disk.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct Cache {
    /// When the cache was last written (unix timestamp).
    pub timestamp: u64,
    /// Package name → cached result.
    pub entries: HashMap<String, CachedPackage>,
}

/// A cached package version result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedPackage {
    pub version: Option<String>,
    pub description: Option<String>,
}

/// Path to the cache file.
fn cache_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("cheni")
        .join("versions.json")
}

/// Current unix timestamp in seconds.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Load the cache from disk.
///
/// Returns an empty cache if the file doesn't exist, is corrupted, or has expired.
pub fn load() -> Cache {
    let path = cache_path();

    if !path.exists() {
        debug!("No cache file found");
        return Cache::default();
    }

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            debug!("Failed to read cache: {}", e);
            return Cache::default();
        }
    };

    let mut cache: Cache = match serde_json::from_str(&content) {
        Ok(c) => c,
        Err(e) => {
            debug!("Failed to parse cache: {}", e);
            return Cache::default();
        }
    };

    let age = now_secs().saturating_sub(cache.timestamp);
    if age > CACHE_TTL_SECS {
        debug!("Cache expired ({}s old, TTL={}s)", age, CACHE_TTL_SECS);
        return Cache::default();
    }

    // Drop entries whose version is None — these were either failed lookups
    // (now retryable) or stale "unknown" markers from older cheni versions
    // that used to persist them. Either way they masquerade as legitimate
    // hits and end up classified as "Unknown" forever.
    let before = cache.entries.len();
    cache.entries.retain(|_, entry| entry.version.is_some());
    let dropped = before - cache.entries.len();
    if dropped > 0 {
        debug!("Cache: dropped {} stale null entries", dropped);
    }

    debug!("Cache loaded: {} entries, {}s old", cache.entries.len(), age);
    cache
}

/// Read-only stats about the on-disk cache. Used by `cheni doctor` to
/// report freshness and surface stale-null cleanups.
pub struct CacheStats {
    pub age_secs: u64,
    pub total_entries: usize,
    pub null_entries: usize,
    pub exists: bool,
}

pub fn stats() -> CacheStats {
    let path = cache_path();
    if !path.exists() {
        return CacheStats {
            age_secs: 0,
            total_entries: 0,
            null_entries: 0,
            exists: false,
        };
    }
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let cache: Cache = serde_json::from_str(&content).unwrap_or_default();
    let null_entries = cache.entries.values().filter(|e| e.version.is_none()).count();
    CacheStats {
        age_secs: now_secs().saturating_sub(cache.timestamp),
        total_entries: cache.entries.len(),
        null_entries,
        exists: true,
    }
}

/// Save the cache to disk.
pub fn save(cache: &Cache) {
    let path = cache_path();

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    match serde_json::to_string(cache) {
        Ok(content) => {
            // Atomic write so a concurrent `cheni check` or a SIGKILL
            // mid-write can't leave the cache truncated / half-JSON.
            if let Err(e) = crate::util::atomic_write(&path, &content) {
                debug!("Failed to write cache: {}", e);
            } else {
                debug!("Cache saved: {} entries", cache.entries.len());
            }
        }
        Err(e) => {
            debug!("Failed to serialize cache: {}", e);
        }
    }
}

/// Delete the on-disk cache file (best effort — silently ignores
/// 'file does not exist'). Used by `cheni check --refresh`.
pub fn clear() -> std::io::Result<()> {
    let path = cache_path();
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Create a new cache with the current timestamp.
pub fn new_with_timestamp() -> Cache {
    Cache {
        timestamp: now_secs(),
        entries: HashMap::new(),
    }
}

#[cfg(test)]
#[path = "tests/cache.rs"]
mod tests;
