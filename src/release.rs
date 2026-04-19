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

use crate::api::net;

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
        .timeout(net::http_timeout())
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

/// Download bytes with the shared HTTP body cap from `api::net`.
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
    net::check_content_length(response.content_length(), net::MAX_BODY_BYTES)?;
    let body = response.bytes().await.context("reading body")?;
    net::verify_body_size(body.len(), net::MAX_BODY_BYTES)?;
    Ok(body.to_vec())
}

async fn fetch_signature_text(client: &reqwest::Client, url: &str) -> Result<String> {
    let bytes = fetch_bounded(client, url).await?;
    String::from_utf8(bytes).context("signature file is not valid UTF-8")
}

#[cfg(test)]
#[path = "tests/release.rs"]
mod tests;
