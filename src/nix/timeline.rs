//! Persistent operation log for cheni.
//!
//! JSONL file at `~/.cache/cheni/timeline.jsonl`. Append-only.
//! `record()` is best-effort — any IO error is logged at debug
//! level and swallowed. The timeline is observational, never
//! authoritative.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
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

/// Path to the timeline file.
pub fn timeline_path() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("cheni")
        .join("timeline.jsonl")
}

/// Append an event to the timeline. Best-effort: any IO error is
/// logged at debug level and swallowed.
pub fn record(kind: &str, package: Option<&str>, details: serde_json::Value) {
    let event = Event {
        ts: now_rfc3339(),
        kind: kind.to_string(),
        package: package.map(|s| s.to_string()),
        details,
    };
    if let Err(e) = append_event(&event) {
        debug!("timeline: failed to append event: {e}");
    }
}

fn append_event(event: &Event) -> Result<()> {
    let path = timeline_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(event)?;
    line.push('\n');
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    file.write_all(line.as_bytes())?;
    // Flush data pages to disk so a crash doesn't leave a truncated JSONL
    // line. sync_data (not sync_all) is sufficient — metadata (mtime, size)
    // can lag behind without risking data loss. Swallow the error: the
    // timeline is best-effort and the write already succeeded.
    file.sync_data().ok();
    Ok(())
}

/// Read all events from disk. Returns empty Vec if the file doesn't
/// exist. Skips lines that fail to parse (logs debug).
pub fn read_events() -> Result<Vec<Event>> {
    let path = timeline_path();
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path)?;
    let mut events = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Event>(line) {
            Ok(e) => events.push(e),
            Err(e) => debug!("timeline: skipping invalid line {}: {e}", i + 1),
        }
    }
    Ok(events)
}

/// Parse an RFC3339 timestamp (the format `record()` writes) back to
/// Unix seconds. Returns `None` for unparseable strings — skip-and-debug
/// is the policy for stale/garbage events.
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

    // Days from 1970-01-01 to year-month-day.
    let mut days: i64 = 0;
    for y in 1970..year {
        let leap = (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0);
        days += if leap { 366 } else { 365 };
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let month_lens = [
        31u32,
        if leap { 29 } else { 28 },
        31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
    ];
    for (i, m) in month_lens.iter().enumerate() {
        if (i as u32) + 1 == month {
            break;
        }
        days += *m as i64;
    }
    days += (day - 1) as i64;
    Some((days as u64) * 86_400 + hour * 3600 + minute * 60 + second)
}

/// RFC 3339 timestamp for "now". Reuses the same manual decomposition
/// as snapshot.rs (we don't pull chrono).
pub(crate) fn now_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_rfc3339(secs)
}

#[allow(clippy::manual_is_multiple_of)]
fn format_rfc3339(secs: u64) -> String {
    // Same algorithm as snapshot.rs::now_rfc3339 — extracted for testability.
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
    let months = [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month = 1u32;
    for &m in months.iter() {
        if remaining_days < m {
            break;
        }
        remaining_days -= m;
        month += 1;
    }
    let day = remaining_days as u32 + 1;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, minute, second
    )
}

#[cfg(test)]
#[path = "tests/timeline.rs"]
mod tests;
