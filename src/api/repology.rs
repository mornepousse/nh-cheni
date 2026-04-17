//! Repology API client.
//!
//! Queries https://repology.org/api/v1/project/<name> to find
//! the latest version of a package on nixos-unstable.
//!
//! Rate-limited to 2 concurrent requests with automatic retry on 429.

use anyhow::{Context, Result};
use serde::Deserialize;
use tracing::{debug, trace};

use super::cache::{self, CachedPackage};

/// Maximum number of concurrent API requests.
/// Repology rate-limits aggressively, so we keep this very low.
const MAX_CONCURRENT: usize = 2;

/// Delay between batches of requests (in milliseconds).
const BATCH_DELAY_MS: u64 = 500;

/// Wait time after a 429 response before retrying (in seconds).
const RATE_LIMIT_RETRY_SECS: u64 = 3;

/// Maximum random jitter added to batch delay (in milliseconds).
/// Avoids thundering herd when multiple instances run concurrently.
const JITTER_MAX_MS: u64 = 200;

/// Repology API base URL.
const API_URL: &str = "https://repology.org/api/v1/project";

/// Result of looking up a package version.
#[derive(Debug, Clone)]
pub struct PackageLookup {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
}

/// Repology API response entry.
#[derive(Debug, Deserialize)]
struct RepologyEntry {
    #[serde(default)]
    repo: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    summary: Option<String>,
}

/// Look up versions for a list of packages.
///
/// Uses the cache for packages that were recently looked up.
/// Queries the Repology API for the rest, with concurrency limiting.
pub async fn lookup_versions(names: &[String]) -> Result<Vec<PackageLookup>> {
    let cache = cache::load();
    let mut results = Vec::new();
    let mut to_fetch = Vec::new();

    // Check cache first
    for name in names {
        if let Some(cached) = cache.entries.get(name) {
            debug!("Cache hit: {}", name);
            results.push(PackageLookup {
                name: name.clone(),
                version: cached.version.clone(),
                description: cached.description.clone(),
            });
        } else {
            to_fetch.push(name.clone());
        }
    }

    if to_fetch.is_empty() {
        debug!("All {} packages found in cache", names.len());
        return Ok(results);
    }

    debug!("Cache: {} hits, {} misses", results.len(), to_fetch.len());

    // Query the API for cache misses
    let client = reqwest::Client::builder()
        .user_agent("nixup/0.1")
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("Failed to create HTTP client")?;

    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT));
    let mut handles = Vec::new();

    for (i, name) in to_fetch.into_iter().enumerate() {
        let client = client.clone();
        let sem = semaphore.clone();

        // Stagger requests with jitter to avoid rate limiting and thundering herd
        let batch_index = i as u64 / MAX_CONCURRENT as u64;
        let base_delay_ms = batch_index * BATCH_DELAY_MS;
        let jitter_ms = simple_jitter(i as u64);
        let delay_ms = base_delay_ms + jitter_ms;

        let handle = tokio::spawn(async move {
            if delay_ms > 0 {
                tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
            }
            let _permit = sem.acquire().await;
            query_one(&client, &name).await
        });

        handles.push(handle);
    }

    // Collect results and update cache
    let mut new_cache = cache::new_with_timestamp();

    // Preserve existing cache entries
    for (name, entry) in &cache.entries {
        new_cache.entries.insert(name.clone(), entry.clone());
    }

    for handle in handles {
        match handle.await {
            Ok(Ok(lookup)) => {
                new_cache.entries.insert(lookup.name.clone(), CachedPackage {
                    version: lookup.version.clone(),
                    description: lookup.description.clone(),
                });
                results.push(lookup);
            }
            Ok(Err(e)) => {
                // Log at debug level — 429 retries are expected and noisy at WARN
                debug!("API error: {}", e);
            }
            Err(e) => {
                debug!("Task error: {}", e);
            }
        }
    }

    cache::save(&new_cache);
    Ok(results)
}

/// Query the Repology API for a single package.
///
/// Retries once on 429 (rate limited) with a 3-second wait.
/// Returns unknown version on persistent failure to avoid noisy errors.
async fn query_one(client: &reqwest::Client, name: &str) -> Result<PackageLookup> {
    let url = format!("{}/{}", API_URL, name);
    trace!("HTTP GET {}", url);

    let response = client
        .get(&url)
        .send()
        .await
        .context("API request failed")?;

    // Gérer le rate limiting avec un seul retry
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        debug!("Rate limited for '{}', retrying in {}s", name, RATE_LIMIT_RETRY_SECS);
        tokio::time::sleep(tokio::time::Duration::from_secs(RATE_LIMIT_RETRY_SECS)).await;

        let retry_response = client.get(&url).send().await;
        match retry_response {
            Ok(resp) if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS => {
                // Deuxième 429 — abandonner silencieusement et retourner "inconnu"
                debug!("Rate limited again for '{}', returning unknown", name);
                return Ok(PackageLookup {
                    name: name.to_string(),
                    version: None,
                    description: None,
                });
            }
            Ok(resp) => return parse_response(resp, name).await,
            Err(e) => {
                debug!("Retry failed for '{}': {}", name, e);
                return Ok(PackageLookup {
                    name: name.to_string(),
                    version: None,
                    description: None,
                });
            }
        }
    }

    // Gérer 404 (paquet introuvable)
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        debug!("Package '{}' not found on Repology", name);
        return Ok(PackageLookup {
            name: name.to_string(),
            version: None,
            description: None,
        });
    }

    parse_response(response, name).await
}

/// Simple jitter basé sur l'index du paquet.
/// Retourne une valeur en millisecondes entre 0 et JITTER_MAX_MS.
/// Pas besoin de vrai aléa — on veut juste étaler les requêtes.
fn simple_jitter(index: u64) -> u64 {
    // Utiliser un hash simple de l'index + timestamp pour du pseudo-aléatoire
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;

    let mixed = index.wrapping_mul(6364136223846793005).wrapping_add(now);
    mixed % (JITTER_MAX_MS + 1)
}

/// Parse the Repology response and find the nix_unstable entry.
async fn parse_response(response: reqwest::Response, name: &str) -> Result<PackageLookup> {
    let status = response.status();
    let entries: Vec<RepologyEntry> = response
        .json()
        .await
        .with_context(|| format!("Failed to parse response for '{}' (HTTP {})", name, status))?;

    trace!("Repology returned {} entries for '{}'", entries.len(), name);

    // Look for nix_unstable first, fall back to nix_stable
    let nix_entry = entries
        .iter()
        .find(|e| e.repo == "nix_unstable")
        .or_else(|| entries.iter().find(|e| e.repo.starts_with("nix_stable")));

    let lookup = match nix_entry {
        Some(entry) => {
            debug!("API: {} → {} ({})", name, entry.version.as_deref().unwrap_or("?"), entry.repo);
            PackageLookup {
                name: name.to_string(),
                version: entry.version.clone(),
                description: entry.summary.clone(),
            }
        }
        None => {
            debug!("API: {} → not found in nix repos", name);
            PackageLookup {
                name: name.to_string(),
                version: None,
                description: None,
            }
        }
    };

    Ok(lookup)
}
