use super::*;

#[test]
fn short_rev_truncates_to_twelve_chars() {
    assert_eq!(
        short_rev("abcdef0123456789abcdef0123456789abcdef01"),
        "abcdef012345"
    );
}

#[test]
fn short_rev_handles_short_input() {
    assert_eq!(short_rev("abc"), "abc");
    assert_eq!(short_rev(""), "");
}

#[test]
fn short_rev_is_char_safe_on_non_ascii() {
    // Must not panic at a multi-byte codepoint boundary.
    assert_eq!(short_rev("é🦀x"), "é🦀x");
}
