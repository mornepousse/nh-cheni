//! Date / time helpers — RFC 3339 formatting and parsing without
//! pulling `chrono`.
//!
//! The same manual leap-year + month-length decomposition was
//! duplicated in `freezes::today_iso`, `timeline::format_rfc3339`
//! and `timeline::parse_rfc3339_to_unix`. Lifted here so a fix to
//! the calendar logic propagates to all callers.

use std::time::{SystemTime, UNIX_EPOCH};

const SECS_PER_DAY: u64 = 86_400;

/// Today's date as `YYYY-MM-DD` in UTC. Used by freeze records to
/// stamp `frozen_at`.
#[must_use]
pub fn today_iso() -> String {
    let secs = unix_now_secs();
    let (y, m, d, _, _, _) = unix_to_ymdhms(secs);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Now as an RFC 3339 timestamp (`YYYY-MM-DDTHH:MM:SSZ`). Used by
/// timeline events.
#[must_use]
pub fn now_rfc3339() -> String {
    format_rfc3339(unix_now_secs())
}

/// Format `unix_secs` (seconds since epoch) as RFC 3339.
///
/// Public so callers that store mtimes can render them in the same
/// shape as event timestamps (e.g. `nh os events` printing
/// generation mtimes).
#[must_use]
pub fn format_rfc3339(unix_secs: u64) -> String {
    let (y, m, d, h, min, s) = unix_to_ymdhms(unix_secs);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}Z")
}

/// Parse an RFC 3339 timestamp back to seconds since epoch. Returns
/// `None` on any parse failure (out-of-range fields, bad shape,
/// non-numeric segments). The skip-and-degrade policy: a malformed
/// timestamp in the timeline drops the event from queries, never
/// crashes them.
#[must_use]
pub fn parse_rfc3339_to_unix(ts: &str) -> Option<u64> {
    // Format: "YYYY-MM-DDTHH:MM:SSZ"
    let trimmed = ts.trim_end_matches('Z');
    let (date, time) = trimmed.split_once('T')?;
    let date_parts: Vec<&str> = date.split('-').collect();
    let time_parts: Vec<&str> = time.split(':').collect();
    if date_parts.len() != 3 || time_parts.len() != 3 {
        return None;
    }
    let year: i64 = date_parts[0].parse().ok()?;
    let month: u32 = date_parts[1].parse().ok()?;
    let day: u32 = date_parts[2].parse().ok()?;
    let hour: u64 = time_parts[0].parse().ok()?;
    let minute: u64 = time_parts[1].parse().ok()?;
    let second: u64 = time_parts[2].parse().ok()?;
    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    Some(ymdhms_to_unix(year, month, day, hour, minute, second))
}

// ── Internal calendar arithmetic ──────────────────────────────────

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

fn days_in_month(year: i64, month_idx_0based: usize) -> u32 {
    const LENGTHS: [u32; 12] =
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let l = LENGTHS[month_idx_0based];
    if month_idx_0based == 1 && is_leap(year) { 29 } else { l }
}

fn unix_to_ymdhms(secs: u64) -> (i64, u32, u32, u64, u64, u64) {
    let days = secs / SECS_PER_DAY;
    let time_of_day = secs % SECS_PER_DAY;
    let hour = time_of_day / 3600;
    let minute = (time_of_day % 3600) / 60;
    let second = time_of_day % 60;

    let mut year = 1970i64;
    let mut remaining = days as i64;
    loop {
        let year_days = if is_leap(year) { 366 } else { 365 };
        if remaining < year_days {
            break;
        }
        remaining -= year_days;
        year += 1;
    }
    let mut month_idx = 0usize;
    while month_idx < 12 {
        let l = i64::from(days_in_month(year, month_idx));
        if remaining < l {
            break;
        }
        remaining -= l;
        month_idx += 1;
    }
    let day = remaining as u32 + 1;
    (year, (month_idx as u32) + 1, day, hour, minute, second)
}

fn ymdhms_to_unix(
    year: i64,
    month: u32,
    day: u32,
    hour: u64,
    minute: u64,
    second: u64,
) -> u64 {
    let mut days = 0i64;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    for m in 0..(month - 1) as usize {
        days += i64::from(days_in_month(year, m));
    }
    days += i64::from(day - 1);
    (days as u64) * SECS_PER_DAY + hour * 3600 + minute * 60 + second
}

#[cfg(test)]
#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
mod tests {
    use super::*;

    #[test]
    fn today_iso_format_is_well_formed() {
        let s = today_iso();
        assert_eq!(s.len(), 10);
        assert_eq!(&s[4..5], "-");
        assert_eq!(&s[7..8], "-");
        assert!(s.starts_with("20"));
    }

    #[test]
    fn now_rfc3339_format_is_well_formed() {
        let ts = now_rfc3339();
        assert_eq!(ts.len(), 20);
        assert_eq!(&ts[10..11], "T");
        assert_eq!(&ts[19..20], "Z");
    }

    #[test]
    fn format_rfc3339_known_values() {
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339(946_684_800), "2000-01-01T00:00:00Z");
    }

    #[test]
    fn parse_rfc3339_round_trips() {
        for &secs in &[
            0u64,
            100,
            86_399,
            86_400,
            946_684_800,
            1_700_000_000,
            // Leap year boundary: 2024-02-29 12:00:00 UTC.
            1_709_208_000,
        ] {
            let s = format_rfc3339(secs);
            assert_eq!(parse_rfc3339_to_unix(&s), Some(secs), "round-trip {s}");
        }
    }

    #[test]
    fn parse_rfc3339_rejects_garbage() {
        assert_eq!(parse_rfc3339_to_unix(""), None);
        assert_eq!(parse_rfc3339_to_unix("not-a-date"), None);
        assert_eq!(parse_rfc3339_to_unix("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_rfc3339_to_unix("2026-01-32T00:00:00Z"), None);
        assert_eq!(parse_rfc3339_to_unix("2026-01-01T25:00:00Z"), None);
    }

    #[test]
    fn leap_year_boundary_february_29() {
        // 2024 is a leap year (div 4, not 100).
        assert!(is_leap(2024));
        // 1900 is NOT a leap year (div 100, not 400).
        assert!(!is_leap(1900));
        // 2000 IS a leap year (div 400).
        assert!(is_leap(2000));
    }
}
