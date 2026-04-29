use super::*;

// --- resolve_remote_head (pure parsing path — no network) ---

#[test]
fn resolve_remote_head_returns_none_for_missing_input() {
    // A flake.lock with no root inputs: the function must return Ok(None)
    // rather than panic or error.
    let dir = tempfile::TempDir::new().expect("tempdir");
    let lock = serde_json::json!({
        "nodes": {
            "root": { "inputs": {} }
        },
        "root": "root",
        "version": 7
    });
    std::fs::write(
        dir.path().join("flake.lock"),
        serde_json::to_string_pretty(&lock).expect("serialize"),
    )
    .expect("write");

    let result = resolve_remote_head(dir.path(), "nixpkgs-latest");
    // Must not error — the input simply isn't there.
    assert!(result.is_ok(), "should return Ok even when input is absent");
    assert!(result.unwrap().is_none(), "should return None for missing input");
}

#[test]
fn resolve_remote_head_returns_none_for_unsupported_type() {
    // A `tarball`-type input has no `owner`/`repo`/`ref` fields that can
    // be reconstructed into a flakeref. resolve_remote_head must return
    // Ok(None) without shelling out.
    let dir = tempfile::TempDir::new().expect("tempdir");
    let lock = serde_json::json!({
        "nodes": {
            "root": {
                "inputs": { "some-tarball": "some-tarball" }
            },
            "some-tarball": {
                "locked": {
                    "rev": "abcdef1234567890abcdef1234567890abcdef12",
                    "narHash": "sha256-AAAA=",
                    "type": "tarball",
                    "url": "https://example.com/pkg.tar.gz"
                },
                "original": {
                    "type": "tarball",
                    "url": "https://example.com/pkg.tar.gz"
                }
            }
        },
        "root": "root",
        "version": 7
    });
    std::fs::write(
        dir.path().join("flake.lock"),
        serde_json::to_string_pretty(&lock).expect("serialize"),
    )
    .expect("write");

    let result = resolve_remote_head(dir.path(), "some-tarball");
    assert!(result.is_ok(), "should return Ok for unsupported type");
    assert!(
        result.unwrap().is_none(),
        "should return None for tarball-type input (unsupported)"
    );
}

#[test]
fn infrastructure_inputs_excluded() {
    // Core infrastructure (always excluded)
    assert!(INFRASTRUCTURE_INPUTS.contains(&"nixpkgs"));
    assert!(INFRASTRUCTURE_INPUTS.contains(&"nixpkgs-latest"));
    assert!(INFRASTRUCTURE_INPUTS.contains(&"home-manager"));
    assert!(INFRASTRUCTURE_INPUTS.contains(&"cheni"));

    // User-facing package flakes (never excluded)
    assert!(!INFRASTRUCTURE_INPUTS.contains(&"zen-browser"));
    assert!(!INFRASTRUCTURE_INPUTS.contains(&"claude-code"));

    // Optional toolchain flakes should NOT be excluded — not every user
    // has them and those who do want update visibility.
    assert!(!INFRASTRUCTURE_INPUTS.contains(&"rust-overlay"));
    assert!(!INFRASTRUCTURE_INPUTS.contains(&"nixpkgs-esp-dev"));
    assert!(!INFRASTRUCTURE_INPUTS.contains(&"fenix"));
}

#[test]
fn short_hash_handles_short_input() {
    // The API may return a hash shorter than 12 chars (rare, but a
    // byte-slice would panic). Char-based truncation returns as many
    // chars as exist without panicking.
    assert_eq!(short_hash("abc"), "abc");
    assert_eq!(short_hash(""), "");
}

#[test]
fn short_hash_truncates_to_twelve() {
    assert_eq!(
        short_hash("abcdef1234567890"),
        "abcdef123456"
    );
}

#[test]
fn short_hash_survives_non_ascii() {
    // Not expected in real Git output, but we parse external JSON
    // so we can't assume it. Must not panic at a multi-byte boundary.
    assert_eq!(short_hash("é🦀x"), "é🦀x");
}

#[test]
fn short_date_handles_short_input() {
    assert_eq!(short_date("2026"), "2026");
    assert_eq!(short_date(""), "");
}

#[test]
fn is_revision_outdated_detects_change() {
    // flake.lock stores 12-char prefixes; API returns longer SHAs.
    // The comparison should truncate the remote to the local length.
    assert!(is_revision_outdated("abcdef123456", "000000000000"));
    assert!(is_revision_outdated("abcdef123456789", "000000000000"));
}

