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

// format_elapsed is fully covered in src/tests/util.rs — no duplication here.

// --- N4: self-update-unsigned timeline event ---

/// Verify that the `self-update-unsigned` event kind serialises correctly
/// and round-trips through JSONL parsing. This mirrors what
/// `enforce_signature` records when `--allow-unsigned` bypasses a
/// signature failure. No network required.
#[test]
fn self_update_unsigned_event_round_trips() {
    use crate::nix::timeline::Event;

    let event = Event {
        ts: "2026-04-30T10:00:00Z".to_string(),
        kind: "self-update-unsigned".to_string(),
        package: None,
        details: serde_json::json!({
            "tag": "v0.8.0",
            "reason": "minisign: signature file not found",
        }),
    };
    let line = serde_json::to_string(&event).expect("serialise");
    let parsed: Event = serde_json::from_str(&line).expect("parse");
    assert_eq!(parsed.kind, "self-update-unsigned");
    assert_eq!(
        parsed.details.get("tag").and_then(|v| v.as_str()),
        Some("v0.8.0")
    );
    assert!(parsed.package.is_none());
}

/// When tag is "unknown" (couldn't read flake.lock), the event must still
/// serialise cleanly.
#[test]
fn self_update_unsigned_event_unknown_tag() {
    use crate::nix::timeline::Event;

    let event = Event {
        ts: "2026-04-30T10:00:00Z".to_string(),
        kind: "self-update-unsigned".to_string(),
        package: None,
        details: serde_json::json!({
            "tag": "unknown",
            "reason": "could not determine release tag",
        }),
    };
    let line = serde_json::to_string(&event).expect("serialise");
    let parsed: Event = serde_json::from_str(&line).expect("parse");
    assert_eq!(
        parsed.details.get("tag").and_then(|v| v.as_str()),
        Some("unknown")
    );
}

// --- bump_cheni_pin_in_flake_text ---

#[test]
fn bump_pin_replaces_simple_version() {
    let input = r#"{
  inputs.cheni = {
    url = "gitlab:harrael/cheni/v0.4.1";
    inputs.nixpkgs.follows = "nixpkgs";
  };
}
"#;
    let (out, changed) = bump_cheni_pin_in_flake_text(input, "v0.5.1");
    assert!(changed);
    assert!(out.contains(r#"url = "gitlab:harrael/cheni/v0.5.1";"#));
    assert!(!out.contains("v0.4.1"));
}

#[test]
fn bump_pin_preserves_owner_for_forks() {
    // A fork at gitlab:somefork/cheni still gets bumped — the regex
    // captures the owner part rather than hard-coding `harrael`.
    let input = r#"url = "gitlab:somefork/cheni/v0.4.1";"#;
    let (out, changed) = bump_cheni_pin_in_flake_text(input, "v0.5.1");
    assert!(changed);
    assert_eq!(out, r#"url = "gitlab:somefork/cheni/v0.5.1";"#);
}

#[test]
fn bump_pin_handles_pre_release_suffixes() {
    // `v0.1.0-beta` shape used during the alpha cycle. Both source and
    // target may carry a suffix; the substitution rewrites the whole
    // version segment.
    let input = r#"url = "gitlab:harrael/cheni/v0.1.0-beta";"#;
    let (out, changed) = bump_cheni_pin_in_flake_text(input, "v0.2.0");
    assert!(changed);
    assert_eq!(out, r#"url = "gitlab:harrael/cheni/v0.2.0";"#);
}

#[test]
fn bump_pin_returns_unchanged_when_no_match() {
    // User uses a branch tracking pin (no version segment) — nothing
    // to substitute, but the call must succeed without modifying.
    let input = r#"url = "gitlab:harrael/cheni";"#;
    let (out, changed) = bump_cheni_pin_in_flake_text(input, "v0.5.1");
    assert!(!changed);
    assert_eq!(out, input);
}

#[test]
fn bump_pin_does_not_touch_unrelated_urls() {
    // Mid-file mention of another flake input that happens to also
    // pin a version must be left alone.
    let input = r#"
inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
inputs.cheni.url = "gitlab:harrael/cheni/v0.4.1";
inputs.zen-browser.url = "github:0xc000022070/zen-browser-flake/v1.0.0";
"#;
    let (out, changed) = bump_cheni_pin_in_flake_text(input, "v0.5.1");
    assert!(changed);
    assert!(out.contains(r#"gitlab:harrael/cheni/v0.5.1"#));
    assert!(out.contains(r#"github:NixOS/nixpkgs/nixos-unstable"#));
    assert!(out.contains(r#"github:0xc000022070/zen-browser-flake/v1.0.0"#));
}
