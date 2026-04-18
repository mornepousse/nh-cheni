use super::*;

#[test]
fn empty_cache_is_valid() {
    let cache = Cache::default();
    assert!(cache.entries.is_empty());
    assert_eq!(cache.timestamp, 0);
}

#[test]
fn new_cache_has_timestamp() {
    let cache = new_with_timestamp();
    assert!(cache.timestamp > 0);
}
