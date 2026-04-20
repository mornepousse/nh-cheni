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
fn reads_cheni_timestamp_from_lockfile() {
    // Pure helper: parse the lock's `cheni` input and return its
    // `lastModified`. Used to detect whether `nix flake update cheni`
    // actually bumped anything.
    let lock = flake_lock_with_cheni("v0.4.0");
    let value: serde_json::Value = serde_json::from_str(&lock).unwrap();
    assert_eq!(get_input_timestamp(&value, "cheni"), Some(1_700_000_000));
}

#[test]
fn cheni_timestamp_is_none_when_input_absent() {
    let lock = serde_json::json!({
        "nodes": { "root": { "inputs": {} } },
        "root": "root"
    });
    assert_eq!(get_input_timestamp(&lock, "cheni"), None);
}

#[test]
fn format_elapsed_under_a_minute() {
    assert_eq!(format_elapsed(std::time::Duration::from_secs(0)), "0s");
    assert_eq!(format_elapsed(std::time::Duration::from_secs(42)), "42s");
}

#[test]
fn format_elapsed_over_a_minute() {
    assert_eq!(format_elapsed(std::time::Duration::from_secs(60)), "1m00s");
    assert_eq!(format_elapsed(std::time::Duration::from_secs(125)), "2m05s");
}
