//! Release tarball verification.
//!
//! cheni releases are signed with minisign. The trusted public key is
//! embedded in the binary at compile time from
//! `public-keys/cheni-release.pub`, so both `cheni self-update` and
//! `cheni verify` share the same trust anchor without reading from
//! disk at runtime.
//!
//! The module exposes two layers:
//!
//! - Pure helpers (`tarball_url`, `signature_url`, `verify_bytes`,
//!   `strip_dev_suffix`) — easy to unit-test in isolation.
//! - A `verify_release` orchestrator that downloads, verifies, and
//!   returns a structured `VerifyReport` suitable for the verify CLI
//!   and for self-update's gating decision.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use minisign_verify::{PublicKey, Signature};
use tracing::debug;

use crate::http;

/// Minisign public key that signs every cheni release. Loaded at
/// compile time so the binary and its trust anchor ship together.
pub const RELEASE_PUBKEY: &str = include_str!("../public-keys/cheni-release.pub");

/// Per-request HTTP ceiling on top of the project-wide timeout. Release
/// assets are tiny (~150 KB for the tarball, a few hundred bytes for
/// the signature) — a minute is comfortably larger than any sane
/// network hiccup but small enough that a stuck call fails loudly.
const RELEASE_FETCH_TIMEOUT_SECS: u64 = 60;

/// Structured outcome of a successful release verification.
///
/// The verify CLI turns this into a user-facing report; self-update
/// uses it to decide whether to call `nh os switch`.
#[derive(Debug, Clone)]
pub struct VerifyReport {
    /// The release tag (e.g. `v0.2.0`) whose signature was verified.
    pub tag: String,
    /// Size of the downloaded tarball in bytes.
    pub tarball_bytes: usize,
    /// The `trusted comment` from the signature — typically something
    /// like `"cheni v0.2.0 release"`. Set by the release-manager agent
    /// at sign time; we expose it for audit display.
    pub trusted_comment: String,
}

/// GitLab auto-archive URL for `tag`. Matches exactly what Nix fetches
/// when the flake input is `gitlab:harrael/cheni/<tag>`, so signing
/// this URL's output covers the `nix flake update` path too.
pub fn tarball_url(tag: &str) -> String {
    format!(
        "https://gitlab.com/harrael/cheni/-/archive/{tag}/cheni-{tag}.tar.gz",
        tag = tag
    )
}

/// URL of the `.minisig` release asset. Matches what
/// `glab release create <tag> cheni-<tag>.tar.gz.minisig` publishes.
pub fn signature_url(tag: &str) -> String {
    format!(
        "https://gitlab.com/harrael/cheni/-/releases/{tag}/downloads/cheni-{tag}.tar.gz.minisig",
        tag = tag
    )
}

/// Strip the dev-build suffix (`-N-gHASH[-dirty]`) that `git describe`
/// appends on commits past a tag, so a runtime `GIT_DESCRIBE` like
/// `v0.1.0-beta-5-gabcdef-dirty` resolves back to the tag
/// `v0.1.0-beta` we can verify against. Returns the input unchanged
/// when there's no dev suffix to strip.
///
/// The heuristic matches the exact shape `-N-g<hexhash>` (N decimal,
/// hash hex) so it doesn't accidentally truncate pre-release suffixes
/// like `-beta` or `-rc1`. The trailing `-dirty` flag is stripped
/// first when present.
pub fn strip_dev_suffix(describe: &str) -> String {
    let trimmed = describe.strip_suffix("-dirty").unwrap_or(describe);
    match try_strip_commit_suffix(trimmed) {
        Some(stripped) => stripped.to_string(),
        None => trimmed.to_string(),
    }
}

/// Returns `Some(&tag)` when `s` matches `<tag>-<N>-g<hex>` with `N`
/// decimal and `<hex>` a non-empty hex run. `None` otherwise — which
/// covers every non-dev shape (exact tag, `-beta`, `-rc1`, etc.).
fn try_strip_commit_suffix(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    // Hash run: walk back over ASCII hex digits.
    let mut i = bytes.len();
    while i > 0 && bytes[i - 1].is_ascii_hexdigit() {
        i -= 1;
    }
    // Hash must be non-empty and preceded by `-g`.
    if i == bytes.len() || i < 2 || bytes[i - 1] != b'g' || bytes[i - 2] != b'-' {
        return None;
    }
    // Commit-count run: walk back over ASCII digits, starting just
    // before the `-g`.
    let mut j = i - 2;
    let digits_end = j;
    while j > 0 && bytes[j - 1].is_ascii_digit() {
        j -= 1;
    }
    // Digits must be non-empty and preceded by `-`.
    if j == digits_end || j == 0 || bytes[j - 1] != b'-' {
        return None;
    }
    // Everything before that final `-` is the tag.
    Some(&s[..j - 1])
}

