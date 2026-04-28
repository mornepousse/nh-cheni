use super::parse_eval_output;

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
