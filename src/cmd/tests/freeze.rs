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
    // Shorter-than-12 input mustn't panic — returns as-is.
    assert_eq!(short_rev("abc"), "abc");
    assert_eq!(short_rev(""), "");
}

#[test]
fn short_rev_is_char_safe_on_non_ascii() {
    // Defence against malformed rev strings — char-based truncation
    // mustn't split a multi-byte codepoint.
    assert_eq!(short_rev("é🦀x"), "é🦀x");
}

#[test]
fn short_nar_hash_returns_input_when_short() {
    // Shorter than the head+tail width → return as-is, no ellipsis.
    assert_eq!(short_nar_hash("sha256-abc"), "sha256-abc");
}

#[test]
fn short_nar_hash_elides_middle_when_long() {
    let full = "sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
    let short = short_nar_hash(full);
    assert!(short.starts_with("sha256-"), "got: {}", short);
    assert!(short.contains('…'), "got: {}", short);
    assert!(short.ends_with("AAAAA="), "got: {}", short);
    assert!(short.len() < full.len(), "got: {}", short);
}

#[test]
fn civil_from_days_matches_known_dates() {
    // 1970-01-01 = day 0.
    assert_eq!(civil_from_days(0), (1970, 1, 1));
    // 2000-01-01 = day 10_957.
    assert_eq!(civil_from_days(10_957), (2000, 1, 1));
    // Leap day: 2000-02-29 = day 11_016 (Jan 31 + Feb 28 + 1 extra day from 2000-01-01).
    assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    // 2100 is NOT a leap year (div by 100, not by 400). So Feb has 28 days
    // and 2100-03-01 lands on day 47_541 (Jan=31 + Feb=28 after 2100-01-01).
    assert_eq!(civil_from_days(47_541), (2100, 3, 1));
}

#[test]
fn epoch_to_iso_date_matches_known_epochs() {
    // 2026-04-20 00:00:00 UTC = 1_776_643_200 sec (day 20_563).
    assert_eq!(epoch_to_iso_date(1_776_643_200), "2026-04-20");
    // 1970-01-01 00:00:00 UTC = 0 sec.
    assert_eq!(epoch_to_iso_date(0), "1970-01-01");
    // Just before midnight still maps to the same day.
    assert_eq!(epoch_to_iso_date(1_776_643_200 + 86_399), "2026-04-20");
}

#[test]
fn epoch_to_iso_date_rolls_over_at_midnight() {
    // Exactly midnight of the next day.
    assert_eq!(epoch_to_iso_date(1_776_643_200 + 86_400), "2026-04-21");
}

#[test]
fn today_iso_is_well_formed() {
    // Smoke test: today_iso() must produce a YYYY-MM-DD string.
    let t = today_iso();
    assert_eq!(t.len(), 10);
    assert_eq!(&t[4..5], "-");
    assert_eq!(&t[7..8], "-");
    let year: u32 = t[..4].parse().expect("year is numeric");
    assert!((2020..=2200).contains(&year), "got {}", year);
}
