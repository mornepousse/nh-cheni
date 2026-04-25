use super::*;

#[test]
fn tarball_url_matches_gitlab_auto_archive_pattern() {
    assert_eq!(
        tarball_url("v0.2.0"),
        "https://gitlab.com/harrael/cheni/-/archive/v0.2.0/cheni-v0.2.0.tar.gz"
    );
}

#[test]
fn signature_url_matches_gitlab_release_download_pattern() {
    assert_eq!(
        signature_url("v0.2.0"),
        "https://gitlab.com/harrael/cheni/-/releases/v0.2.0/downloads/cheni-v0.2.0.tar.gz.minisig"
    );
}

#[test]
fn urls_handle_prerelease_tags() {
    assert!(tarball_url("v0.1.0-beta").contains("v0.1.0-beta"));
    assert!(signature_url("v1.0.0-rc1").contains("v1.0.0-rc1"));
}

#[test]
fn embedded_public_key_decodes_cleanly() {
    // A cheni binary where this test fails at compile-time cannot
    // verify any release — the whole self-update / verify flow is
    // dead. So guard it explicitly.
    assert!(minisign_verify::PublicKey::decode(RELEASE_PUBKEY.trim()).is_ok());
}

#[test]
fn verify_rejects_garbage_signature() {
    // Garbage input must produce a typed error, not a panic.
    let bad_sig = "untrusted comment: garbage\n\
                   AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==\n\
                   trusted comment: garbage\n\
                   BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB==\n";
    assert!(verify_bytes(RELEASE_PUBKEY, b"irrelevant", bad_sig).is_err());
}

#[test]
fn verify_rejects_completely_malformed_signature_text() {
    assert!(verify_bytes(RELEASE_PUBKEY, b"payload", "not a signature at all").is_err());
}

#[test]
fn extract_trusted_comment_reads_the_marker_line() {
    let sig = "untrusted comment: signature from minisign secret key\n\
               AAAAAAAAAAAAAAAAAAAAAAAAAAAA\n\
               trusted comment: cheni v0.2.0 release\n\
               BBBBBBBBBBBBBBBBBBBBBBBBBBBB\n";
    assert_eq!(extract_trusted_comment(sig), "cheni v0.2.0 release");
}

#[test]
fn extract_trusted_comment_strips_surrounding_whitespace() {
    // minisign may or may not put a space after the colon; handle both.
    let sig = "trusted comment:   padded with spaces   \n";
    assert_eq!(extract_trusted_comment(sig), "padded with spaces");
}

#[test]
fn extract_trusted_comment_returns_empty_when_missing() {
    let sig = "untrusted comment: only the untrusted one\nAAAAAA\n";
    assert_eq!(extract_trusted_comment(sig), "");
}

#[test]
fn strip_dev_suffix_passes_exact_tag_through() {
    assert_eq!(strip_dev_suffix("v0.1.0-beta"), "v0.1.0-beta");
    assert_eq!(strip_dev_suffix("v1.2.3"), "v1.2.3");
}

#[test]
fn strip_dev_suffix_strips_commit_count_and_hash() {
    assert_eq!(strip_dev_suffix("v0.1.0-beta-5-gabcdef0"), "v0.1.0-beta");
    assert_eq!(strip_dev_suffix("v1.2.3-42-g1234567890ab"), "v1.2.3");
}

#[test]
fn strip_dev_suffix_strips_dirty_flag_too() {
    assert_eq!(strip_dev_suffix("v0.1.0-beta-5-gabcdef0-dirty"), "v0.1.0-beta");
    // `-dirty` without a preceding `-N-gHASH` run — the helper still
    // trims the `-dirty` marker, leaving just the tag.
    assert_eq!(strip_dev_suffix("v0.1.0-beta-dirty"), "v0.1.0-beta");
}

#[test]
fn strip_dev_suffix_preserves_prerelease_tags() {
    // A pre-release tag has a `-pre` suffix that looks superficially
    // like the dev suffix, but doesn't match `-N-gHEX`. It must pass
    // through untouched.
    assert_eq!(strip_dev_suffix("v1.0.0-rc1"), "v1.0.0-rc1");
    assert_eq!(strip_dev_suffix("v2.0.0-alpha"), "v2.0.0-alpha");
    assert_eq!(strip_dev_suffix("v2.0.0-alpha.2"), "v2.0.0-alpha.2");
}

#[test]
fn strip_dev_suffix_handles_unknown_fallback() {
    // build.rs falls back to the string "unknown" when it can't call
    // git. `cheni verify` should surface that distinctly rather than
    // try to verify against a tag named "unknown".
    assert_eq!(strip_dev_suffix("unknown"), "unknown");
}

// --- is_release_tag / pick_latest_tag ---

#[test]
fn is_release_tag_accepts_bare_semver() {
    assert!(is_release_tag("v0.5.1"));
    assert!(is_release_tag("v10.20.30"));
    assert!(is_release_tag("v0.0.0"));
}

#[test]
fn is_release_tag_accepts_pre_release_suffix() {
    assert!(is_release_tag("v0.1.0-beta"));
    assert!(is_release_tag("v1.0.0-rc1"));
    assert!(is_release_tag("v2.0.0-alpha.2"));
}

#[test]
fn is_release_tag_rejects_non_release_shapes() {
    assert!(!is_release_tag("main"));
    assert!(!is_release_tag("0.5.1"));     // missing leading 'v'
    assert!(!is_release_tag("v0.5"));      // 2-segment
    assert!(!is_release_tag("v0.5.1.2"));  // 4-segment
    assert!(!is_release_tag("vX.Y.Z"));    // non-numeric
    assert!(!is_release_tag(""));
}

#[test]
fn pick_latest_tag_orders_by_version_descending() {
    // Mixed shipping order — the API may return tags in date order,
    // but we pick the highest version regardless.
    let body = serde_json::json!([
        {"name": "v0.4.0"},
        {"name": "v0.5.1"},
        {"name": "v0.5.0"},
        {"name": "v0.4.1"},
    ])
    .to_string();
    assert_eq!(pick_latest_tag(&body).unwrap(), "v0.5.1");
}

#[test]
fn pick_latest_tag_filters_out_non_release_shapes() {
    // GitLab also lists branch-shaped or feature-marker tags. They
    // must not bubble up as the "latest".
    let body = serde_json::json!([
        {"name": "preview-experimental"},
        {"name": "v0.5.0"},
        {"name": "old-snapshot"},
    ])
    .to_string();
    assert_eq!(pick_latest_tag(&body).unwrap(), "v0.5.0");
}

#[test]
fn pick_latest_tag_prefers_stable_over_prerelease_at_same_version() {
    // `v0.5.0` and `v0.5.0-rc1` both parse to `[0, 5, 0]`. The
    // tie-breaker should put the stable one on top so we don't
    // recommend a release candidate over its own GA tag.
    let body = serde_json::json!([
        {"name": "v0.5.0-rc1"},
        {"name": "v0.5.0"},
    ])
    .to_string();
    assert_eq!(pick_latest_tag(&body).unwrap(), "v0.5.0");
}

#[test]
fn pick_latest_tag_errors_when_no_release_tag_present() {
    let body = serde_json::json!([
        {"name": "preview-1"},
        {"name": "scratch-branch"},
    ])
    .to_string();
    assert!(pick_latest_tag(&body).is_err());
}

#[test]
fn pick_latest_tag_errors_on_non_array_payload() {
    assert!(pick_latest_tag(r#"{"oops": "object instead of array"}"#).is_err());
    assert!(pick_latest_tag("not even json").is_err());
}
