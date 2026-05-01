//! Persistent operation log for cheni-fork actions.
//!
//! JSONL file at `$XDG_CACHE_HOME/cheni/timeline.jsonl`. Append-only.
//! [`record`] is best-effort — any IO error is logged at DEBUG and
//! swallowed. The timeline is observational; nothing in the rebuild
//! path depends on it.
//!
//! Compat with the wrapper-era timeline file: same path, same JSONL
//! schema (`{ts, kind, package?, details}`). An existing file from
//! the wrapper era keeps being read by [`read_events`] and appended
//! to by [`record`].

use std::{
  fs,
  io::Write,
  path::PathBuf,
  time::{SystemTime, UNIX_EPOCH},
};

use color_eyre::eyre::Result;
use serde::{Deserialize, Serialize};
use tracing::debug;

/// One event in the timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
  pub ts: String,
  pub kind: String,
  pub package: Option<String>,
  #[serde(default = "empty_object")]
  pub details: serde_json::Value,
}

fn empty_object() -> serde_json::Value {
  serde_json::json!({})
}

/// Canonical path to the on-disk timeline.
pub fn timeline_path() -> PathBuf {
  cache_dir().join("timeline.jsonl")
}

fn cache_dir() -> PathBuf {
  if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
    return PathBuf::from(xdg).join("cheni");
  }
  if let Some(home) = std::env::var_os("HOME") {
    return PathBuf::from(home).join(".cache").join("cheni");
  }
  PathBuf::from("/tmp").join("cheni")
}

/// Append an event to the timeline. Best-effort: any IO error is
/// logged at DEBUG level and swallowed.
pub fn record(
  kind: &str,
  package: Option<&str>,
  details: serde_json::Value,
) {
  let event = Event {
    ts: now_rfc3339(),
    kind: kind.to_string(),
    package: package.map(str::to_string),
    details,
  };
  if let Err(e) = append_event(&event) {
    debug!("timeline: failed to append event: {e}");
  }
}

fn append_event(event: &Event) -> Result<()> {
  let path = timeline_path();
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent)?;
  }
  let mut line = serde_json::to_string(event)?;
  line.push('\n');
  let mut file = fs::OpenOptions::new()
    .create(true)
    .append(true)
    .open(&path)?;
  file.write_all(line.as_bytes())?;
  // sync_data (not sync_all) is enough — a crash mid-write leaves a
  // partial line at most, which read_events skips silently.
  file.sync_data().ok();
  Ok(())
}

/// Read all events from disk. Returns an empty Vec if the file
/// doesn't exist. Skips lines that fail to parse (logs DEBUG).
///
/// # Errors
///
/// Returns an error only when the file exists but can't be read at
/// all (permissions, hardware fault).
pub fn read_events() -> Result<Vec<Event>> {
  let path = timeline_path();
  if !path.exists() {
    return Ok(Vec::new());
  }
  let raw = fs::read_to_string(&path)?;
  let mut events = Vec::new();
  for (i, line) in raw.lines().enumerate() {
    if line.trim().is_empty() {
      continue;
    }
    match serde_json::from_str::<Event>(line) {
      Ok(e) => events.push(e),
      Err(e) => {
        debug!("timeline: skipping invalid line {}: {e}", i + 1);
      },
    }
  }
  Ok(events)
}

/// Parse an RFC3339 timestamp (the format [`record`] writes) back to
/// Unix seconds. Returns `None` for unparseable strings — skip-and-
/// debug is the policy for stale/garbage events.
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
  let mut days = 0i64;
  for y in 1970..year {
    let leap = (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0);
    days += if leap { 366 } else { 365 };
  }
  let leap =
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
  let months = [
    31u32,
    if leap { 29 } else { 28 },
    31,
    30,
    31,
    30,
    31,
    31,
    30,
    31,
    30,
    31,
  ];
  for m in 0..month - 1 {
    days += i64::from(months[m as usize]);
  }
  days += i64::from(day - 1);
  Some((days as u64) * 86_400 + hour * 3600 + minute * 60 + second)
}

/// RFC 3339 "now" timestamp. Same manual decomposition used by
/// `freezes::today_iso` — kept here so timeline is self-contained.
pub fn now_rfc3339() -> String {
  let secs = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_secs())
    .unwrap_or(0);
  format_rfc3339(secs)
}