/// Pure verification core — decode the public key and signature text,
/// check `payload` against them. Kept IO-free so fixtures-based tests
/// cover the crypto path without touching disk or network.
pub fn verify_bytes(pubkey_text: &str, payload: &[u8], signature_text: &str) -> Result<()> {
    let pubkey = PublicKey::decode(pubkey_text.trim())
        .map_err(|e| anyhow!("decoding embedded public key: {}", e))?;
    let signature = Signature::decode(signature_text.trim())
        .map_err(|e| anyhow!("decoding signature file: {}", e))?;
    pubkey
        .verify(payload, &signature, false)
        .map_err(|e| anyhow!("signature check: {}", e))?;
    Ok(())
}

/// Extract the `trusted comment:` line from a `.minisig` file. Returns
/// an empty string when absent so a missing comment is never a hard
/// error — the signature itself is what we gate on.
pub fn extract_trusted_comment(signature_text: &str) -> String {
    const MARKER: &str = "trusted comment:";
    for line in signature_text.lines() {
        if let Some(rest) = line.strip_prefix(MARKER) {
            return rest.trim().to_string();
        }
    }
    String::new()
}

/// Download the release tarball + its `.minisig` for `tag`, verify
/// against `RELEASE_PUBKEY`, and return a `VerifyReport`. Network
/// errors, verification failures, and schema issues all return `Err`.
///
/// Async because callers (`cmd/verify`, `cmd/self_update`) run inside
/// the tokio runtime — using `reqwest::blocking` from there crashes
/// at drop ("Cannot drop a runtime in a context where blocking is not
/// allowed"). The sync pure helpers (`verify_bytes`, `tarball_url`,
/// etc.) stay callable on their own.
pub async fn verify_release(tag: &str) -> Result<VerifyReport> {
    let tarball_url = tarball_url(tag);
    let signature_url = signature_url(tag);

    debug!("fetching tarball: {}", tarball_url);
    debug!("fetching signature: {}", signature_url);

    let client = reqwest::Client::builder()
        .timeout(http::http_timeout())
        .user_agent(concat!("cheni/", env!("GIT_DESCRIBE")))
        .build()
        .context("building HTTP client")?;

    let tarball = fetch_bounded(&client, &tarball_url)
        .await
        .with_context(|| format!("fetching {}", tarball_url))?;
    let signature_text = fetch_signature_text(&client, &signature_url)
        .await
        .with_context(|| format!("fetching {}", signature_url))?;

    verify_bytes(RELEASE_PUBKEY, &tarball, &signature_text)?;

    Ok(VerifyReport {
        tag: tag.to_string(),
        tarball_bytes: tarball.len(),
        trusted_comment: extract_trusted_comment(&signature_text),
    })
}

/// Download bytes with the shared HTTP body cap from `crate::http`.
async fn fetch_bounded(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .timeout(Duration::from_secs(RELEASE_FETCH_TIMEOUT_SECS))
        .send()
        .await
        .context("HTTP send")?;
    if !response.status().is_success() {
        anyhow::bail!("HTTP {} fetching {}", response.status(), url);
    }
    http::check_content_length(response.content_length(), http::MAX_BODY_BYTES)?;
    let body = response.bytes().await.context("reading body")?;
    http::verify_body_size(body.len(), http::MAX_BODY_BYTES)?;
    Ok(body.to_vec())
}

async fn fetch_signature_text(client: &reqwest::Client, url: &str) -> Result<String> {
    let bytes = fetch_bounded(client, url).await?;
    String::from_utf8(bytes).context("signature file is not valid UTF-8")
}

/// GitLab API endpoint listing the most-recent cheni tags. 20 entries
/// is well above any realistic gap between the user's pin and the
/// current latest, while staying small enough to fit in one cheap
/// request.
const GITLAB_TAGS_URL: &str =
    "https://gitlab.com/api/v4/projects/harrael%2Fcheni/repository/tags?per_page=20";

/// Query GitLab for the latest released cheni tag.
///
/// Filters the most-recent 20 tags down to release-shaped names
/// (`vX.Y.Z` or `vX.Y.Z-suffix`) and returns the highest one by
/// version comparison. `Err` on HTTP failures or an empty release
/// set — callers fall back to "leave the current pin alone" rather
/// than blowing up self-update for a transient API hiccup.
pub async fn latest_release_tag() -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(http::http_timeout())
        .user_agent(concat!("cheni/", env!("GIT_DESCRIBE")))
        .build()
        .context("building HTTP client")?;

    let resp = client
        .get(GITLAB_TAGS_URL)
        .send()
        .await
        .context("querying GitLab tags API")?;
    if !resp.status().is_success() {
        anyhow::bail!("GitLab tags API returned HTTP {}", resp.status());
    }
    let body = resp.text().await.context("reading tags response body")?;
    pick_latest_tag(&body)
}

/// File name used inside `~/.cache/cheni/` for the self-update tag check.
const SELF_UPDATE_CHECK_CACHE_FILE: &str = "self-update-check.json";

/// Cache TTL for the self-update tag check (24h). The check is a
/// passive UX hint — checking GitLab once a day is plenty, hammering
/// the API on every `cheni check` would just be rude.
const SELF_UPDATE_CHECK_TTL_SECS: u64 = 86_400;

