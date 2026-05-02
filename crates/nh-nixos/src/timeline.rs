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
//!
//! # Helpers used (jump table for navigation)
//!
//! When you read this file and hit one of these calls, the
//! implementation lives in `crates/nh-nixos/src/cheni_util/<x>.rs`:
//!
//! - `time::now_rfc3339()` — `YYYY-MM-DDTHH:MM:SSZ` timestamp.
//!   Used inside [`record`] when stamping new events. Re-exported
//!   from this module under the same name so existing call sites
//!   that did `timeline::now_rfc3339()` still compile.
//! - `time::format_rfc3339(unix_secs)` — render a unix timestamp
//!   in RFC 3339. Re-exported as
//!   [`now_rfc3339_from_secs`] for back-compat with `events.rs`
//!   which adopted the longer name during the cheni extension phase.
//! - `time::parse_rfc3339_to_unix(ts)` — inverse parse. Re-exported
//!   under the same name. Used by `events::build_rows` to slot
//!   events into generations.
//!
//! Local helper `create_private_dir` (defined inline) creates the
//! cache directory with mode 0o700 explicitly so the existence of
//! events isn't disclosed via `ls`.

use std::{
  fs,
  io::Write,
  path::{Path, PathBuf},
};

use color_eyre::eyre::Result;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::cheni_util::time;

// Re-export the time helpers under their wrapper-era names so call
// sites that already do `timeline::now_rfc3339()` keep working.
pub use crate::cheni_util::time::{
    now_rfc3339,
    parse_rfc3339_to_unix,
};

/// Public `now_rfc3339_from_secs` alias kept for callers like
/// `events::render_date` that adopted this name during the cheni
/// extension phase. New callers should use `cheni_util::time::format_rfc3339`
/// directly.
#[must_use]
pub fn now_rfc3339_from_secs(unix_secs: u64) -> String {
  time::format_rfc3339(unix_secs)
}

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
    create_private_dir(parent)?;
  }
  let mut line = serde_json::to_string(event)?;
  line.push('\n');
  let mut opts = fs::OpenOptions::new();
  opts.create(true).append(true);
  #[cfg(unix)]
  {
    use std::os::unix::fs::OpenOptionsExt;
    // 0o600 on creation so the file is private to the user.
    opts.mode(0o600);
    // Refuse to follow a symlink at the predictable timeline path —
    // closes the TOCTOU where a local attacker pre-plants a symlink
    // in a shared cache directory pointing at /home/mae/.bashrc or
    // similar. Ineffective once the cache_dir is properly 0o700
    // (no one else can write there) but defence-in-depth.
    opts.custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits());
  }
  let mut file = opts.open(&path)?;
  file.write_all(line.as_bytes())?;
  // sync_data (not sync_all) is enough — a crash mid-write leaves a
  // partial line at most, which read_events skips silently.
  file.sync_data().ok();
  Ok(())
}

/// Create the cache directory with mode 0o700 on Unix so that the
/// existence of timeline events isn't disclosed to other local users.
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
  #[cfg(unix)]
  {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
      .recursive(true)
      .mode(0o700)
      .create(dir)
  }
  #[cfg(not(unix))]
  {
    fs::create_dir_all(dir)
  }
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

// Time / date helpers (now_rfc3339, parse_rfc3339_to_unix,
// format_rfc3339) live in cheni_util::time. They were lifted out of
// this module during the post-pivot audit, since freezes.rs and
// events.rs use them too.

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

  // Time / date format tests (now_rfc3339, format_rfc3339,
  // parse_rfc3339_to_unix) moved with the implementations to
  // cheni_util::time. Only Event-level serde tests remain here.

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