#[allow(clippy::manual_is_multiple_of)]
fn format_rfc3339(secs: u64) -> String {
  let secs_in_day = 86_400u64;
  let days = secs / secs_in_day;
  let time_of_day = secs % secs_in_day;
  let hour = time_of_day / 3600;
  let minute = (time_of_day % 3600) / 60;
  let second = time_of_day % 60;
  let mut year = 1970i64;
  let mut remaining_days = days as i64;
  loop {
    let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let year_days = if leap { 366 } else { 365 };
    if remaining_days < year_days {
      break;
    }
    remaining_days -= year_days;
    year += 1;
  }
  let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
  let months = [
    31,
    if leap { 29 } else { 28 },
    31,
    30,
    31,
    30,
    31,
    31,
    30,
    31,
    30,
    31,
  ];
  let mut month = 1u32;
  for &m in &months {
    if remaining_days < m {
      break;
    }
    remaining_days -= m;
    month += 1;
  }
  let day = remaining_days as u32 + 1;
  format!(
    "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
  )
}

// ── Subcommand ────────────────────────────────────────────────────

use crate::args::OsTimelineArgs;

impl OsTimelineArgs {
  /// Run `nh os timeline`. Prints the most recent events from the
  /// JSONL log in reverse-chronological order (newest first).
  ///
  /// # Errors
  ///
  /// Returns an error if the timeline file exists but can't be read.
  pub fn run(self) -> Result<()> {
    let mut events = read_events()?;
    if events.is_empty() {
      println!("Timeline is empty.");
      println!(
        "Events are recorded automatically by `nh os pin/unpin/freeze/\
         unfreeze`."
      );
      return Ok(());
    }
    let limit = self.limit.unwrap_or(20);
    events.reverse(); // newest first
    let total = events.len();
    let to_show = total.min(limit);
    println!("Recent events ({to_show} of {total}):");
    for ev in events.into_iter().take(to_show) {
      let pkg = ev
        .package
        .as_deref()
        .map(|p| format!(" {p}"))
        .unwrap_or_default();
      let details_short = match &ev.details {
        serde_json::Value::Object(m) if !m.is_empty() => {
          let mut parts: Vec<String> =
            m.iter().map(|(k, v)| format!("{k}={v}")).collect();
          parts.sort();
          format!(" — {{{}}}", parts.join(", "))
        },
        _ => String::new(),
      };
      println!("  {} {}{pkg}{details_short}", ev.ts, ev.kind);
    }
    Ok(())
  }
}

#[cfg(test)]
#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
mod tests {
  use super::*;

  #[test]
  fn now_rfc3339_format_is_well_formed() {
    let ts = now_rfc3339();
    assert_eq!(ts.len(), 20);
    assert_eq!(&ts[4..5], "-");
    assert_eq!(&ts[7..8], "-");
    assert_eq!(&ts[10..11], "T");
    assert_eq!(&ts[13..14], ":");
    assert_eq!(&ts[16..17], ":");
    assert_eq!(&ts[19..20], "Z");
    assert!(ts.starts_with("20"));
  }

  #[test]
  fn format_rfc3339_known_unix_epoch() {
    assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
  }

  #[test]
  fn format_rfc3339_known_y2k() {
    // 2000-01-01T00:00:00Z = 946684800 seconds since epoch.
    assert_eq!(format_rfc3339(946_684_800), "2000-01-01T00:00:00Z");
  }

  #[test]
  fn parse_rfc3339_round_trips_with_format_rfc3339() {
    for &secs in
      &[0u64, 100, 86_399, 86_400, 946_684_800, 1_700_000_000]
    {
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
  fn event_serializes_with_expected_keys() {
    let ev = Event {
      ts: "2026-05-01T19:00:00Z".to_string(),
      kind: "pin".to_string(),
      package: Some("firefox".to_string()),
      details: serde_json::json!({"flake_dir": "/home/mae/cfg"}),
    };
    let s = serde_json::to_string(&ev).unwrap();
    assert!(s.contains("\"ts\":\"2026-05-01T19:00:00Z\""));
    assert!(s.contains("\"kind\":\"pin\""));
    assert!(s.contains("\"package\":\"firefox\""));
    assert!(s.contains("\"details\":{\"flake_dir\":\"/home/mae/cfg\"}"));
  }

  #[test]
  fn event_deserializes_with_default_details_object() {
    let raw = r#"{"ts":"2026-01-01T00:00:00Z","kind":"unpin","package":null}"#;
    let ev: Event = serde_json::from_str(raw).unwrap();
    assert_eq!(ev.kind, "unpin");
    assert_eq!(ev.package, None);
    assert_eq!(ev.details, serde_json::json!({}));
  }
}
