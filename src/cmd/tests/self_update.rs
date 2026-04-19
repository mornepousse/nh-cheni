use super::*;

/// Minimal flake.lock shape that mirrors what `nix flake update cheni`
/// produces when the user pins `inputs.cheni.url = "gitlab:harrael/cheni/vX.Y.Z"`.
fn flake_lock_with_cheni(tag: &str) -> String {
    serde_json::json!({
        "nodes": {
            "cheni": {
                "locked": {
                    "type": "gitlab",
                    "owner": "harrael",
                    "repo": "cheni",
                    "rev": "abc123def456",
                    "ref": tag,
                    "lastModified": 1_700_000_000u64,
                    "narHash": "sha256-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa="
                },
                "original": {
                    "type": "gitlab",
                    "owner": "harrael",
                    "repo": "cheni",
                    "ref": tag
                }
            },
            "root": {
                "inputs": { "cheni": "cheni" }
            }
        },
        "root": "root",
        "version": 7
    })
    .to_string()
}

#[test]
fn extracts_tag_from_pinned_input() {
    let lock = flake_lock_with_cheni("v0.2.0");
    assert_eq!(extract_cheni_tag(&lock).unwrap(), "v0.2.0");
}

#[test]
fn extracts_prerelease_tag() {
    let lock = flake_lock_with_cheni("v0.1.0-alpha");
    assert_eq!(extract_cheni_tag(&lock).unwrap(), "v0.1.0-alpha");
}

#[test]
fn errors_when_cheni_input_absent() {
    let lock = serde_json::json!({
        "nodes": { "root": { "inputs": {} } },
        "root": "root",
        "version": 7
    })
    .to_string();
    let err = extract_cheni_tag(&lock).unwrap_err().to_string();
    assert!(err.contains("no 'cheni' input"));
}

#[test]
fn errors_when_input_pinned_to_branch_without_ref() {
    // Users with `inputs.cheni.url = "gitlab:harrael/cheni"` have no
    // `ref` in their flake.lock. We can't know which release to verify.
    let lock = serde_json::json!({
        "nodes": {
            "cheni": {
                "locked": {
                    "type": "gitlab",
                    "owner": "harrael",
                    "repo": "cheni",
                    "rev": "abc123",
                    "lastModified": 1_700_000_000u64
                },
                "original": {
                    "type": "gitlab",
                    "owner": "harrael",
                    "repo": "cheni"
                }
            },
            "root": { "inputs": { "cheni": "cheni" } }
        },
        "root": "root",
        "version": 7
    })
    .to_string();
    let err = extract_cheni_tag(&lock).unwrap_err().to_string();
    assert!(err.contains("no 'ref'"));
}

#[test]
fn errors_on_malformed_json() {
    let err = extract_cheni_tag("not json at all").unwrap_err().to_string();
    assert!(err.contains("parsing flake.lock"));
}

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
fn verify_accepts_the_bundled_public_key_format() {
    // The constant embedded at build time must parse as a valid
    // minisign public key, or every self-update fails at startup.
    assert!(minisign_verify::PublicKey::decode(RELEASE_PUBKEY.trim()).is_ok());
}

#[test]
fn verify_rejects_tampered_payload() {
    // Hand-crafted invalid signature (wrong base64, wrong length).
    // We only need to confirm verify_bytes surfaces a typed error
    // rather than panicking.
    let bad_sig = "untrusted comment: test\n\
                   AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA==\n\
                   trusted comment: test\n\
                   BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB==\n";
    assert!(verify_bytes(RELEASE_PUBKEY, b"some payload", bad_sig).is_err());
}
