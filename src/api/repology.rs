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

/// Nix package names that differ from Repology project names.
/// Maps nix_name → repology_name.
/// Maintained by the community — add mappings as discovered.
const NAME_MAPPINGS: &[(&str, &str)] = &[
    // LLVM/Clang — all part of the "llvm" project
    ("clang", "llvm"),
    ("clang-tools", "llvm"),
    ("lldb", "llvm"),
    // Python
    ("python3", "python"),
    // Terminal emulators / tools with different Repology names
    ("kitty", "kitty-terminal"),
    ("mako", "mako-notifier"),
    // Fonts — Repology's `fonts:noto` tracks the meta-bundle (calver,
    // e.g. "2026.04.01"), but the noto-fonts-* sub-packages in nixpkgs
    // ship with their own per-script versions ("2.004", "2.051", ...).
    // Mapping the sub-packages to fonts:noto produced phantom updates
    // every time the bundle was tagged. Removed — they fall through to
    // the input-name lookup, which simply doesn't exist on Repology
    // for these specific sub-packages, classified as "Unknown".
    // Qt 6 — split across many sub-modules in nixpkgs but tracked under "qt"
    ("qtbase", "qt"),
    ("qtcharts", "qt"),
    ("qtconnectivity", "qt"),
    ("qtdeclarative", "qt"),
    ("qtmultimedia", "qt"),
    ("qtnetworkauth", "qt"),
    ("qtquick3d", "qt"),
    ("qtremoteobjects", "qt"),
    ("qtscxml", "qt"),
    ("qtsensors", "qt"),
    ("qtserialbus", "qt"),
    ("qtserialport", "qt"),
    ("qtshadertools", "qt"),
    ("qtspeech", "qt"),
    ("qtsvg", "qt"),
    ("qttools", "qt"),
    ("qttranslations", "qt"),
    ("qtvirtualkeyboard", "qt"),
    ("qtwayland", "qt"),
    ("qtwebchannel", "qt"),
    ("qtwebengine", "qt"),
    ("qtwebsockets", "qt"),
    // Others
    ("discover", "plasma-discover"),
];

/// Result of looking up a package version.
#[derive(Debug, Clone)]
pub struct PackageLookup {
    pub name: String,
    pub version: Option<String>,
    pub description: Option<String>,
}

/// Repology API response entry.
///
/// One Repology *project* (e.g. "firefox") can contain many entries
/// per repo (firefox, firefox-esr, firefox-bin, firefox-unwrapped...).
/// We need binname/srcname to pick the entry that matches the package
/// name we actually queried with — without it, picking the first
/// `nix_unstable` entry gives e.g. firefox-esr 140 when the user
/// actually has firefox 149.
#[derive(Debug, Deserialize)]
struct RepologyEntry {
    #[serde(default)]
    repo: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    /// Package name in the repo (e.g. "firefox-esr"). Optional in the
    /// API response — only present for binary repos.
    #[serde(default)]
    binname: Option<String>,
    /// Source package name (e.g. "firefox-unwrapped"). Optional.
    #[serde(default)]
    srcname: Option<String>,
    /// Display name. Always present in practice.
    #[serde(default)]
    visiblename: Option<String>,
}

/// Look up versions for a list of packages.
///
/// Uses the cache for packages that were recently looked up.
/// Queries the Repology API for the rest, with concurrency limiting.
///
/// Backwards-compat wrapper. Prefer `lookup_versions_with_installed`
/// when you have the user's installed versions — it disambiguates
/// Repology projects that contain multiple nix entries (e.g.
/// `breeze-icons` has both `kdePackages.breeze-icons` 6.x and
/// `libsForQt5.breeze-icons` 5.x).
pub async fn lookup_versions(names: &[String]) -> Result<Vec<PackageLookup>> {
    let with_hints: Vec<(String, Option<String>)> =
        names.iter().map(|n| (n.clone(), None)).collect();
    lookup_versions_with_installed(&with_hints).await
}

