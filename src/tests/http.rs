use super::*;

// Tests call the pure `resolve_timeout(env_value)` helper rather than
// `http_timeout()` directly — that way we don't mutate the shared
// CHENI_HTTP_TIMEOUT env var and parallel test runs (cargo test
// default + the Nix build) stop racing.

#[test]
fn default_when_unset() {
    assert_eq!(resolve_timeout(None), Duration::from_secs(DEFAULT_TIMEOUT_SECS));
}

#[test]
fn respects_override() {
    assert_eq!(resolve_timeout(Some("45")), Duration::from_secs(45));
}

#[test]
fn respects_override_with_whitespace() {
    // Shell users sometimes wrap values in quotes that survive into the
    // env; trim handles that case without complaint.
    assert_eq!(resolve_timeout(Some("  60  ")), Duration::from_secs(60));
}

#[test]
fn accepts_exact_minimum() {
    // Boundary: MIN_TIMEOUT_SECS itself must be accepted, not rejected.
    assert_eq!(
        resolve_timeout(Some("5")),
        Duration::from_secs(5)
    );
}

#[test]
fn rejects_too_small() {
    // Below MIN_TIMEOUT_SECS (5) the user almost certainly made a typo —
    // fall back to the default with a debug log.
    assert_eq!(
        resolve_timeout(Some("1")),
        Duration::from_secs(DEFAULT_TIMEOUT_SECS)
    );
}

#[test]
fn rejects_garbage() {
    assert_eq!(
        resolve_timeout(Some("banana")),
        Duration::from_secs(DEFAULT_TIMEOUT_SECS)
    );
}

#[test]
fn rejects_empty_string() {
    // `CHENI_HTTP_TIMEOUT=` unset-ish case — not a valid number, falls
    // through to the default.
    assert_eq!(
        resolve_timeout(Some("")),
        Duration::from_secs(DEFAULT_TIMEOUT_SECS)
    );
}

#[test]
fn content_length_within_limit_passes() {
    assert!(check_content_length(Some(1024), MAX_BODY_BYTES).is_ok());
    assert!(check_content_length(Some(MAX_BODY_BYTES as u64), MAX_BODY_BYTES).is_ok());
}

#[test]
fn content_length_over_limit_rejected() {
    let err = check_content_length(Some(MAX_BODY_BYTES as u64 + 1), MAX_BODY_BYTES).unwrap_err();
    assert!(err.to_string().contains("Content-Length"));
    assert!(err.to_string().contains("exceeds"));
}

#[test]
fn content_length_missing_passes() {
    // No Content-Length header — we can't pre-check, defer to verify_body_size.
    assert!(check_content_length(None, MAX_BODY_BYTES).is_ok());
}

#[test]
fn verify_body_size_within_limit_passes() {
    assert!(verify_body_size(0, MAX_BODY_BYTES).is_ok());
    assert!(verify_body_size(MAX_BODY_BYTES, MAX_BODY_BYTES).is_ok());
}

#[test]
fn verify_body_size_over_limit_rejected() {
    let err = verify_body_size(MAX_BODY_BYTES + 1, MAX_BODY_BYTES).unwrap_err();
    assert!(err.to_string().contains("exceeds"));
}

#[test]
fn retry_after_honors_server_seconds() {
    assert_eq!(parse_retry_after(Some("5")), 5);
    assert_eq!(parse_retry_after(Some("  12  ")), 12);
}

#[test]
fn retry_after_caps_at_max() {
    // Beyond the cap we fall back to the default — we'd rather
    // return "unknown" than block a user command for a full minute.
    assert_eq!(parse_retry_after(Some("60")), RATE_LIMIT_RETRY_SECS);
    assert_eq!(
        parse_retry_after(Some(&(RATE_LIMIT_MAX_WAIT_SECS + 1).to_string())),
        RATE_LIMIT_RETRY_SECS
    );
}

#[test]
fn retry_after_accepts_boundary() {
    assert_eq!(parse_retry_after(Some("1")), 1);
    assert_eq!(
        parse_retry_after(Some(&RATE_LIMIT_MAX_WAIT_SECS.to_string())),
        RATE_LIMIT_MAX_WAIT_SECS
    );
}

#[test]
fn retry_after_falls_back_on_missing_or_invalid() {
    assert_eq!(parse_retry_after(None), RATE_LIMIT_RETRY_SECS);
    assert_eq!(parse_retry_after(Some("")), RATE_LIMIT_RETRY_SECS);
    assert_eq!(parse_retry_after(Some("0")), RATE_LIMIT_RETRY_SECS);
    // HTTP-date form — not parsed, falls back to the default.
    assert_eq!(
        parse_retry_after(Some("Wed, 21 Oct 2026 07:28:00 GMT")),
        RATE_LIMIT_RETRY_SECS
    );
    assert_eq!(parse_retry_after(Some("soon")), RATE_LIMIT_RETRY_SECS);
}

