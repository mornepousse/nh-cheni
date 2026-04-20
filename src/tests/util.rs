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

#[test]
fn tree_glyph_uses_corner_for_the_last_row() {
    assert_eq!(tree_glyph(0, 1), "└──");
    assert_eq!(tree_glyph(2, 3), "└──");
    assert_eq!(tree_glyph(99, 100), "└──");
}

#[test]
fn tree_glyph_uses_tee_for_every_non_last_row() {
    assert_eq!(tree_glyph(0, 2), "├──");
    assert_eq!(tree_glyph(1, 3), "├──");
    assert_eq!(tree_glyph(0, 100), "├──");
}

#[test]
fn format_ymd_matches_known_epochs() {
    // 1970-01-01 00:00:00 UTC.
    assert_eq!(format_ymd(0), "1970-01-01");
    // 2000-01-01 00:00:00 UTC = 946_684_800 sec.
    assert_eq!(format_ymd(946_684_800), "2000-01-01");
    // Leap day 2000-02-29.
    assert_eq!(format_ymd(951_782_400), "2000-02-29");
    // 2100 is NOT a leap year (divisible by 100, not by 400).
    // 2100-03-01 00:00:00 UTC = 4_107_542_400 sec.
    assert_eq!(format_ymd(4_107_542_400), "2100-03-01");
}

#[test]
fn format_ymd_rolls_over_at_midnight() {
    // Just before midnight → same day.
    assert_eq!(format_ymd(86_399), "1970-01-01");
    // Exactly midnight → next day.
    assert_eq!(format_ymd(86_400), "1970-01-02");
}

#[test]
fn format_ymd_hm_preserves_time_of_day() {
    // 1970-01-01 12:34 UTC.
    let secs = 12 * 3600 + 34 * 60;
    assert_eq!(format_ymd_hm(secs), "1970-01-01 12:34");
}

#[test]
fn format_ymd_hm_pads_single_digit_hour_and_minute() {
    // 01:05 should render as "01:05", not "1:5".
    let secs = 3600 + 5 * 60;
    assert_eq!(format_ymd_hm(secs), "1970-01-01 01:05");
}
