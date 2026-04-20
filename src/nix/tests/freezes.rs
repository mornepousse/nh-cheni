use super::*;

fn setup_temp_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn sample_entry() -> FreezeEntry {
    FreezeEntry {
        rev: "abcdef0123456789abcdef0123456789abcdef01".to_string(),
        nar_hash: "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string(),
        version: "127.0.1".to_string(),
        frozen_at: "2026-04-20".to_string(),
        major_constraint: None,
    }
}

#[test]
fn read_nonexistent_returns_empty() {
    let dir = setup_temp_dir();
    let freezes = read(dir.path()).unwrap();
    assert!(freezes.is_empty());
}

#[test]
fn write_and_read_roundtrip() {
    let dir = setup_temp_dir();
    let mut freezes = Freezes::new();
    freezes.insert("firefox".to_string(), sample_entry());
    write(dir.path(), &freezes).unwrap();

    let loaded = read(dir.path()).unwrap();
    assert_eq!(loaded, freezes);
}

#[test]
fn add_inserts_new_entry() {
    let dir = setup_temp_dir();
    let inserted = add(dir.path(), "firefox", sample_entry()).unwrap();
    assert!(inserted, "first add should report inserted_new=true");

    let loaded = read(dir.path()).unwrap();
    assert_eq!(loaded.len(), 1);
    assert!(loaded.contains_key("firefox"));
}

#[test]
fn add_replaces_existing_entry() {
    let dir = setup_temp_dir();
    add(dir.path(), "firefox", sample_entry()).unwrap();

    let mut updated = sample_entry();
    updated.version = "128.0.0".to_string();
    let inserted = add(dir.path(), "firefox", updated.clone()).unwrap();
    assert!(!inserted, "second add should report inserted_new=false");

    let loaded = read(dir.path()).unwrap();
    assert_eq!(loaded.get("firefox"), Some(&updated));
}

#[test]
fn remove_existing_returns_name() {
    let dir = setup_temp_dir();
    add(dir.path(), "firefox", sample_entry()).unwrap();
    add(dir.path(), "linux_zen", sample_entry()).unwrap();

    let removed = remove(dir.path(), &["firefox".to_string()]).unwrap();
    assert_eq!(removed, vec!["firefox".to_string()]);

    let loaded = read(dir.path()).unwrap();
    assert_eq!(loaded.len(), 1);
    assert!(loaded.contains_key("linux_zen"));
}

#[test]
fn remove_nonexistent_is_ok() {
    let dir = setup_temp_dir();
    add(dir.path(), "firefox", sample_entry()).unwrap();

    let removed = remove(dir.path(), &["not-there".to_string()]).unwrap();
    assert!(removed.is_empty());
}

#[test]
fn clear_removes_every_freeze() {
    let dir = setup_temp_dir();
    add(dir.path(), "firefox", sample_entry()).unwrap();
    add(dir.path(), "linux_zen", sample_entry()).unwrap();

    let count = clear(dir.path()).unwrap();
    assert_eq!(count, 2);
    assert!(read(dir.path()).unwrap().is_empty());
}

#[test]
fn empty_file_is_treated_as_no_freezes() {
    let dir = setup_temp_dir();
    std::fs::write(dir.path().join("package-freezes.json"), "").unwrap();
    let freezes = read(dir.path()).unwrap();
    assert!(freezes.is_empty());
}

#[test]
fn whitespace_only_file_is_treated_as_no_freezes() {
    let dir = setup_temp_dir();
    std::fs::write(dir.path().join("package-freezes.json"), "\n\n   \n").unwrap();
    let freezes = read(dir.path()).unwrap();
    assert!(freezes.is_empty());
}

#[test]
fn add_rejects_empty_name() {
    let dir = setup_temp_dir();
    let err = add(dir.path(), "", sample_entry()).unwrap_err();
    assert!(format!("{:#}", err).contains("empty"));
}

#[test]
fn add_rejects_control_chars_in_name() {
    let dir = setup_temp_dir();
    let err = add(dir.path(), "foo\nbar", sample_entry()).unwrap_err();
    assert!(format!("{:#}", err).contains("invalid character"));
}

