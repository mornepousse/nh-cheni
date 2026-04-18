use super::*;

#[test]
fn atomic_write_creates_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("greeting.txt");
    atomic_write(&path, "hello").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello");
}

#[test]
fn atomic_write_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("counter");
    atomic_write(&path, "1").unwrap();
    atomic_write(&path, "2").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "2");
}

#[test]
fn atomic_write_leaves_no_tmp_files_on_success() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("final.txt");
    atomic_write(&path, "clean").unwrap();
    // The only thing in the directory should be the target file.
    let entries: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0], "final.txt");
}
