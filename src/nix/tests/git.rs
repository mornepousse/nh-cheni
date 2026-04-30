use super::*;
use std::process::Command;
use std::time::{Duration, UNIX_EPOCH};

// --- validate_git_filename ---

#[test]
fn git_filename_valid_simple() {
    assert!(validate_git_filename("package-pins.json").is_ok());
}

#[test]
fn git_filename_valid_freezes() {
    assert!(validate_git_filename("package-freezes.json").is_ok());
}

#[test]
fn git_filename_rejects_empty() {
    assert!(validate_git_filename("").is_err());
}

#[test]
fn git_filename_rejects_path_with_slash() {
    assert!(validate_git_filename("subdir/file.json").is_err());
}

#[test]
fn git_filename_rejects_dotdot_traversal() {
    assert!(validate_git_filename("../etc/passwd").is_err());
}

#[test]
fn git_filename_rejects_leading_dot() {
    assert!(validate_git_filename(".git").is_err());
    assert!(validate_git_filename(".gitconfig").is_err());
}

#[test]
fn git_filename_rejects_backslash() {
    assert!(validate_git_filename("sub\\file").is_err());
}

#[test]
fn git_filename_rejects_dotdot_embedded() {
    assert!(validate_git_filename("foo..bar").is_err());
}

/// Spin up a fresh git repo in a tempdir with a deterministic identity
/// and a single committed file. Returns the tempdir handle (kept alive
/// by the caller for the test's lifetime) and the absolute commit time
/// in Unix seconds.
///
/// `git commit` requires both an author and a committer identity; we
/// inject them via `-c` so the test doesn't pollute or depend on the
/// caller's `~/.gitconfig`. Same goes for `--no-gpg-sign` — a host that
/// signs commits by default would otherwise hang the test waiting for
/// the GPG agent.
fn fixture_repo(filename: &str, contents: &str, ts: u64) -> (tempfile::TempDir, u64) {
    let dir = tempfile::tempdir().unwrap();

    let init = Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["init", "-q", "-b", "main"])
        .status()
        .unwrap();
    assert!(init.success(), "git init failed");

    std::fs::write(dir.path().join(filename), contents).unwrap();

    let add = Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["add", filename])
        .status()
        .unwrap();
    assert!(add.success(), "git add failed");

    // GIT_*_DATE pins the commit timestamp deterministically; -c sets
    // identity without touching ~/.gitconfig.
    let iso = crate::util::format_iso_utc(ts);
    let commit = Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .env("GIT_AUTHOR_DATE", &iso)
        .env("GIT_COMMITTER_DATE", &iso)
        .args([
            "-c",
            "user.email=test@cheni",
            "-c",
            "user.name=test",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--no-gpg-sign",
            "-m",
            "fixture",
        ])
        .status()
        .unwrap();
    assert!(commit.success(), "git commit failed");

    (dir, ts)
}

#[test]
fn is_repo_true_inside_a_git_work_tree() {
    let (dir, _) = fixture_repo("hello.txt", "hi", 1_700_000_000);
    assert!(is_repo(dir.path()));
}

#[test]
fn is_repo_false_for_a_plain_directory() {
    let dir = tempfile::tempdir().unwrap();
    assert!(!is_repo(dir.path()));
}

#[test]
fn read_file_at_time_returns_the_committed_blob() {
    let (dir, ts) = fixture_repo("config.json", "[\"a\"]", 1_700_000_000);
    let at = UNIX_EPOCH + Duration::from_secs(ts + 60);
    let content = read_file_at_time(dir.path(), "config.json", at).unwrap();
    assert_eq!(content.trim(), "[\"a\"]");
}

#[test]
fn read_file_at_time_returns_none_before_the_first_commit() {
    let (dir, ts) = fixture_repo("config.json", "[\"a\"]", 1_700_000_000);
    let before = UNIX_EPOCH + Duration::from_secs(ts - 3600);
    assert!(read_file_at_time(dir.path(), "config.json", before).is_none());
}

#[test]
fn read_file_at_time_returns_none_for_unknown_file() {
    let (dir, ts) = fixture_repo("config.json", "[\"a\"]", 1_700_000_000);
    let after = UNIX_EPOCH + Duration::from_secs(ts + 60);
    assert!(read_file_at_time(dir.path(), "no-such-file.json", after).is_none());
}

#[test]
fn read_file_at_time_returns_none_outside_a_repo() {
    let dir = tempfile::tempdir().unwrap();
    let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    assert!(read_file_at_time(dir.path(), "anything", now).is_none());
}