#[test]
fn add_rejects_path_traversal_in_name() {
    let dir = setup_temp_dir();
    let err = add(dir.path(), "../../etc/passwd", sample_entry()).unwrap_err();
    assert!(format!("{:#}", err).contains("invalid character"));
}

#[test]
fn add_rejects_non_hex_rev() {
    let dir = setup_temp_dir();
    let mut entry = sample_entry();
    entry.rev = "not-a-hash-at-all".to_string();
    let err = add(dir.path(), "firefox", entry).unwrap_err();
    assert!(format!("{:#}", err).contains("hex git hash"));
}

#[test]
fn add_rejects_short_rev() {
    let dir = setup_temp_dir();
    let mut entry = sample_entry();
    entry.rev = "abc".to_string();
    let err = add(dir.path(), "firefox", entry).unwrap_err();
    assert!(format!("{:#}", err).contains("unusual length"));
}

#[test]
fn add_rejects_malformed_narhash() {
    let dir = setup_temp_dir();
    let mut entry = sample_entry();
    entry.nar_hash = "not-an-sri-hash".to_string();
    let err = add(dir.path(), "firefox", entry).unwrap_err();
    assert!(format!("{:#}", err).contains("SRI hash"));
}

#[test]
fn add_accepts_sha512_narhash() {
    let dir = setup_temp_dir();
    let mut entry = sample_entry();
    entry.nar_hash = format!("sha512-{}", "A".repeat(88));
    add(dir.path(), "firefox", entry).unwrap();
}

#[test]
fn add_rejects_quote_in_narhash() {
    let dir = setup_temp_dir();
    let mut entry = sample_entry();
    entry.nar_hash = "sha256-AAAA\"BBBB".to_string();
    let err = add(dir.path(), "firefox", entry).unwrap_err();
    assert!(format!("{:#}", err).contains("invalid character"));
}

#[test]
fn add_rejects_newline_in_version() {
    let dir = setup_temp_dir();
    let mut entry = sample_entry();
    entry.version = "1.0\nevil".to_string();
    let err = add(dir.path(), "firefox", entry).unwrap_err();
    assert!(format!("{:#}", err).contains("control character"));
}

#[test]
fn add_accepts_valid_special_chars_in_name() {
    let dir = setup_temp_dir();
    add(dir.path(), "gtk+3", sample_entry()).unwrap();
    add(dir.path(), "python3.13", sample_entry()).unwrap();
    add(dir.path(), "kdePackages.breeze-icons", sample_entry()).unwrap();
    add(dir.path(), "noto-fonts-cjk-sans", sample_entry()).unwrap();
}

#[test]
fn corrupt_file_gives_actionable_error() {
    let dir = setup_temp_dir();
    std::fs::write(
        dir.path().join("package-freezes.json"),
        "not valid json at all {",
    )
    .unwrap();
    let err = read(dir.path()).unwrap_err();
    let chain = format!("{:#}", err);
    assert!(chain.contains("not valid JSON"), "message was: {}", chain);
    assert!(chain.contains("package-freezes.json"), "message was: {}", chain);
    assert!(chain.contains("echo '{}'"), "message was: {}", chain);
}

#[test]
fn btreemap_ordering_is_deterministic_on_disk() {
    // Two adds in different orders should produce the same JSON — the
    // file is committed to the user's flake, so a deterministic diff is
    // important for clean git history.
    let dir_a = setup_temp_dir();
    let dir_b = setup_temp_dir();

    add(dir_a.path(), "zzz", sample_entry()).unwrap();
    add(dir_a.path(), "aaa", sample_entry()).unwrap();

    add(dir_b.path(), "aaa", sample_entry()).unwrap();
    add(dir_b.path(), "zzz", sample_entry()).unwrap();

    let text_a = std::fs::read_to_string(dir_a.path().join("package-freezes.json")).unwrap();
    let text_b = std::fs::read_to_string(dir_b.path().join("package-freezes.json")).unwrap();
    assert_eq!(text_a, text_b);
}
