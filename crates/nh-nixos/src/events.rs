//! `nh os events` — NixOS generations annotated with cheni timeline.
//!
//! Lists system generations in reverse-chronological order, grouping
//! the cheni-modifying events from
//! `$XDG_CACHE_HOME/cheni/timeline.jsonl` under whichever generation
//! they fall into. Useful to answer questions like "what changed
//! around the time generation 142 was built?" without leaving the
//! shell.
//!
//! Deliberately a separate subcommand from `nh os info` (the
//! vanilla-nh generation listing) so we never touch upstream nh code
//! and future merges stay friction-free.

use std::{
  fs,
  path::{Path, PathBuf},
  time::UNIX_EPOCH,
};

use color_eyre::eyre::{Context, Result};
use tracing::debug;

use crate::timeline::{Event, parse_rfc3339_to_unix};

const SYSTEM_PROFILE_DIR: &str = "/nix/var/nix/profiles";

/// One row in the events table — a generation plus its containing
/// events.
#[derive(Debug, Clone)]
pub struct GenerationEvents {
  pub number: u64,
  /// Unix mtime of the generation symlink.
  pub mtime_secs: u64,
  /// Whether this is the currently-active generation.
  pub current: bool,
  pub events: Vec<Event>,
}

/// List `system-N-link` entries under `profiles_dir` and return their
/// number + mtime in numeric order. Skips entries we can't read mtime
/// for (rare, but exotic FS may lack it).
pub fn read_generations(profiles_dir: &Path) -> Result<Vec<(u64, u64)>> {
  let mut out: Vec<(u64, u64)> = Vec::new();
  for entry in fs::read_dir(profiles_dir).with_context(|| {
    format!("listing {} for system-*-link entries", profiles_dir.display())
  })? {
    let Ok(entry) = entry else {
      continue;
    };
    let name = entry.file_name();
    let Some(name) = name.to_str() else {
      continue;
    };
    let Some(num) = parse_generation_link(name) else {
      continue;
    };
    // symlink_metadata, NOT metadata: profile entries are symlinks
    // pointing into the Nix store, and the store path's mtime is
    // useless (typically epoch+1). We want the link's own mtime,
    // which is when nixos-rebuild created the generation.
    let mtime = match fs::symlink_metadata(entry.path())
      .and_then(|m| m.modified())
      .map(|t| t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs())
    {
      Ok(t) => t,
      Err(e) => {
        debug!("events: skipping {} (mtime: {})", name, e);
        continue;
      },
    };
    out.push((num, mtime));
  }
  out.sort_by_key(|(num, _)| *num);
  Ok(out)
}

/// Parse `system-42-link` → `Some(42)`. Returns None for any other
/// shape so unrelated entries (e.g. `system`) are skipped.
fn parse_generation_link(name: &str) -> Option<u64> {
  let rest = name.strip_prefix("system-")?.strip_suffix("-link")?;
  rest.parse().ok()
}

/// Determine which generation owns `event_ts_secs`. Returns the
/// largest generation number whose mtime is ≤ the event timestamp
/// (events live in the interval `[gen_N.mtime, gen_{N+1}.mtime)`).
/// Returns `None` if every generation is newer than the event (i.e.
/// the event predates the oldest known generation).
pub fn pin_event_to_generation(
  generations: &[(u64, u64)],
  event_ts_secs: u64,
) -> Option<u64> {
  generations
    .iter()
    .filter(|(_, mtime)| *mtime <= event_ts_secs)
    .max_by_key(|(_, mtime)| *mtime)
    .map(|(n, _)| *n)
}

/// Combine generations + events into the GenerationEvents rows.
/// Events that predate every generation are dropped (they have no
/// home). Generations with zero events still appear (so the user
/// sees the full history, including "quiet" rebuilds).
pub fn build_rows(
  generations: Vec<(u64, u64)>,
  current_gen: Option<u64>,
  events: Vec<Event>,
) -> Vec<GenerationEvents> {
  let mut rows: Vec<GenerationEvents> = generations
    .iter()
    .map(|(n, mtime)| GenerationEvents {
      number: *n,
      mtime_secs: *mtime,
      current: Some(*n) == current_gen,
      events: Vec::new(),
    })
    .collect();
  let gens_for_pin: Vec<(u64, u64)> = generations.clone();
  for ev in events {
    let Some(ts) = parse_rfc3339_to_unix(&ev.ts) else {
      continue;
    };
    let Some(gen_num) = pin_event_to_generation(&gens_for_pin, ts) else {
      continue;
    };
    if let Some(row) = rows.iter_mut().find(|r| r.number == gen_num) {
      row.events.push(ev);
    }
  }
  for row in &mut rows {
    row.events.sort_by(|a, b| a.ts.cmp(&b.ts));
  }
  rows
}