// --- USER_AGENT centralisation guard ---

#[test]
fn user_agent_constant_carries_a_real_version() {
    // env!("GIT_DESCRIBE") may be a tag (`v0.5.5`), a dev shape
    // (`v0.5.5-3-gabc-dirty`), or the literal "unknown" when build.rs
    // can't reach git. The UA must carry a non-empty version segment
    // for Repology to distinguish cheni versions in its rate-limit policy.
    //
    // Note: the prefix is NOT "cheni/" — Repology's nginx blocks any UA
    // whose token contains the string "cheni" (confirmed 2026-04-27).
    // See `user_agent_repology_compliance` below for the full invariant.
    assert!(USER_AGENT.contains('/'));
    let version_part = USER_AGENT.split('/').nth(1).unwrap_or("");
    assert!(
        !version_part.is_empty(),
        "USER_AGENT must include a non-empty version after '/': got {:?}",
        USER_AGENT
    );
}

#[test]
fn user_agent_repology_compliance() {
    // Two hard requirements verified against the live API on 2026-04-27:
    //
    // 1. The UA token (part before the first '/') must NOT contain
    //    "cheni" — Repology's nginx blocklists it and returns HTTP 403:
    //      cheni/v0.5.6                              → 403
    //      cheni/v0.5.9 (https://gitlab.com/…)       → 403
    //      harrael-cheni/... (https://...)            → 403
    //      nixos-cheni/... (https://...)              → 403
    //    The repo URL containing "cheni" in the *path* is NOT filtered.
    //
    // 2. Repology API terms of use require a repo URL in the UA:
    //    «Bulk clients MUST identify themselves with a User-Agent
    //    containing a link to their source code repository.»
    //    UAs without a https:// link are also blocked:
    //      curl/8.19.0                               → 403
    //      nix-version-checker/... (https://…)       → 200
    let token = USER_AGENT.split('/').next().unwrap_or("");
    assert!(
        !token.to_lowercase().contains("cheni"),
        "USER_AGENT token must not contain 'cheni' (Repology blocklist): got {:?}",
        USER_AGENT
    );
    assert!(
        USER_AGENT.contains("https://gitlab.com/harrael/cheni"),
        "USER_AGENT must contain the repo URL for Repology compliance: got {:?}",
        USER_AGENT
    );
}

#[test]
fn no_hardcoded_user_agent_outside_http_module() {
    // Sentinel against the regression that prompted v0.5.5 — every
    // `.user_agent(...)` call in `src/` must reference the central
    // `crate::http::USER_AGENT` (or its module-local alias
    // `http::USER_AGENT` for callers that already import the module).
    // A literal string would silently desync from the real version
    // and end up exactly where `cheni/0.1` did: blocklisted.
    let src_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations: Vec<String> = Vec::new();
    walk_source_files(&src_root, &mut |path, contents| {
        // Skip the http module itself — it owns the constant
        // declaration plus this very test, both of which contain the
        // string ".user_agent(" in comments / fixtures.
        if path.ends_with("http.rs") || path.ends_with("tests/http.rs") {
            return;
        }
        for line in contents.lines() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            if !line.contains(".user_agent(") {
                continue;
            }
            // Allowed forms: `.user_agent(crate::http::USER_AGENT)` or
            // `.user_agent(http::USER_AGENT)`.
            let allowed = line.contains("crate::http::USER_AGENT")
                || line.contains("http::USER_AGENT");
            if !allowed {
                violations.push(format!("{}: {}", path.display(), line.trim()));
            }
        }
    });
    assert!(
        violations.is_empty(),
        "found `.user_agent(...)` calls that don't go through `crate::http::USER_AGENT`:\n  {}",
        violations.join("\n  ")
    );
}

/// Walk every `.rs` file under `root`, calling `visit(path, contents)`
/// for each. Recursive directory walk without external crates so the
/// guard test stays a single-file pure dep on std.
fn walk_source_files(root: &std::path::Path, visit: &mut impl FnMut(&std::path::Path, &str)) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_source_files(&path, visit);
        } else if path.extension().is_some_and(|e| e == "rs") {
            if let Ok(contents) = std::fs::read_to_string(&path) {
                visit(&path, &contents);
            }
        }
    }
}