/// Same as `lookup_versions` but each input carries its installed
/// version as a disambiguation hint. When Repology returns multiple
/// nix entries for one project, the entry whose version matches the
/// hint is preferred over a `srcname` exact match — handles the
/// kdePackages/libsForQt5 namespace collisions and the unrelated
/// "exo" packages (Xfce vs LLM tool) that share a project name.
pub async fn lookup_versions_with_installed(
    packages: &[(String, Option<String>)],
) -> Result<Vec<PackageLookup>> {
    let cache = cache::load();
    let mut results = Vec::new();
    let mut to_fetch: Vec<(String, Option<String>)> = Vec::new();

    // Check cache first — keyed by package name (cached entries don't
    // depend on installed version since the picked entry was version-
    // hinted at write time too).
    for (name, installed) in packages {
        if let Some(cached) = cache.entries.get(name) {
            debug!("Cache hit: {}", name);
            results.push(PackageLookup {
                name: name.clone(),
                version: cached.version.clone(),
                description: cached.description.clone(),
            });
        } else {
            to_fetch.push((name.clone(), installed.clone()));
        }
    }

    if to_fetch.is_empty() {
        debug!("All {} packages found in cache", packages.len());
        return Ok(results);
    }

    debug!("Cache: {} hits, {} misses", results.len(), to_fetch.len());

    // Query the API for cache misses. Timeout is generous by default
    // (30s) and overridable via $CHENI_HTTP_TIMEOUT for slow links.
    let client = reqwest::Client::builder()
        .user_agent("cheni/0.1")
        .timeout(super::net::http_timeout())
        .build()
        .context("Failed to create HTTP client")?;

    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT));
    let mut handles = Vec::new();

    for (i, (name, installed)) in to_fetch.into_iter().enumerate() {
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
            query_one(&client, &name, installed.as_deref()).await
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
                // Only cache successful lookups — don't cache unknowns
                // so they get retried next time (might have been rate-limited)
                if lookup.version.is_some() {
                    new_cache.entries.insert(lookup.name.clone(), CachedPackage {
                        version: lookup.version.clone(),
                        description: lookup.description.clone(),
                    });
                }
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

/// Look up the Repology name for a Nix package.
/// Returns the mapped name if one exists, otherwise the original.
fn repology_name(nix_name: &str) -> &str {
    for (nix, repology) in NAME_MAPPINGS {
        if *nix == nix_name {
            return repology;
        }
    }
    nix_name
}

/// Query the Repology API for a single package.
///
/// Uses name mapping to translate Nix names to Repology names.
/// Retries once on 429 (rate limited) with a 3-second wait.
/// Returns unknown version on persistent failure to avoid noisy errors.
async fn query_one(
    client: &reqwest::Client,
    name: &str,
    installed: Option<&str>,
) -> Result<PackageLookup> {
    let lookup_name = repology_name(name);
    let url = format!("{}/{}", API_URL, lookup_name);
    trace!("HTTP GET {} (nix: {})", url, name);
    // Names to try when filtering nix entries inside the project page —
    // both the original Nix name and the mapped Repology name. For
    // identity mappings these are the same; for real mappings (e.g.
    // python3 → python) we want the second so the matcher actually
    // hits the nix_unstable entry whose srcname is "python".
    let match_names: Vec<&str> = if name == lookup_name {
        vec![name]
    } else {
        vec![name, lookup_name]
    };

    let response = client
        .get(&url)
        .send()
        .await
        .context("API request failed")?;

    // Handle rate limiting with a single retry
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        debug!("Rate limited for '{}', retrying in {}s", name, RATE_LIMIT_RETRY_SECS);
        tokio::time::sleep(tokio::time::Duration::from_secs(RATE_LIMIT_RETRY_SECS)).await;

        let retry_response = client.get(&url).send().await;
        match retry_response {
            Ok(resp) if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS => {
                // Second 429 — give up silently and return unknown
                debug!("Rate limited again for '{}', returning unknown", name);
                return Ok(PackageLookup {
                    name: name.to_string(),
                    version: None,
                    description: None,
                });
            }
            Ok(resp) => return parse_response(resp, name, &match_names, installed).await,
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

    // Handle 404 (package not found)
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        debug!("Package '{}' not found on Repology", name);
        return Ok(PackageLookup {
            name: name.to_string(),
            version: None,
            description: None,
        });
    }

    parse_response(response, name, &match_names, installed).await
}

/// Simple jitter based on the package index.
/// Returns a value in milliseconds between 0 and JITTER_MAX_MS.
/// No need for true randomness -- just spreading requests out.
fn simple_jitter(index: u64) -> u64 {
    // Use a simple hash of the index + timestamp for pseudo-randomness
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;

    let mixed = index.wrapping_mul(6364136223846793005).wrapping_add(now);
    mixed % (JITTER_MAX_MS + 1)
}

/// Parse the Repology response and find the matching nix_unstable entry.
///
/// The Repology project page (e.g. /api/v1/project/firefox) lists every
/// repo + package combination that maps to that project. nixpkgs alone
/// often appears multiple times — for "firefox" you get firefox,
/// firefox-esr, firefox-bin, firefox-unwrapped... — and picking the
/// first one shows e.g. ESR 140 for a user who actually has 149.
///
/// Selection logic:
///   1. nix_unstable entry whose binname / srcname / visiblename matches
///      the queried package name exactly (e.g. "firefox" → firefox 149).
///   2. nix_unstable entry without a name field (older Repology data).
///   3. Any nix_unstable entry (fallback for projects with one entry).
///   4. nix_stable equivalent of the same cascade.
async fn parse_response(
    response: reqwest::Response,
    name: &str,
    match_names: &[&str],
    installed: Option<&str>,
) -> Result<PackageLookup> {
    let status = response.status();
    let entries: Vec<RepologyEntry> = response
        .json()
        .await
        .with_context(|| format!("Failed to parse response for '{}' (HTTP {})", name, status))?;

    trace!("Repology returned {} entries for '{}'", entries.len(), name);

    let nix_entry = pick_nix_entry(&entries, match_names, "nix_unstable", installed).or_else(|| {
        entries
            .iter()
            .filter(|e| e.repo.starts_with("nix_stable"))
            .find(|e| match_names.iter().any(|n| entry_name_matches(e, n)))
            .or_else(|| entries.iter().find(|e| e.repo.starts_with("nix_stable")))
    });

    let lookup = match nix_entry {
        Some(entry) => {
            debug!(
                "API: {} → {} ({} / {})",
                name,
                entry.version.as_deref().unwrap_or("?"),
                entry.repo,
                entry
                    .binname
                    .as_deref()
                    .or(entry.srcname.as_deref())
                    .or(entry.visiblename.as_deref())
                    .unwrap_or("?"),
            );
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

/// Pick the best Repology entry for `name` from `entries`, scoped to one
/// `repo`. Tries the most reliable fields first.
///
/// Why this ordering matters: for the `firefox` project on Repology,
/// nix_unstable contributes ~8 entries — firefox (srcname=firefox,
/// visiblename=firefox), firefox-esr (srcname=firefox-esr,
/// visiblename=firefox), firefox-mobile (srcname=firefox-mobile,
/// visiblename=firefox), and so on. visiblename matches several
/// of them; only srcname disambiguates "the actual `firefox`
/// derivation in nixpkgs" from "another entry that's tagged as
/// firefox in the UI".
fn pick_nix_entry<'a>(
    entries: &'a [RepologyEntry],
    match_names: &[&str],
    repo: &str,
    installed: Option<&str>,
) -> Option<&'a RepologyEntry> {
    let needles: Vec<String> = match_names.iter().map(|n| n.to_lowercase()).collect();
    let in_repo = || entries.iter().filter(|e| e.repo == repo);

    let name_matches = |e: &RepologyEntry| {
        let any = |f: &Option<String>| needles.iter().any(|n| field_matches(f, n));
        any(&e.srcname) || any(&e.binname) || any(&e.visiblename)
    };

    // Disambiguation pass when we have an installed version. Repology's
    // project pages often contain unrelated nix entries that share a
    // visible/srcname (exo LLM vs xfce4-exo, kdePackages vs libsForQt5).
    // Picking purely by srcname picks the wrong one in those cases.
    if let Some(installed) = installed {
        // 1. Name-matched entry whose version equals installed exactly —
        //    almost always the right package.
        if let Some(e) = in_repo()
            .filter(|e| name_matches(e))
            .find(|e| e.version.as_deref() == Some(installed))
        {
            return Some(e);
        }
        // 2. Any entry whose version equals installed exactly. For
        //    namespaced srcnames like "kdePackages.breeze-icons" the
        //    bare-name match misses but the version is conclusive.
        if let Some(e) = in_repo().find(|e| e.version.as_deref() == Some(installed)) {
            return Some(e);
        }
        // 3. Any entry whose major version matches installed.
        let installed_major = installed
            .split('.')
            .next()
            .and_then(|s| s.parse::<u64>().ok());
        if let Some(major) = installed_major {
            if let Some(e) = in_repo().find(|e| {
                e.version
                    .as_deref()
                    .and_then(|v| v.split('.').next())
                    .and_then(|s| s.parse::<u64>().ok())
                    == Some(major)
            }) {
                return Some(e);
            }
        }
    }

    // No installed-version hint or no version-based winner: name matching.
    let any = |getter: fn(&RepologyEntry) -> &Option<String>| {
        in_repo().find(|e| needles.iter().any(|n| field_matches(getter(e), n)))
    };
    any(|e| &e.srcname)
        .or_else(|| any(|e| &e.binname))
        .or_else(|| any(|e| &e.visiblename))
        .or_else(|| in_repo().next())
}

fn field_matches(field: &Option<String>, needle: &str) -> bool {
    field
        .as_deref()
        .map(|s| s.to_lowercase() == needle)
        .unwrap_or(false)
}

/// Backwards-compat helper used by the nix_stable fallback path.
fn entry_name_matches(entry: &RepologyEntry, name: &str) -> bool {
    let needle = name.to_lowercase();
    field_matches(&entry.srcname, &needle)
        || field_matches(&entry.binname, &needle)
        || field_matches(&entry.visiblename, &needle)
}