/// On-disk shape of the self-update tag cache.
#[derive(serde::Serialize, serde::Deserialize)]
struct CachedSelfUpdateCheck {
    latest: String,
    fetched_at: u64,
}

/// Cached wrapper around [`latest_release_tag`]. Returns the cached
/// answer when fresh (under 24h), otherwise hits GitLab and refreshes
/// the cache. Failures to read/write the cache file are silently
/// ignored — the call still works, just without persistence.
///
/// Intended for the "is a newer cheni shipped?" hint in `cheni check`,
/// not for the self-update flow itself: that one needs the live answer
/// at decision time and uses [`latest_release_tag`] directly.
pub async fn latest_release_tag_cached() -> Result<String> {
    if let Some(cached) = read_self_update_check_cache() {
        debug!("self-update check: cache hit ({})", cached.latest);
        return Ok(cached.latest);
    }
    let latest = latest_release_tag().await?;
    write_self_update_check_cache(&latest);
    Ok(latest)
}

/// Resolve the self-update cache file path, or `None` when the user
/// has no cache directory (rare — typically a misconfigured XDG env
/// or a chroot without HOME).
fn self_update_check_cache_path() -> Option<std::path::PathBuf> {
    dirs::cache_dir().map(|d| d.join("cheni").join(SELF_UPDATE_CHECK_CACHE_FILE))
}

/// Read the cache and return `Some` only when the entry is still fresh.
/// Stale, missing, or unparseable entries return `None` — caller falls
/// back to the live API.
fn read_self_update_check_cache() -> Option<CachedSelfUpdateCheck> {
    let path = self_update_check_cache_path()?;
    let content = std::fs::read_to_string(&path).ok()?;
    let entry: CachedSelfUpdateCheck = serde_json::from_str(&content).ok()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    if now > entry.fetched_at.saturating_add(SELF_UPDATE_CHECK_TTL_SECS) {
        debug!("self-update check: cache expired");
        return None;
    }
    Some(entry)
}

/// Persist a fresh self-update tag answer. Atomic write via
/// `util::atomic_write` so a concurrent reader never sees a half-file.
/// Best-effort: directory-creation and write failures are swallowed.
fn write_self_update_check_cache(latest: &str) {
    let Some(path) = self_update_check_cache_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let entry = CachedSelfUpdateCheck {
        latest: latest.to_string(),
        fetched_at: now,
    };
    if let Ok(json) = serde_json::to_string(&entry) {
        let _ = crate::util::atomic_write(&path, &json);
    }
}

/// Pure half of `latest_release_tag` — pick the highest release tag
/// from a JSON tag-list payload. Extracted so the version-picking
/// logic is tested with hand-rolled fixtures rather than a live API.
pub(crate) fn pick_latest_tag(body: &str) -> Result<String> {
    let tags: serde_json::Value =
        serde_json::from_str(body).context("parsing GitLab tags response")?;
    let arr = tags
        .as_array()
        .ok_or_else(|| anyhow!("GitLab tags response is not a JSON array"))?;

    let mut candidates: Vec<&str> = arr
        .iter()
        .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
        .filter(|n| is_release_tag(n))
        .collect();
    if candidates.is_empty() {
        anyhow::bail!("no release-shaped tags returned by GitLab");
    }

    // Highest-version-first ordering, with an explicit tie-break that
    // prefers stable (no `-suffix`) over pre-release at the same
    // numeric: if v0.5.0 and v0.5.0-rc1 both ship, self-update should
    // recommend the GA tag, not the release candidate.
    candidates.sort_by(|a, b| {
        let va = crate::version::parse::parse_version(a.strip_prefix('v').unwrap_or(a));
        let vb = crate::version::parse::parse_version(b.strip_prefix('v').unwrap_or(b));
        vb.cmp(&va).then_with(|| {
            match (has_prerelease_suffix(a), has_prerelease_suffix(b)) {
                (false, true) => std::cmp::Ordering::Less,
                (true, false) => std::cmp::Ordering::Greater,
                _ => a.cmp(b),
            }
        })
    });
    Ok(candidates[0].to_string())
}

/// True when a release tag carries a pre-release suffix (`-beta`,
/// `-rc1`, …). Used as the tie-breaker in `pick_latest_tag`.
fn has_prerelease_suffix(tag: &str) -> bool {
    tag.strip_prefix('v').unwrap_or(tag).contains('-')
}

/// True for a tag name shaped like a cheni release.
///
/// Accepts `vX.Y.Z` and `vX.Y.Z-<suffix>` (alpha/beta/rc/etc.). Used
/// to filter the GitLab tag list so non-release tags (preview branches,
/// debug markers) don't bubble up as the "latest" candidate.
pub fn is_release_tag(name: &str) -> bool {
    let s = match name.strip_prefix('v') {
        Some(s) => s,
        None => return false,
    };
    let main_part = s.split_once('-').map(|(p, _)| p).unwrap_or(s);
    let parts: Vec<&str> = main_part.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

#[cfg(test)]
#[path = "tests/release.rs"]
mod tests;