#[test]
fn is_revision_outdated_false_when_prefix_matches() {
    // Remote rev is longer but starts with the local prefix → up to date.
    assert!(!is_revision_outdated("abcdef1234", "abcdef1234"));
    assert!(!is_revision_outdated("abcdef123456", "abcdef123456"));
}

#[test]
fn is_revision_outdated_empty_inputs() {
    // Defensive: empty vs empty is not outdated; empty vs non-empty
    // compares prefix-of-length-0 which is always equal, so not outdated.
    // This matches the behaviour of the API-failed-to-respond path.
    assert!(!is_revision_outdated("", ""));
    assert!(!is_revision_outdated("", "abcdef"));
}

#[test]
fn is_revision_outdated_survives_non_ascii() {
    // Char-based slicing: mustn't panic on a multi-byte codepoint.
    assert!(is_revision_outdated("é🦀x000000", "abc000000000"));
}

// --- is_valid_repo_slug ---

#[test]
fn repo_slug_accepts_typical_names() {
    assert!(is_valid_repo_slug("nixpkgs"));
    assert!(is_valid_repo_slug("harrael"));
    assert!(is_valid_repo_slug("my-repo_42"));
    assert!(is_valid_repo_slug("Org.Name"));
}

#[test]
fn repo_slug_rejects_empty() {
    assert!(!is_valid_repo_slug(""));
}

#[test]
fn repo_slug_rejects_slashes_and_traversal() {
    // A slash or ".." in owner/repo would allow constructing a URL
    // that escapes the intended path (e.g. /repos/foo/../../../etc).
    assert!(!is_valid_repo_slug("foo/bar"));
    assert!(!is_valid_repo_slug(".."));
    assert!(!is_valid_repo_slug("../etc"));
    assert!(!is_valid_repo_slug("foo/../../bar"));
}

#[test]
fn repo_slug_rejects_special_characters() {
    assert!(!is_valid_repo_slug("foo bar"));
    assert!(!is_valid_repo_slug("foo@bar"));
    assert!(!is_valid_repo_slug("foo#bar"));
    assert!(!is_valid_repo_slug("foo%2Fbar")); // URL-encoded slash
}

#[test]
fn sanitize_username_accepts_typical_forms() {
    assert_eq!(sanitize_username("mae").as_deref(), Some("mae"));
    assert_eq!(sanitize_username("user_42").as_deref(), Some("user_42"));
    assert_eq!(sanitize_username("dev-box").as_deref(), Some("dev-box"));
    assert_eq!(sanitize_username("CamelCase").as_deref(), Some("CamelCase"));
}

#[test]
fn sanitize_username_rejects_path_traversal() {
    // The whole point of the helper: `/etc/profiles/per-user/{user}`
    // must not let `..` or `/` out of the prefix.
    assert_eq!(sanitize_username(".."), None);
    assert_eq!(sanitize_username("../etc"), None);
    assert_eq!(sanitize_username("foo/bar"), None);
    assert_eq!(sanitize_username("a\\b"), None);
}

#[test]
fn sanitize_username_rejects_special_chars() {
    assert_eq!(sanitize_username("foo bar"), None);
    assert_eq!(sanitize_username("foo$bar"), None);
    assert_eq!(sanitize_username("foo\0bar"), None);
    assert_eq!(sanitize_username("foo\nbar"), None);
    assert_eq!(sanitize_username("foo.bar"), None);
}

#[test]
fn extract_root_input_rev_reads_full_rev() {
    // Minimal flake.lock-shaped fixture exercising the indirection:
    // root.inputs.nixpkgs points to "nixpkgs_4", which carries the
    // actual `locked.rev`. This is the common flake.lock shape when
    // an input is referenced by other inputs (follows).
    let lock = serde_json::json!({
        "nodes": {
            "root": {
                "inputs": {
                    "nixpkgs": "nixpkgs_4"
                }
            },
            "nixpkgs_4": {
                "locked": {
                    "rev": "abcdef0123456789abcdef0123456789abcdef01",
                    "lastModified": 1710000000
                }
            }
        }
    });
    let rev = extract_root_input_rev(&lock, "nixpkgs").unwrap();
    assert_eq!(rev, "abcdef0123456789abcdef0123456789abcdef01");
}

