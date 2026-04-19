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
