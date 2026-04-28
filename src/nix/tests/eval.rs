use super::{lookup_or_eval, parse_eval_output};

#[test]
fn parse_version_strips_trailing_newline() {
    let result = parse_eval_output("128.5.0\n");
    assert_eq!(result, Some("128.5.0".to_string()));
}

#[test]
fn parse_version_strips_quotes_and_whitespace() {
    let result = parse_eval_output("  \"1.2.3\"  \n");
    assert_eq!(result, Some("1.2.3".to_string()));
}

#[test]
fn parse_version_rejects_empty() {
    assert_eq!(parse_eval_output(""), None);
    assert_eq!(parse_eval_output("\n"), None);
    assert_eq!(parse_eval_output("   "), None);
    // Quoted-empty: covers the post-dequoting empty branch.
    assert_eq!(parse_eval_output("\"\""), None);
}

#[test]
fn parse_version_rejects_error_marker() {
    let result = parse_eval_output("error: attribute 'version' missing");
    assert_eq!(result, None);
}

#[test]
fn lookup_or_eval_cache_hit_returns_without_subprocess() {
    use crate::nix::version_cache::VersionCache;

    let mut cache = VersionCache::default();
    cache.store("nixpkgs-latest", "rev1", "firefox", "128.5.0");

    let v = lookup_or_eval(&mut cache, "nixpkgs-latest", "rev1", "fake-nar-hash", "firefox")
        .expect("cache hit should not error");
    assert_eq!(v, Some("128.5.0".to_string()));
}

#[test]
fn lookup_or_eval_cache_miss_with_invalid_rev_returns_none() {
    // Invalid rev (not hex) → query_pkg_version_at_rev returns None.
    // lookup_or_eval must propagate Ok(None), not panic.
    use crate::nix::version_cache::VersionCache;

    let mut cache = VersionCache::default();
    let v = lookup_or_eval(
        &mut cache,
        "nixpkgs-latest",
        "not-a-hex-rev",
        "sha256-fakehash",
        "firefox",
    )
    .expect("invalid rev should return Ok(None), not Err");
    assert_eq!(v, None);
}