#[test]
fn extract_root_input_rev_handles_direct_node() {
    // When the input has no indirection (root.inputs.nixpkgs is not a
    // string pointing to another node), we fall back to a node of the
    // same name.
    let lock = serde_json::json!({
        "nodes": {
            "root": {
                "inputs": {
                    "nixpkgs": {}
                }
            },
            "nixpkgs": {
                "locked": {
                    "rev": "1111111111111111111111111111111111111111"
                }
            }
        }
    });
    let rev = extract_root_input_rev(&lock, "nixpkgs").unwrap();
    assert_eq!(rev, "1111111111111111111111111111111111111111");
}

#[test]
fn extract_root_input_rev_returns_none_for_missing_input() {
    let lock = serde_json::json!({
        "nodes": {
            "root": { "inputs": {} }
        }
    });
    assert!(extract_root_input_rev(&lock, "nixpkgs").is_none());
}

#[test]
fn read_nixpkgs_rev_roundtrips_from_fixture() {
    // End-to-end: write a fixture flake.lock, read it back.
    let dir = tempfile::tempdir().unwrap();
    let lock = serde_json::json!({
        "nodes": {
            "root": { "inputs": { "nixpkgs": "nixpkgs" } },
            "nixpkgs": {
                "locked": {
                    "rev": "cafebabe0000000000000000000000000000cafe"
                }
            }
        }
    });
    std::fs::write(
        dir.path().join("flake.lock"),
        serde_json::to_string_pretty(&lock).unwrap(),
    )
    .unwrap();
    let rev = read_nixpkgs_rev(dir.path()).unwrap();
    assert_eq!(rev, "cafebabe0000000000000000000000000000cafe");
}

#[test]
fn read_nixpkgs_rev_reports_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let err = read_nixpkgs_rev(dir.path()).unwrap_err();
    assert!(format!("{:#}", err).contains("flake.lock"));
}

#[test]
fn read_nixpkgs_rev_reports_missing_nixpkgs_input() {
    let dir = tempfile::tempdir().unwrap();
    let lock = serde_json::json!({
        "nodes": { "root": { "inputs": {} } }
    });
    std::fs::write(
        dir.path().join("flake.lock"),
        serde_json::to_string(&lock).unwrap(),
    )
    .unwrap();
    let err = read_nixpkgs_rev(dir.path()).unwrap_err();
    assert!(format!("{:#}", err).contains("nixpkgs"));
}

#[test]
fn prefetch_nixpkgs_rev_rejects_non_hex_rev() {
    // Defence in depth: even without running `nix`, the rev validation
    // kicks in synchronously before we shell out.
    let err = prefetch_nixpkgs_rev("not-a-hash").unwrap_err();
    assert!(format!("{:#}", err).contains("non-hex"));
}

#[test]
fn prefetch_nixpkgs_rev_rejects_empty_rev() {
    let err = prefetch_nixpkgs_rev("").unwrap_err();
    assert!(format!("{:#}", err).contains("non-hex"));
}

#[test]
fn prefetch_nixpkgs_rev_rejects_injection_attempt() {
    // The rev is interpolated into a URL passed as an argv arg to `nix`.
    // Command::args is safe (no shell interpretation), but we still
    // refuse anything non-hex so the URL stays clean in logs and any
    // future change to how the rev is consumed stays safe.
    let err = prefetch_nixpkgs_rev("abc; rm -rf /").unwrap_err();
    assert!(format!("{:#}", err).contains("non-hex"));
}

#[test]
fn sanitize_username_rejects_empty_and_oversized() {
    assert_eq!(sanitize_username(""), None);
    // POSIX usernames traditionally cap at 32; anything longer is a
    // signal something is wrong (env injection, corrupted state).
    let long = "a".repeat(33);
    assert_eq!(sanitize_username(&long), None);
    let boundary = "a".repeat(32);
    assert_eq!(sanitize_username(&boundary).as_deref(), Some(boundary.as_str()));
}

// --- read_input_locked ---

