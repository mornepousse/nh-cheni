//! On-disk cache for API results.
//!
//! Stores Repology query results in ~/.cache/nixup/versions.json
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
        .join("nixup")
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

    let cache: Cache = match serde_json::from_str(&content) {
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

    debug!("Cache loaded: {} entries, {}s old", cache.entries.len(), age);
    cache
}

/// Save the cache to disk.
pub fn save(cache: &Cache) {
    let path = cache_path();

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    match serde_json::to_string(cache) {
        Ok(content) => {
            if let Err(e) = std::fs::write(&path, content) {
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

/// Create a new cache with the current timestamp.
pub fn new_with_timestamp() -> Cache {
    Cache {
        timestamp: now_secs(),
        entries: HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
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
}
