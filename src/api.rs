use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Repology API URL
const REPOLOGY_API_URL: &str = "https://repology.org/api/v1/project";

/// Maximum number of concurrent requests
const MAX_CONCURRENT_REQUESTS: usize = 10;

/// Cache validity duration (in seconds) -- 1 hour
const CACHE_TTL_SECS: u64 = 3600;

/// Result of an API request for a package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResult {
    pub query_name: String,
    pub version: Option<String>,
    pub description: Option<String>,
    pub homepage: Option<String>,
}

/// On-disk cache
#[derive(Debug, Serialize, Deserialize, Default)]
struct Cache {
    timestamp: u64,
    entries: HashMap<String, ApiResult>,
}

/// Repology response structure
#[derive(Debug, Deserialize)]
struct RepologyEntry {
    #[serde(default)]
    repo: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    summary: Option<String>,
}

/// Cache file path
fn cache_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("nixup")
        .join("versions.json")
}

/// Load the cache from disk
fn load_cache() -> Cache {
    let path = cache_path();
    if !path.exists() {
        return Cache::default();
    }

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Cache::default(),
    };

    match serde_json::from_str(&content) {
        Ok(cache) => cache,
        Err(_) => Cache::default(),
    }
}

/// Save the cache to disk
fn save_cache(cache: &Cache) {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(content) = serde_json::to_string(cache) {
        let _ = std::fs::write(&path, content);
    }
}

/// Current timestamp in seconds
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Fetch the latest versions for a list of package names.
/// Uses cache for recent results, queries the API for the rest.
pub async fn fetch_latest_versions(
    package_names: Vec<String>,
    tx: mpsc::UnboundedSender<ApiResult>,
) -> Result<()> {
    let now = now_secs();
    let cache = load_cache();
    let cache_valid = now.saturating_sub(cache.timestamp) < CACHE_TTL_SECS;

    // Separate cached packages from those needing a request
    let mut to_fetch = Vec::new();

    for name in &package_names {
        if cache_valid {
            if let Some(cached) = cache.entries.get(name) {
                let _ = tx.send(cached.clone());
                continue;
            }
        }
        to_fetch.push(name.clone());
    }

    if to_fetch.is_empty() {
        return Ok(());
    }

    // Query the API for non-cached packages
    let client = reqwest::Client::builder()
        .user_agent("nixup/0.1")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("Unable to create HTTP client")?;

    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_REQUESTS));
    let new_results = std::sync::Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    let mut handles = Vec::new();

    for name in to_fetch {
        let client = client.clone();
        let tx = tx.clone();
        let permit = semaphore.clone();
        let results = new_results.clone();

        let handle = tokio::spawn(async move {
            let _permit = permit.acquire().await;

            let api_result = match query_package(&client, &name).await {
                Ok(r) => r,
                Err(_) => ApiResult {
                    query_name: name,
                    version: None,
                    description: None,
                    homepage: None,
                },
            };

            // Store for cache
            results.lock().await.insert(api_result.query_name.clone(), api_result.clone());

            let _ = tx.send(api_result);
        });

        handles.push(handle);
    }

    for handle in handles {
        let _ = handle.await;
    }

    // Update the cache
    let fetched = new_results.lock().await;
    let mut updated_cache = if cache_valid { cache } else { Cache::default() };
    updated_cache.timestamp = now;
    for (name, result) in fetched.iter() {
        updated_cache.entries.insert(name.clone(), result.clone());
    }
    save_cache(&updated_cache);

    Ok(())
}

/// Query the Repology API for a single package
async fn query_package(client: &reqwest::Client, name: &str) -> Result<ApiResult> {
    let url = format!("{}/{}", REPOLOGY_API_URL, name);

    let response = client
        .get(&url)
        .send()
        .await
        .context("Error during API request")?;

    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        let response = client
            .get(&url)
            .send()
            .await
            .context("Error during API retry")?;
        return parse_response(response, name).await;
    }

    parse_response(response, name).await
}

/// Parse the Repology response and extract the nix_unstable entry
async fn parse_response(response: reqwest::Response, name: &str) -> Result<ApiResult> {
    let entries: Vec<RepologyEntry> = response
        .json()
        .await
        .context("Error parsing API response")?;

    // Look for the nix_unstable entry, fall back to nix_stable
    let nix_entry = entries
        .iter()
        .find(|e| e.repo == "nix_unstable")
        .or_else(|| entries.iter().find(|e| e.repo.starts_with("nix_stable")));

    let api_result = match nix_entry {
        Some(entry) => ApiResult {
            query_name: name.to_string(),
            version: entry.version.clone(),
            description: entry.summary.clone(),
            homepage: None,
        },
        None => ApiResult {
            query_name: name.to_string(),
            version: None,
            description: None,
            homepage: None,
        },
    };

    Ok(api_result)
}
