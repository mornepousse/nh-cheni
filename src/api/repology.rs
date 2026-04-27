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

// Rate-limit policy lives in `crate::http` so every HTTP path
// (repology, flake-input probe, release fetch) shares the same
// Retry-After behaviour.
use crate::http::parse_retry_after;

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
/// Uses the on-disk cache for packages that were recently looked up,
/// queries the Repology API for the rest with concurrency limiting.
///
/// Each input is `(name, Option<installed_version>)`. The optional
/// installed version is a disambiguation hint: when Repology returns
/// multiple nix entries for one project, the entry whose version
/// matches the hint wins over a `srcname` exact match. Handles the
/// kdePackages/libsForQt5 namespace collisions and the unrelated
/// "exo" packages (Xfce file manager vs LLM tool) that share a
/// Repology project name.
///
/// Pass `None` for callers that don't have an installed version
/// handy (e.g. a single-package lookup from `cheni pin <name>`).
pub async fn lookup_versions(
    packages: &[(String, Option<String>)],
) -> Result<Vec<PackageLookup>> {
    lookup_versions_with_progress(packages, None).await
}

/// Same as [`lookup_versions`], but lets the caller observe live
/// progress via an `AtomicUsize` counter. The counter is bumped once
/// for each cache hit (in bulk, as soon as the cache is loaded) and
/// once more each time a per-package API call resolves. The caller is
/// responsible for knowing the total (= `packages.len()`) and for
/// reading the counter from a separate thread to drive the UI.
pub async fn lookup_versions_with_progress(
    packages: &[(String, Option<String>)],
    resolved: Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>,
) -> Result<Vec<PackageLookup>> {
    use std::sync::atomic::Ordering;
    let cache = cache::load();
    let (mut results, to_fetch) = split_cache_hits(packages, &cache);

    // Cache hits resolve immediately — surface them to the progress
    // indicator so the counter doesn't sit at 0 during a fully-cached
    // run.
    if let Some(ref r) = resolved {
        r.fetch_add(results.len(), Ordering::Relaxed);
    }

    if to_fetch.is_empty() {
        debug!("All {} packages found in cache", packages.len());
        return Ok(results);
    }
    debug!("Cache: {} hits, {} misses", results.len(), to_fetch.len());

    let client = build_http_client()?;
    let handles = spawn_lookups(&client, to_fetch);
    let (fresh, updated_cache) = collect_and_merge(handles, &cache, resolved).await;
    results.extend(fresh);
    cache::save(&updated_cache);
    Ok(results)
}

/// Partition `packages` into (already-known cached hits, names still
/// needing an API query). The cache is keyed by package name — the
/// picked entry was version-hinted at write time, so we don't need
/// the installed version to look it up.
fn split_cache_hits(
    packages: &[(String, Option<String>)],
    cache: &cache::Cache,
) -> (Vec<PackageLookup>, Vec<(String, Option<String>)>) {
    let mut hits = Vec::new();
    let mut misses = Vec::new();
    for (name, installed) in packages {
        if let Some(cached) = cache.entries.get(name) {
            debug!("Cache hit: {}", name);
            hits.push(PackageLookup {
                name: name.clone(),
                version: cached.version.clone(),
                description: cached.description.clone(),
            });
        } else {
            misses.push((name.clone(), installed.clone()));
        }
    }
    (hits, misses)
}

/// Build the shared reqwest client. Timeout is generous by default
/// (30s) and overridable via $CHENI_HTTP_TIMEOUT for slow links.
///
/// User-Agent carries the live `git describe` so Repology can
/// distinguish cheni versions in its rate-limit policy. The previous
/// hardcoded `cheni/0.1` lasted from the prototype phase and got
/// blanket-blocked by Repology with HTTP 403 once enough installs
/// hammered them with that stale identifier — every `cheni check`
/// reported "Up to date: 0 | Unknown: N" silently because parse
/// errors on the HTML 403 body classified every package as unknown.
/// Same pattern as `release.rs` and `cmd::search`.
fn build_http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(crate::http::USER_AGENT)
        .timeout(crate::http::http_timeout())
        .build()
        .context("Failed to create HTTP client")
}

