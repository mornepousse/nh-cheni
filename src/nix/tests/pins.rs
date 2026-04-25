use super::*;

fn setup_temp_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn read_nonexistent_returns_empty() {
    let dir = setup_temp_dir();
    let pins = read(dir.path()).unwrap();
    assert!(pins.is_empty());
}

#[test]
fn write_and_read_roundtrip() {
    let dir = setup_temp_dir();
    let pins = vec!["legcord".to_string(), "vivaldi".to_string()];
    write(dir.path(), &pins).unwrap();
    let loaded = read(dir.path()).unwrap();
    assert_eq!(loaded, pins);
}

#[test]
fn add_deduplicates() {
    let dir = setup_temp_dir();
    write(dir.path(), &["legcord".to_string()]).unwrap();

    let added = add(dir.path(), &["legcord".into(), "vivaldi".into()]).unwrap();
    assert_eq!(added, vec!["vivaldi".to_string()]);

    let pins = read(dir.path()).unwrap();
    assert_eq!(pins, vec!["legcord", "vivaldi"]);
}

#[test]
fn remove_existing() {
    let dir = setup_temp_dir();
    write(dir.path(), &["a".into(), "b".into(), "c".into()]).unwrap();

    let removed = remove(dir.path(), &["b".into()]).unwrap();
    assert_eq!(removed, vec!["b".to_string()]);

    let pins = read(dir.path()).unwrap();
    assert_eq!(pins, vec!["a", "c"]);
}

#[test]
fn remove_nonexistent_is_ok() {
    let dir = setup_temp_dir();
    write(dir.path(), &["a".into()]).unwrap();

    let removed = remove(dir.path(), &["z".into()]).unwrap();
    assert!(removed.is_empty());
}

#[test]
fn clear_all() {
    let dir = setup_temp_dir();
    write(dir.path(), &["a".into(), "b".into(), "c".into()]).unwrap();

    let count = clear(dir.path()).unwrap();
    assert_eq!(count, 3);

    let pins = read(dir.path()).unwrap();
    assert!(pins.is_empty());
}

#[test]
fn empty_file_is_treated_as_no_pins() {
    // An editor that saves an empty file, or a `> package-pins.json`
    // shell redirect, shouldn't break cheni.
    let dir = setup_temp_dir();
    std::fs::write(dir.path().join("package-pins.json"), "").unwrap();
    let pins = read(dir.path()).unwrap();
    assert!(pins.is_empty());
}

#[test]
fn whitespace_only_file_is_treated_as_no_pins() {
    let dir = setup_temp_dir();
    std::fs::write(dir.path().join("package-pins.json"), "\n\n   \n").unwrap();
    let pins = read(dir.path()).unwrap();
    assert!(pins.is_empty());
}

#[test]
fn add_rejects_empty_name() {
    let dir = setup_temp_dir();
    let err = add(dir.path(), &["".to_string()]).unwrap_err();
    assert!(format!("{:#}", err).contains("empty"));
}

#[test]
fn add_rejects_control_chars() {
    let dir = setup_temp_dir();
    let err = add(dir.path(), &["foo\nbar".to_string()]).unwrap_err();
    let msg = format!("{:#}", err);
    assert!(msg.contains("invalid character"), "got: {}", msg);
}

#[test]
fn add_rejects_path_traversal() {
    let dir = setup_temp_dir();
    let err = add(dir.path(), &["../../etc/passwd".to_string()]).unwrap_err();
    assert!(format!("{:#}", err).contains("invalid character"));
}

#[test]
fn add_rejects_quote_injection() {
    let dir = setup_temp_dir();
    let err = add(dir.path(), &["foo\"; rm -rf /".to_string()]).unwrap_err();
    assert!(format!("{:#}", err).contains("invalid character"));
}

#[test]
fn add_rejects_overlong_name() {
    let dir = setup_temp_dir();
    let huge = "a".repeat(256);
    let err = add(dir.path(), &[huge]).unwrap_err();
    assert!(format!("{:#}", err).contains("suspiciously long"));
}

#[test]
fn add_accepts_valid_special_chars() {
    // Real nixpkgs names that use the separator characters we allow.
    let dir = setup_temp_dir();
    add(
        dir.path(),
        &[
            "gtk+3".to_string(),
            "python3.13".to_string(),
            "kdePackages.breeze-icons".to_string(),
            "noto-fonts-cjk-sans".to_string(),
        ],
    )
    .unwrap();
}

#[test]
fn read_at_time_returns_committed_pins() {
    // Wires the git-time-travel helper through to the pins parser.
    // The shape coverage (no-repo, missing file, before-first-commit,
    // multi-commit) lives in nix::git::tests; this test only locks the
    // pins-specific JSON parsing on top of git output.
    use std::process::Command;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    let dir = setup_temp_dir();
    Command::new("git")
        .arg("-C").arg(dir.path())
        .args(["init", "-q", "-b", "main"])
        .status().unwrap();
    write(dir.path(), &["firefox".into(), "vivaldi".into()]).unwrap();
    Command::new("git")
        .arg("-C").arg(dir.path())
        .args(["add", "package-pins.json"])
        .status().unwrap();
    Command::new("git")
        .arg("-C").arg(dir.path())
        .args([
            "-c", "user.email=test@cheni",
            "-c", "user.name=test",
            "-c", "commit.gpgsign=false",
            "commit", "-q", "--no-gpg-sign", "-m", "pin",
        ])
        .status().unwrap();

    let now: SystemTime = SystemTime::now() + Duration::from_secs(60);
    let _ = now.duration_since(UNIX_EPOCH).unwrap(); // anchor, fail loudly if clock is broken

    let pins = read_at_time(dir.path(), now);
    assert_eq!(pins, vec!["firefox".to_string(), "vivaldi".to_string()]);
}

#[test]
fn read_at_time_returns_empty_outside_of_a_repo() {
    // A user whose flake isn't versioned: the helper must degrade
    // silently — `cheni history` falls back to no annotation.
    use std::time::SystemTime;
    let dir = setup_temp_dir();
    write(dir.path(), &["firefox".into()]).unwrap();
    let pins = read_at_time(dir.path(), SystemTime::now());
    assert!(pins.is_empty());
}

#[test]
fn corrupt_file_gives_actionable_error() {
    // Garbage-in-file produces an error whose message contains the
    // path and the reset command — not just a raw serde error.
    let dir = setup_temp_dir();
    std::fs::write(
        dir.path().join("package-pins.json"),
        "not valid json at all {",
    )
    .unwrap();
    let err = read(dir.path()).unwrap_err();
    let chain = format!("{:#}", err);
    assert!(chain.contains("not valid JSON"), "message was: {}", chain);
    assert!(chain.contains("package-pins.json"), "message was: {}", chain);
    assert!(chain.contains("echo '[]'"), "message was: {}", chain);
}