/// Resolve the currently-active generation number by reading
/// `/run/current-system` and parsing it back to a generation link
/// number. Returns `None` if that link doesn't resolve to a
/// `system-N-link` form (e.g. on a freshly booted system before
/// nixos-rebuild has ever run, or in a non-NixOS sandbox).
pub fn current_generation(profiles_dir: &Path) -> Option<u64> {
  // /run/current-system is a symlink to .../system → which is itself a
  // symlink to system-N-link. Read it twice.
  let cur = fs::read_link("/run/current-system").ok()?;
  let cur_canonical = fs::canonicalize(&cur).unwrap_or(cur);
  // The canonical resolution may end at the store path. Fall back to
  // walking profiles_dir to find which system-N-link points at the
  // same store path.
  let mut store_target = String::new();
  if let Some(s) = cur_canonical.to_str() {
    store_target = s.to_string();
  }
  for entry in fs::read_dir(profiles_dir).ok()? {
    let entry = entry.ok()?;
    let name = entry.file_name();
    let name_s = name.to_str()?;
    let Some(num) = parse_generation_link(name_s) else {
      continue;
    };
    if let Ok(target) = fs::canonicalize(entry.path())
      && let Some(t) = target.to_str()
      && t == store_target
    {
      return Some(num);
    }
  }
  None
}

// ── Subcommand entry point ─────────────────────────────────────────

use crate::{args::OsEventsArgs, timeline::read_events};

impl OsEventsArgs {
  /// Run `nh os events`.
  ///
  /// # Errors
  ///
  /// Returns an error if the system profiles directory can't be
  /// listed. Timeline read errors propagate (rare — `read_events`
  /// only fails when the file exists but is unreadable).
  pub fn run(self) -> Result<()> {
    let profiles_dir = self
      .profiles_dir
      .clone()
      .unwrap_or_else(|| PathBuf::from(SYSTEM_PROFILE_DIR));
    let generations = read_generations(&profiles_dir)?;
    if generations.is_empty() {
      println!(
        "No system generations found in {}.",
        profiles_dir.display()
      );
      return Ok(());
    }
    let current = current_generation(&profiles_dir);
    let events = read_events()?;
    let mut rows = build_rows(generations, current, events);
    rows.reverse(); // newest generation first
    let limit = self.limit.unwrap_or(10);
    let total = rows.len();
    let to_show = total.min(limit);
    println!("System generations ({to_show} of {total}):");
    for row in rows.into_iter().take(to_show) {
      let date = render_date(row.mtime_secs);
      let cur_marker = if row.current { " *" } else { "" };
      let count = row.events.len();
      let count_label = match count {
        0 => " (no cheni events)".to_string(),
        1 => " (1 event)".to_string(),
        n => format!(" ({n} events)"),
      };
      println!(
        "\n  Generation {}{cur_marker}  {date}{count_label}",
        row.number
      );
      for ev in row.events {
        let pkg = ev
          .package
          .as_deref()
          .map(|p| format!(" {p}"))
          .unwrap_or_default();
        println!("    - {} {}{}", ev.ts, ev.kind, pkg);
      }
    }
    Ok(())
  }
}

fn render_date(secs: u64) -> String {
  // Reuse timeline's RFC3339 formatter for consistency.
  crate::timeline::now_rfc3339_from_secs(secs)
}

#[cfg(test)]
#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
mod tests {
  use super::*;

  fn ev(ts: &str, kind: &str, package: Option<&str>) -> Event {
    Event {
      ts: ts.to_string(),
      kind: kind.to_string(),
      package: package.map(str::to_string),
      details: serde_json::json!({}),
    }
  }

  #[test]
  fn parse_generation_link_accepts_well_formed_names() {
    assert_eq!(parse_generation_link("system-1-link"), Some(1));
    assert_eq!(parse_generation_link("system-42-link"), Some(42));
    assert_eq!(parse_generation_link("system-1234-link"), Some(1234));
  }