/// Fan out one tokio task per package, throttled by a
/// `MAX_CONCURRENT`-sized semaphore and staggered with jittered
/// per-batch delay so Repology isn't thundered all at once.
fn spawn_lookups(
    client: &reqwest::Client,
    to_fetch: Vec<(String, Option<String>)>,
) -> Vec<tokio::task::JoinHandle<Result<PackageLookup>>> {
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT));
    let mut handles = Vec::with_capacity(to_fetch.len());
    for (i, (name, installed)) in to_fetch.into_iter().enumerate() {
        let client = client.clone();
        let sem = semaphore.clone();

        // Stagger requests with jitter to avoid rate limiting + thundering herd.
        let batch_index = i as u64 / MAX_CONCURRENT as u64;
        let delay_ms = batch_index * BATCH_DELAY_MS + simple_jitter(i as u64);

        handles.push(tokio::spawn(async move {
            if delay_ms > 0 {
                tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;
            }
            let _permit = sem.acquire().await;
            query_one(&client, &name, installed.as_deref()).await
        }));
    }
    handles
}

/// Await every handle, accumulate successful lookups, and build the
/// updated cache snapshot.
///
/// Successful-with-version lookups replace/populate cache entries so
/// next run gets a hit. Unknown-version results (and outright task /
/// HTTP errors) are intentionally NOT cached — a 429 retry shouldn't
/// poison the cache with a "null" that masks the real answer forever.
async fn collect_and_merge(
    handles: Vec<tokio::task::JoinHandle<Result<PackageLookup>>>,
    prior: &cache::Cache,
    resolved: Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>,
) -> (Vec<PackageLookup>, cache::Cache) {
    use std::sync::atomic::Ordering;
    let mut fresh = Vec::new();
    // Preserve the prior timestamp when the cache was valid (non-zero) so that
    // a partial run — a few misses among many hits — does not silently extend
    // the TTL for every already-cached entry. Only assign a fresh timestamp
    // when prior.timestamp is 0, which is what `cache::load` returns for an
    // empty or expired cache. This prevents a single permanently-Unknown
    // package from infinitely deferring expiry for the rest of the cache.
    let mut new_cache = if prior.timestamp == 0 {
        cache::new_with_timestamp()
    } else {
        cache::Cache {
            timestamp: prior.timestamp,
            entries: std::collections::HashMap::new(),
        }
    };
    for (name, entry) in &prior.entries {
        new_cache.entries.insert(name.clone(), entry.clone());
    }
    for handle in handles {
        match handle.await {
            Ok(Ok(lookup)) => {
                if lookup.version.is_some() {
                    new_cache.entries.insert(
                        lookup.name.clone(),
                        CachedPackage {
                            version: lookup.version.clone(),
                            description: lookup.description.clone(),
                        },
                    );
                }
                fresh.push(lookup);
            }
            // Log at debug level — 429 retries are expected and noisy at WARN.
            Ok(Err(e)) => debug!("API error: {}", e),
            Err(e) => debug!("Task error: {}", e),
        }
        // Count every handle (success, API error, task error) — from
        // the user's point of view the item has been "processed" and
        // the indicator should keep moving even when the API is
        // flaking.
        if let Some(ref r) = resolved {
            r.fetch_add(1, Ordering::Relaxed);
        }
    }
    (fresh, new_cache)
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
        let retry_after_hdr = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok());
        let wait_secs = parse_retry_after(retry_after_hdr);
        debug!("Rate limited for '{}', retrying in {}s", name, wait_secs);
        tokio::time::sleep(tokio::time::Duration::from_secs(wait_secs)).await;

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

    // Defensive: any other non-2xx (403 from a UA-blocklist, 5xx,
    // captive-portal redirect, …) returns an HTML body that
    // `parse_response` would try to deserialise as JSON and fail.
    // Surface the real status in the debug log and return Unknown
    // — same outcome as 404, but the log line tells the user (or me
    // at debug time) what actually happened.
    if !response.status().is_success() {
        debug!(
            "HTTP {} from Repology for '{}' — classifying as Unknown",
            response.status(),
            name
        );
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
    crate::http::check_content_length(response.content_length(), crate::http::MAX_BODY_BYTES)?;
    let body = response
        .bytes()
        .await
        .with_context(|| format!("Reading response body for '{}' (HTTP {})", name, status))?;
    crate::http::verify_body_size(body.len(), crate::http::MAX_BODY_BYTES)?;
    let entries: Vec<RepologyEntry> = serde_json::from_slice(&body)
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
/// `repo`. Tries the most reliable signals first.
///
/// Why this matters: for the `firefox` project on Repology, nix_unstable
/// contributes ~8 entries — firefox (srcname=firefox, visiblename=firefox),
/// firefox-esr (srcname=firefox-esr, visiblename=firefox),
/// firefox-mobile (srcname=firefox-mobile, visiblename=firefox), etc.
/// visiblename matches several of them; only srcname (or the installed
/// version) disambiguates the actual nixpkgs `firefox` from siblings.
///
/// Cascade (when installed-version hint is provided):
/// 1. name match AND exact version — almost certainly the right one
/// 2. exact version alone — handles namespaced srcnames
///    (kdePackages.breeze-icons) where the bare name can't match
/// 3. major-version match alone — degraded fallback
///
/// Then (or when no hint):
/// 4. name match alone (srcname/binname/visiblename, in that order)
/// 5. first entry from the repo — last resort
fn pick_nix_entry<'a>(
    entries: &'a [RepologyEntry],
    match_names: &[&str],
    repo: &str,
    installed: Option<&str>,
) -> Option<&'a RepologyEntry> {
    let needles: Vec<String> = match_names.iter().map(|n| n.to_lowercase()).collect();
    let candidates: Vec<&RepologyEntry> =
        entries.iter().filter(|e| e.repo == repo).collect();

    let version_eq = |e: &&RepologyEntry, target: &str| e.version.as_deref() == Some(target);

    // Field-specific name match — used to break ties between entries whose
    // visiblename matches the query but whose srcname doesn't (e.g.
    // firefox-mobile vs firefox both have visiblename "firefox" but only
    // firefox's srcname exactly matches).
    let by_field = |entries: &[&'a RepologyEntry],
                    pred: &dyn Fn(&&RepologyEntry) -> bool|
     -> Option<&'a RepologyEntry> { entries.iter().copied().find(pred) };

    let by_srcname = |e: &&RepologyEntry| {
        needles.iter().any(|n| field_matches(&e.srcname, n))
    };
    let by_binname = |e: &&RepologyEntry| {
        needles.iter().any(|n| field_matches(&e.binname, n))
    };
    let by_visible = |e: &&RepologyEntry| {
        needles.iter().any(|n| field_matches(&e.visiblename, n))
    };

    if let Some(installed) = installed {
        // 1. exact version match, ordered by name-field specificity so
        //    `srcname=firefox` wins over `visiblename=firefox` from
        //    siblings (firefox-mobile, firefox-bin) at the same version.
        let same_ver: Vec<&RepologyEntry> = candidates
            .iter()
            .copied()
            .filter(|e| version_eq(e, installed))
            .collect();
        if let Some(e) = by_field(&same_ver, &by_srcname)
            .or_else(|| by_field(&same_ver, &by_binname))
            .or_else(|| by_field(&same_ver, &by_visible))
            .or_else(|| same_ver.first().copied())
        {
            return Some(e);
        }
        // 2. major version match alone (handles namespaced srcnames
        //    like kdePackages.breeze-icons that the bare name misses).
        if let Some(major) = parse_major(installed) {
            if let Some(e) = candidates
                .iter()
                .copied()
                .find(|e| e.version.as_deref().and_then(parse_major) == Some(major))
            {
                return Some(e);
            }
        }
    }

    // No installed-version hint or no version-based winner: name match
    // alone, again in srcname > binname > visiblename order.
    by_field(&candidates, &by_srcname)
        .or_else(|| by_field(&candidates, &by_binname))
        .or_else(|| by_field(&candidates, &by_visible))
        .or_else(|| candidates.first().copied())
}

/// Extract the major-version number from a version string ("3.14.3" → 3).
fn parse_major(version: &str) -> Option<u64> {
    version.split('.').next()?.parse().ok()
}

/// Case-insensitive equality between an Optional Repology field and a needle.
fn field_matches(field: &Option<String>, needle: &str) -> bool {
    field.as_deref().map(|s| s.to_lowercase()) == Some(needle.to_string())
}

/// True when any of the entry's name fields equals any of the needles.
/// `needles` are expected to be already lowercase.
fn entry_matches_any(entry: &RepologyEntry, needles: &[String]) -> bool {
    needles.iter().any(|n| {
        field_matches(&entry.srcname, n)
            || field_matches(&entry.binname, n)
            || field_matches(&entry.visiblename, n)
    })
}

/// Wrapper kept for the nix_stable fallback in parse_response — takes a
/// single name instead of pre-lowercased needles.
fn entry_name_matches(entry: &RepologyEntry, name: &str) -> bool {
    entry_matches_any(entry, &[name.to_lowercase()])
}

#[cfg(test)]
#[path = "tests/repology.rs"]
mod tests;