#[test]
fn read_input_locked_returns_rev_and_nar_hash() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let lock = serde_json::json!({
        "nodes": {
            "root": { "inputs": { "nixpkgs-latest": "nixpkgs-latest" } },
            "nixpkgs-latest": {
                "locked": {
                    "rev": "abc123def456abc123def456abc123def456abc1",
                    "narHash": "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
                    "type": "github"
                },
                "original": {}
            }
        },
        "root": "root"
    });
    std::fs::write(
        dir.path().join("flake.lock"),
        serde_json::to_string_pretty(&lock).expect("serialize"),
    )
    .expect("write");

    let result = read_input_locked(dir.path(), "nixpkgs-latest");
    assert!(result.is_some(), "should return Some for a valid entry");
    let (rev, nar_hash) = result.unwrap();
    assert_eq!(rev, "abc123def456abc123def456abc123def456abc1");
    assert_eq!(nar_hash, "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=");
}

#[test]
fn read_input_locked_missing_nar_hash_returns_none() {
    // An entry with rev but no narHash must not panic — returns None.
    let dir = tempfile::TempDir::new().expect("tempdir");
    let lock = serde_json::json!({
        "nodes": {
            "root": { "inputs": { "nixpkgs-latest": "nixpkgs-latest" } },
            "nixpkgs-latest": {
                "locked": {
                    "rev": "abc123def456abc123def456abc123def456abc1"
                    // narHash deliberately absent
                }
            }
        },
        "root": "root"
    });
    std::fs::write(
        dir.path().join("flake.lock"),
        serde_json::to_string_pretty(&lock).expect("serialize"),
    )
    .expect("write");

    assert_eq!(read_input_locked(dir.path(), "nixpkgs-latest"), None);
}

#[test]
fn read_input_locked_missing_input_returns_none() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let lock = serde_json::json!({
        "nodes": { "root": { "inputs": {} } },
        "root": "root"
    });
    std::fs::write(
        dir.path().join("flake.lock"),
        serde_json::to_string(&lock).expect("serialize"),
    )
    .expect("write");
    assert_eq!(read_input_locked(dir.path(), "nixpkgs-latest"), None);
}

#[test]
fn read_input_locked_follows_indirection() {
    // root.inputs["nixpkgs-latest"] = "nixpkgs-latest_2" (indirection)
    let dir = tempfile::TempDir::new().expect("tempdir");
    let lock = serde_json::json!({
        "nodes": {
            "root": { "inputs": { "nixpkgs-latest": "nixpkgs-latest_2" } },
            "nixpkgs-latest_2": {
                "locked": {
                    "rev": "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
                    "narHash": "sha256-deadbeef"
                }
            }
        },
        "root": "root"
    });
    std::fs::write(
        dir.path().join("flake.lock"),
        serde_json::to_string_pretty(&lock).expect("serialize"),
    )
    .expect("write");

    let result = read_input_locked(dir.path(), "nixpkgs-latest");
    assert!(result.is_some());
    let (rev, nar_hash) = result.unwrap();
    assert_eq!(rev, "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef");
    assert_eq!(nar_hash, "sha256-deadbeef");
}

// --- read_input_rev ---

#[test]
fn read_input_rev_returns_locked_rev() {
    // Full end-to-end path through read_input_rev: writes a fixture
    // flake.lock with an indirection node (root.inputs[name] → node_name)
    // and verifies the full 40-char rev is returned.
    // narHash is required (read_input_rev delegates to read_input_locked).
    let dir = tempfile::TempDir::new().expect("tempdir");
    let lock = serde_json::json!({
        "nodes": {
            "root": { "inputs": { "nixpkgs-latest": "nixpkgs-latest" } },
            "nixpkgs-latest": {
                "locked": {
                    "rev": "abc123def456abc123def456abc123def456abc1",
                    "narHash": "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
                    "type": "github"
                },
                "original": {}
            }
        },
        "root": "root"
    });
    std::fs::write(
        dir.path().join("flake.lock"),
        serde_json::to_string_pretty(&lock).expect("serialize"),
    )
    .expect("write");

    let rev = read_input_rev(dir.path(), "nixpkgs-latest");
    assert_eq!(rev, Some("abc123def456abc123def456abc123def456abc1".to_string()));
}

#[test]
fn read_input_rev_missing_input_returns_none() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let lock = serde_json::json!({
        "nodes": { "root": { "inputs": {} } },
        "root": "root"
    });
    std::fs::write(
        dir.path().join("flake.lock"),
        serde_json::to_string(&lock).expect("serialize"),
    )
    .expect("write");
    assert_eq!(read_input_rev(dir.path(), "nixpkgs-latest"), None);
}
