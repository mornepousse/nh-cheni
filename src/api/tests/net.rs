use super::*;

#[test]
fn default_when_unset() {
    std::env::remove_var("CHENI_HTTP_TIMEOUT");
    assert_eq!(http_timeout(), Duration::from_secs(DEFAULT_TIMEOUT_SECS));
}

#[test]
fn respects_override() {
    // Use a unique var name to avoid races with parallel tests — actually
    // this test shares env with others, so serialise the assertion on
    // the known value and restore afterwards.
    std::env::set_var("CHENI_HTTP_TIMEOUT", "45");
    assert_eq!(http_timeout(), Duration::from_secs(45));
    std::env::remove_var("CHENI_HTTP_TIMEOUT");
}

#[test]
fn rejects_too_small() {
    std::env::set_var("CHENI_HTTP_TIMEOUT", "1");
    assert_eq!(http_timeout(), Duration::from_secs(DEFAULT_TIMEOUT_SECS));
    std::env::remove_var("CHENI_HTTP_TIMEOUT");
}

#[test]
fn rejects_garbage() {
    std::env::set_var("CHENI_HTTP_TIMEOUT", "banana");
    assert_eq!(http_timeout(), Duration::from_secs(DEFAULT_TIMEOUT_SECS));
    std::env::remove_var("CHENI_HTTP_TIMEOUT");
}