#[test]
fn read_file_at_time_picks_the_latest_commit_at_or_before_the_query() {
    // Two commits, one hour apart. Query in between → first commit's
    // contents. Query after second → second commit's contents.
    let dir = tempfile::tempdir().unwrap();
    Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["init", "-q", "-b", "main"])
        .status()
        .unwrap();

    let mk_commit = |contents: &str, ts: u64| {
        std::fs::write(dir.path().join("file.json"), contents).unwrap();
        Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["add", "file.json"])
            .status()
            .unwrap();
        let iso = crate::util::format_iso_utc(ts);
        Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .env("GIT_AUTHOR_DATE", &iso)
            .env("GIT_COMMITTER_DATE", &iso)
            .args([
                "-c",
                "user.email=test@cheni",
                "-c",
                "user.name=test",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "--no-gpg-sign",
                "-m",
                "fixture",
            ])
            .status()
            .unwrap();
    };
    let t1: u64 = 1_700_000_000;
    let t2: u64 = t1 + 3600;
    mk_commit("[\"v1\"]", t1);
    mk_commit("[\"v1\",\"v2\"]", t2);

    let between = UNIX_EPOCH + Duration::from_secs(t1 + 1800);
    let after_both = UNIX_EPOCH + Duration::from_secs(t2 + 60);

    let between_content = read_file_at_time(dir.path(), "file.json", between).unwrap();
    assert!(between_content.contains("v1"));
    assert!(!between_content.contains("v2"));

    let after_content = read_file_at_time(dir.path(), "file.json", after_both).unwrap();
    assert!(after_content.contains("v1"));
    assert!(after_content.contains("v2"));
}

// --- is_flake_lock_dirty ---

#[test]
fn is_flake_lock_dirty_false_when_committed_and_clean() {
    // flake.lock committed, no modifications → not dirty.
    let (dir, _) = fixture_repo("flake.lock", "{\"version\":7}", 1_700_000_000);
    assert!(!is_flake_lock_dirty(dir.path()));
}

#[test]
fn is_flake_lock_dirty_true_when_modified_unstaged() {
    // flake.lock committed then modified in the working tree without staging.
    let (dir, _) = fixture_repo("flake.lock", "{\"version\":7}", 1_700_000_000);
    std::fs::write(dir.path().join("flake.lock"), "{\"version\":8}").unwrap();
    assert!(is_flake_lock_dirty(dir.path()));
}

#[test]
fn is_flake_lock_dirty_false_when_modification_is_staged() {
    // `git diff --name-only flake.lock` compares working tree against the
    // index (not HEAD). Once the change is staged the working tree matches
    // the index, so the diff is empty and the function returns false.
    // Callers that want to catch staged changes must use `--cached` — this
    // test documents the current behaviour so any accidental change is caught.
    let (dir, _) = fixture_repo("flake.lock", "{\"version\":7}", 1_700_000_000);
    std::fs::write(dir.path().join("flake.lock"), "{\"version\":8}").unwrap();
    Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["add", "flake.lock"])
        .status()
        .unwrap();
    assert!(!is_flake_lock_dirty(dir.path()));
}

#[test]
fn is_flake_lock_dirty_false_when_directory_is_not_a_git_repo() {
    // Outside a git repo `git diff` exits non-zero → we treat it as
    // "not dirty" rather than crashing. The warning surface is optional.
    let dir = tempfile::tempdir().unwrap();
    assert!(!is_flake_lock_dirty(dir.path()));
}

#[test]
fn is_flake_lock_dirty_false_when_flake_lock_is_untracked() {
    // An untracked flake.lock doesn't appear in `git diff` (index vs
    // worktree) because it was never staged. Returns false.
    let dir = tempfile::tempdir().unwrap();
    Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["init", "-q", "-b", "main"])
        .status()
        .unwrap();
    // Commit an unrelated file so the repo has a HEAD.
    std::fs::write(dir.path().join("other.txt"), "hello").unwrap();
    Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .args(["add", "other.txt"])
        .status()
        .unwrap();
    Command::new("git")
        .arg("-C")
        .arg(dir.path())
        .env("GIT_AUTHOR_DATE", "2024-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2024-01-01T00:00:00Z")
        .args([
            "-c", "user.email=test@cheni",
            "-c", "user.name=test",
            "-c", "commit.gpgsign=false",
            "commit", "-q", "--no-gpg-sign", "-m", "fixture",
        ])
        .status()
        .unwrap();
    // Now drop an untracked flake.lock.
    std::fs::write(dir.path().join("flake.lock"), "{\"version\":7}").unwrap();
    assert!(!is_flake_lock_dirty(dir.path()));
}
