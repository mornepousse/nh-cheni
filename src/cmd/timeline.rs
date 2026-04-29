//! `cheni timeline` — read and filter the operation log.

use anyhow::{bail, Result};
use colored::Colorize;

use crate::nix::timeline::{read_events, Event};

#[derive(Debug, Default)]
pub struct TimelineOptions {
    /// Show only the last N events. Default 20.
    pub last: Option<usize>,
    /// Filter by package name.
    pub package: Option<String>,
    /// Filter by event kind.
    pub kind: Option<String>,
    /// Filter by age (e.g. "7d", "1h", "30m").
    pub since: Option<String>,
    /// Raw JSONL pass-through.
    pub json: bool,
}

const DEFAULT_LAST: usize = 20;

pub fn run(opts: TimelineOptions) -> Result<()> {
    let events = read_events()?;
    if events.is_empty() {
        if !opts.json {
            println!("{}", "No events yet — operations will be logged from now on.".dimmed());
        }
        return Ok(());
    }

    let since_secs = if let Some(spec) = &opts.since {
        Some(parse_since_duration_secs(spec)?)
    } else {
        None
    };

    let filtered: Vec<&Event> = events
        .iter()
        .filter(|e| match_filters(e, opts.package.as_deref(), opts.kind.as_deref(), since_secs))
        .collect();

    let last = opts.last.unwrap_or(DEFAULT_LAST);
    let to_show: Vec<&Event> = filtered.iter().rev().take(last).rev().copied().collect();

    if opts.json {
        for e in &to_show {
            println!("{}", serde_json::to_string(e)?);
        }
        return Ok(());
    }

    println!(
        "{}",
        format!("=== cheni timeline (last {}) ===", to_show.len()).bold()
    );
    println!();
    for e in &to_show {
        render_event(e);
    }
    Ok(())
}

fn render_event(e: &Event) {
    let pkg = e.package.as_deref().unwrap_or("");
    let details_summary = summarise_details(&e.kind, &e.details);
    let line = format!(
        "  {}  {:<8} {:<24} {}",
        e.ts.dimmed(),
        e.kind.cyan(),
        pkg,
        details_summary.dimmed()
    );
    println!("{}", line);
}

fn summarise_details(kind: &str, details: &serde_json::Value) -> String {
    if details.is_null() || details == &serde_json::json!({}) {
        return String::new();
    }
    match kind {
        "promote" | "demote" => {
            let from = details.get("from").and_then(|v| v.as_str()).unwrap_or("?");
            let to = details.get("to").and_then(|v| v.as_str()).unwrap_or("?");
            format!("({from} \u{2192} {to})")
        }
        "freeze" => {
            let v = details.get("version").and_then(|v| v.as_str()).unwrap_or("");
            if v.is_empty() {
                String::new()
            } else {
                format!("at {v}")
            }
        }
        "restore" => {
            let host = details.get("from").and_then(|v| v.as_str()).unwrap_or("?");
            format!("from {host}")
        }
        _ => details.to_string(),
    }
}

pub(crate) fn match_filters(
    event: &Event,
    package: Option<&str>,
    kind: Option<&str>,
    since_secs: Option<u64>,
) -> bool {
    if let Some(p) = package {
        if event.package.as_deref() != Some(p) {
            return false;
        }
    }
    if let Some(k) = kind {
        if event.kind != k {
            return false;
        }
    }
    if let Some(secs) = since_secs {
        if !event_within_last_secs(event, secs) {
            return false;
        }
    }
    true
}

fn event_within_last_secs(event: &Event, max_age_secs: u64) -> bool {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let event_secs = parse_rfc3339_to_unix(&event.ts).unwrap_or(0);
    if event_secs == 0 || event_secs > now {
        return false;
    }
    (now - event_secs) <= max_age_secs
}

fn parse_rfc3339_to_unix(ts: &str) -> Option<u64> {
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
    let month_lens = [31u32, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for (i, m) in month_lens.iter().enumerate() {
        if (i as u32) + 1 == month {
            break;
        }
        days += *m as i64;
    }
    days += (day - 1) as i64;
    let unix = (days as u64) * 86_400 + hour * 3600 + minute * 60 + second;
    Some(unix)
}

pub(crate) fn parse_since_duration_secs(spec: &str) -> Result<u64> {
    let spec = spec.trim();
    let (num_str, unit) = spec
        .find(|c: char| !c.is_ascii_digit())
        .map(|i| (&spec[..i], &spec[i..]))
        .unwrap_or((spec, ""));
    let n: u64 = num_str
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid duration: '{spec}'. Try 7d, 1h, 30m."))?;
    let mult = match unit {
        "d" => 86_400,
        "h" => 3600,
        "m" => 60,
        "" => bail!("invalid duration: '{spec}'. Need a unit (d, h, m). Try 7d, 1h, 30m."),
        _ => bail!("unknown duration unit '{unit}' in '{spec}'. Use d, h, m."),
    };
    Ok(n * mult)
}

#[cfg(test)]
#[path = "tests/timeline.rs"]
mod tests;