  #[test]
  fn parse_generation_link_rejects_other_shapes() {
    assert_eq!(parse_generation_link("system"), None);
    assert_eq!(parse_generation_link("system-link"), None);
    assert_eq!(parse_generation_link("system-foo-link"), None);
    assert_eq!(parse_generation_link("system-1-bak"), None);
    assert_eq!(parse_generation_link("home-1-link"), None);
  }

  #[test]
  fn pin_event_picks_largest_gen_before_event() {
    let gens = vec![(10, 1_000), (11, 2_000), (12, 3_000)];
    assert_eq!(pin_event_to_generation(&gens, 1_500), Some(10));
    assert_eq!(pin_event_to_generation(&gens, 2_000), Some(11)); // edge: <=
    assert_eq!(pin_event_to_generation(&gens, 2_999), Some(11));
    assert_eq!(pin_event_to_generation(&gens, 3_000), Some(12));
    assert_eq!(pin_event_to_generation(&gens, 5_000), Some(12));
  }

  #[test]
  fn pin_event_returns_none_for_event_before_oldest_gen() {
    let gens = vec![(10, 1_000), (11, 2_000)];
    assert_eq!(pin_event_to_generation(&gens, 999), None);
  }

  #[test]
  fn build_rows_groups_events_into_correct_generations() {
    let gens = vec![(10, 1_000), (11, 2_000)];
    let events = vec![
      ev("1970-01-01T00:25:00Z", "pin", Some("foo")), // 1500 → gen 10
      ev("1970-01-01T00:50:00Z", "freeze", Some("bar")), // 3000 → gen 11
      ev("1970-01-01T00:00:00Z", "pin", Some("orphan")), // 0 → predates all
    ];
    let rows = build_rows(gens, Some(11), events);
    assert_eq!(rows.len(), 2);
    let gen10 = rows.iter().find(|r| r.number == 10).unwrap();
    assert_eq!(gen10.events.len(), 1);
    assert_eq!(gen10.events[0].package.as_deref(), Some("foo"));
    assert!(!gen10.current);
    let gen11 = rows.iter().find(|r| r.number == 11).unwrap();
    assert_eq!(gen11.events.len(), 1);
    assert_eq!(gen11.events[0].package.as_deref(), Some("bar"));
    assert!(gen11.current);
  }

  #[test]
  fn build_rows_keeps_generations_with_no_events() {
    let gens = vec![(10, 1_000), (11, 2_000), (12, 3_000)];
    let rows = build_rows(gens, None, Vec::new());
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r.events.is_empty()));
  }

  #[test]
  fn build_rows_skips_unparseable_event_timestamps() {
    let gens = vec![(10, 1_000)];
    let events =
      vec![ev("not-a-timestamp", "pin", Some("foo"))];
    let rows = build_rows(gens, None, events);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].events.is_empty());
  }

  #[test]
  fn build_rows_orders_events_within_generation_chronologically() {
    let gens = vec![(10, 0)];
    let events = vec![
      ev("2026-05-01T10:00:00Z", "pin", Some("c")),
      ev("2026-05-01T08:00:00Z", "pin", Some("a")),
      ev("2026-05-01T09:00:00Z", "pin", Some("b")),
    ];
    let rows = build_rows(gens, None, events);
    assert_eq!(rows[0].events.len(), 3);
    assert_eq!(
      rows[0]
        .events
        .iter()
        .map(|e| e.package.as_deref().unwrap_or("?"))
        .collect::<Vec<_>>(),
      vec!["a", "b", "c"]
    );
  }

  #[test]
  fn read_generations_picks_only_well_formed_links() {
    let dir = tempfile::TempDir::new().unwrap();
    fs::write(dir.path().join("system-3-link"), b"x").unwrap();
    fs::write(dir.path().join("system-1-link"), b"x").unwrap();
    fs::write(dir.path().join("system-2-link"), b"x").unwrap();
    fs::write(dir.path().join("README"), b"x").unwrap(); // ignored
    fs::write(dir.path().join("system-foo-link"), b"x").unwrap();
    let gens = read_generations(dir.path()).unwrap();
    assert_eq!(gens.len(), 3);
    assert_eq!(gens.iter().map(|g| g.0).collect::<Vec<_>>(), vec![1, 2, 3]);
  }

  #[test]
  fn read_generations_errors_when_dir_missing() {
    let dir = tempfile::TempDir::new().unwrap();
    let bad = dir.path().join("does-not-exist");
    assert!(read_generations(&bad).is_err());
  }
}
