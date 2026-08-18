//! `nh os gen` — browse NixOS generations.
//!
//! Modes:
//!   - list (default): every generation with date, closure size, size
//!     delta vs the previous one, kernel, and current `→` / booted `●`
//!     markers (which differ after a switch without reboot). `--changes`
//!     adds a per-line `+added -removed ~changed` package summary (opt-in
//!     — it diffs each consecutive pair, so it's not free).
//!   - `<N>`: a detail view of one generation (full metadata + its
//!     package diff versus the previous generation).
//!   - `--diff A [B]`: the package diff between generations A and B
//!     (B defaults to the current generation).
//!
//! Coupling to nh is kept to what the shipped `gen` already used —
//! `generations::{describe, from_dir}` (read-only) and
//! `nh_diff::print_dix_diff`. The `--changes` package counts are computed
//! HERE from `nix-store --requisites` + cheni's own store-name parsing
//! (no `dix` dependency), so a future upstream diff-engine change can't
//! break them.

use std::{
  collections::HashMap,
  path::{Path, PathBuf},
  process::Command,
};

use color_eyre::eyre::{Result, bail};
use tracing::debug;

use crate::{args::OsGenArgs, generations, pkg, versioning::parse_version};

const DEFAULT_PROFILE: &str = "/nix/var/nix/profiles/system";
const BOOTED_SYSTEM: &str = "/run/booted-system";

impl OsGenArgs {
  /// Run `nh os gen`.
  ///
  /// # Errors
  ///
  /// Returns an error if the profile is missing, a requested generation
  /// doesn't exist, or the diff can't be produced.
  pub fn run(self) -> Result<()> {
    let profile =
      PathBuf::from(self.profile.as_deref().unwrap_or(DEFAULT_PROFILE));
    if let Some(sel) = self.diff.as_ref() {
      return run_diff(&profile, sel);
    }
    if let Some(n) = self.generation {
      return run_detail(&profile, n);
    }
    run_list(&profile, self.changes)
  }
}

/// One generation row for the list.
#[derive(Debug, Clone)]
struct Row {
  number:        u64,
  date:          String,
  nixos_version: String,
  kernel:        String,
  current:       bool,
  booted:        bool,
  /// Closure size in bytes; `None` when `path-info` couldn't resolve it.
  size:          Option<u64>,
  link:          PathBuf,
}

fn run_list(profile: &Path, changes: bool) -> Result<()> {
  let links = gen_links(profile)?;
  if links.is_empty() {
    println!("Aucune génération trouvée sous {}.", profile.display());
    return Ok(());
  }
  let refs: Vec<&Path> = links.iter().map(PathBuf::as_path).collect();
  let sizes = raw_closure_sizes(&refs);
  let booted = std::fs::read_link(BOOTED_SYSTEM).ok();

  let mut rows: Vec<Row> = links
    .iter()
    .filter_map(|link| {
      // Dummy size so `describe` doesn't re-run `nix path-info` per
      // generation (N+1) — sizes come from the single batched call.
      let info = generations::describe(link, Some(String::new()))?;
      Some(Row {
        number:        info.number,
        date:          info.date,
        nixos_version: info.nixos_version,
        kernel:        info.kernel_version,
        current:       info.current,
        booted:        link.read_link().ok().as_deref() == booted.as_deref(),
        size:          sizes.get(link).copied(),
        link:          link.clone(),
      })
    })
    .collect();
  rows.sort_by_key(|r| r.number);

  let counts = changes.then(|| compute_change_counts(&rows));
  print_list(&rows, counts.as_deref());
  Ok(())
}

fn run_detail(profile: &Path, n: u64) -> Result<()> {
  let link = gen_link_path(profile, n);
  if !link.exists() {
    bail!("génération {n} introuvable ({})", link.display());
  }
  let Some(info) = generations::describe(&link, None) else {
    bail!("impossible de décrire la génération {n}");
  };
  let booted = std::fs::read_link(BOOTED_SYSTEM).ok();
  let is_booted = link.read_link().ok().as_deref() == booted.as_deref();
  print_detail(&info, is_booted);

  let numbers: Vec<u64> = gen_links(profile)?
    .iter()
    .filter_map(|l| generations::from_dir(l))
    .collect();
  match previous_generation(&numbers, n) {
    Some(prev) => {
      println!("\nDiff paquets vs génération {prev} :");
      nh_diff::print_dix_diff(&gen_link_path(profile, prev), &link)?;
    },
    None => println!("\n(première génération — pas de diff)"),
  }
  Ok(())
}

fn run_diff(profile: &Path, sel: &[u64]) -> Result<()> {
  let Some(&a) = sel.first() else {
    bail!("--diff attend au moins un numéro de génération");
  };
  let link_a = gen_link_path(profile, a);
  if !link_a.exists() {
    bail!("génération {a} introuvable ({})", link_a.display());
  }
  // Second operand: the requested generation, or the current one (the
  // profile symlink itself resolves to it).
  let link_b = match sel.get(1) {
    Some(&b) => {
      let p = gen_link_path(profile, b);
      if !p.exists() {
        bail!("génération {b} introuvable ({})", p.display());
      }
      p
    },
    None => profile.to_path_buf(),
  };
  nh_diff::print_dix_diff(&link_a, &link_b)
}

// ── generation enumeration / paths ─────────────────────────────────

/// All generation links of `profile` (`<dir>/<name>-<N>-link`).
fn gen_links(profile: &Path) -> Result<Vec<PathBuf>> {
  if !profile.is_symlink() {
    bail!("aucun profil `{}`", profile.display());
  }
  let dir = profile.parent().unwrap_or_else(|| Path::new("."));
  let prefix = profile
    .file_name()
    .and_then(|n| n.to_str())
    .unwrap_or_default();
  let mut out = Vec::new();
  for entry in std::fs::read_dir(dir)? {
    let path = entry?.path();
    if path
      .file_name()
      .and_then(|n| n.to_str())
      .is_some_and(|n| n.starts_with(prefix) && n != prefix)
    {
      out.push(path);
    }
  }
  Ok(out)
}

/// The generation link path for number `n` under `profile`:
/// `<dir>/<profile-name>-<n>-link`. Pure.
fn gen_link_path(profile: &Path, n: u64) -> PathBuf {
  let dir = profile.parent().unwrap_or_else(|| Path::new("."));
  let name = profile
    .file_name()
    .and_then(|s| s.to_str())
    .unwrap_or("system");
  dir.join(format!("{name}-{n}-link"))
}

/// The largest generation number strictly below `n` (handles gaps).
/// Pure.
fn previous_generation(numbers: &[u64], n: u64) -> Option<u64> {
  numbers.iter().copied().filter(|&x| x < n).max()
}

// ── raw closure sizes (own fetch; upstream exposes only formatted) ──

/// Closure size in bytes for each generation link, via a single
/// `nix path-info -S --json`. Best-effort: an entry is absent when it
/// can't be resolved. Not pure (shells out); the parsing is delegated to
/// the pure [`parse_closure_sizes`].
fn raw_closure_sizes(links: &[&Path]) -> HashMap<PathBuf, u64> {
  if links.is_empty() {
    return HashMap::new();
  }
  let targets: Vec<PathBuf> = links
    .iter()
    .map(|l| l.read_link().unwrap_or_else(|_| l.to_path_buf()))
    .collect();
  let out = match Command::new("nix")
    // `--` so a link path can never be reparsed as a flag (defence in
    // depth; the prefix filter already blocks leading '-').
    .args(["path-info", "-S", "--json", "--"])
    .args(links)
    .output()
  {
    Ok(o) if o.status.success() => o.stdout,
    Ok(o) => {
      debug!(
        "gen: path-info failed: {}",
        String::from_utf8_lossy(&o.stderr).trim()
      );
      return HashMap::new();
    },
    Err(e) => {
      debug!("gen: path-info spawn failed: {e}");
      return HashMap::new();
    },
  };
  let by_store = parse_closure_sizes(&String::from_utf8_lossy(&out));
  links
    .iter()
    .zip(targets.iter())
    .filter_map(|(link, target)| {
      by_store
        .get(&*target.to_string_lossy())
        .map(|&sz| ((*link).to_path_buf(), sz))
    })
    .collect()
}

/// Parse `nix path-info --json` output into `store-path → closureSize`.
/// Handles both the array shape (`[{path, closureSize}]`) and the object
/// shape (`{path: {closureSize}}`). Pure.
fn parse_closure_sizes(json_text: &str) -> HashMap<String, u64> {
  let Ok(json) = serde_json::from_str::<serde_json::Value>(json_text) else {
    return HashMap::new();
  };
  let mut out = HashMap::new();
  if let Some(arr) = json.as_array() {
    for entry in arr {
      if let (Some(p), Some(sz)) = (
        entry.get("path").and_then(serde_json::Value::as_str),
        entry.get("closureSize").and_then(serde_json::Value::as_u64),
      ) {
        out.insert(p.to_string(), sz);
      }
    }
  } else if let Some(obj) = json.as_object() {
    for (p, v) in obj {
      if let Some(sz) =
        v.get("closureSize").and_then(serde_json::Value::as_u64)
      {
        out.insert(p.clone(), sz);
      }
    }
  }
  out
}

/// `(added, removed, changed)` package counts.
type Counts = (usize, usize, usize);
/// Per-generation change counts (`None` for the first row / when a
/// package set couldn't be resolved).
type ChangeRow = Option<Counts>;

// ── package-change counts (own fetch; no dix dependency) ───────────

/// Package `pname → numeric version` of a generation's closure, from
/// `nix-store --requisites` + cheni store-name parsing. Outputs (`-dev`,
/// `-lib`, …) collapse under one pname since the version's non-numeric
/// tail is dropped by [`parse_version`]. Best-effort (empty on failure).
fn package_versions(link: &Path) -> HashMap<String, Vec<u64>> {
  let out = match Command::new("nix-store")
    // `--` so a link path can never be reparsed as a flag (matches
    // `raw_closure_sizes`; defence in depth).
    .args(["--query", "--requisites", "--"])
    .arg(link)
    .output()
  {
    Ok(o) if o.status.success() => o.stdout,
    Ok(o) => {
      debug!(
        "gen: nix-store failed: {}",
        String::from_utf8_lossy(&o.stderr).trim()
      );
      return HashMap::new();
    },
    Err(e) => {
      debug!("gen: nix-store spawn failed: {e}");
      return HashMap::new();
    },
  };
  let text = String::from_utf8_lossy(&out);
  let mut map = HashMap::new();
  for line in text.lines() {
    if let Some(name) = pkg::store_name(Path::new(line))
      && let Some((pname, ver)) = pkg::store_version_any(&name)
    {
      map.insert(pname, parse_version(&ver));
    }
  }
  map
}

/// Package set for each row, then the diff of each consecutive pair
/// (`None` for the first). Each set is computed once.
fn compute_change_counts(rows: &[Row]) -> Vec<ChangeRow> {
  let sets: Vec<HashMap<String, Vec<u64>>> =
    rows.iter().map(|r| package_versions(&r.link)).collect();
  sets
    .iter()
    .enumerate()
    .map(|(i, cur)| (i > 0).then(|| diff_counts(&sets[i - 1], cur)))
    .collect()
}

/// `(added, removed, changed)` package counts between two `pname →
/// version` maps. Pure.
fn diff_counts(
  old: &HashMap<String, Vec<u64>>,
  new: &HashMap<String, Vec<u64>>,
) -> Counts {
  let mut added = 0;
  let mut changed = 0;
  for (name, nv) in new {
    match old.get(name) {
      None => added += 1,
      Some(ov) if ov != nv => changed += 1,
      Some(_) => {},
    }
  }
  let removed = old.keys().filter(|k| !new.contains_key(*k)).count();
  (added, removed, changed)
}

// ── formatting (pure) ──────────────────────────────────────────────

/// The size delta of each row versus the previous one (input must be
/// sorted by generation number). `None` when either side's size is
/// unknown, or for the first row. Pure.
fn size_deltas(sizes: &[Option<u64>]) -> Vec<Option<i64>> {
  sizes
    .iter()
    .enumerate()
    .map(|(i, cur)| {
      if i == 0 {
        return None;
      }
      match (sizes[i - 1], cur) {
        #[expect(
          clippy::cast_possible_wrap,
          reason = "closure sizes fit i64 comfortably (< 8 EiB)"
        )]
        (Some(prev), Some(c)) => Some(*c as i64 - prev as i64),
        _ => None,
      }
    })
    .collect()
}

/// Human-readable byte size with an adaptive unit, so small deltas don't
/// collapse to a misleading `0.0 GB`. Pure.
#[expect(clippy::cast_precision_loss, reason = "display rounding only")]
fn human_bytes(bytes: u64) -> String {
  const KB: f64 = 1024.0;
  const MB: f64 = KB * 1024.0;
  const GB: f64 = MB * 1024.0;
  let b = bytes as f64;
  if b >= GB {
    format!("{:.1} GB", b / GB)
  } else if b >= MB {
    format!("{:.0} MB", b / MB)
  } else if b >= KB {
    format!("{:.0} KB", b / KB)
  } else {
    format!("{bytes} B")
  }
}

fn fmt_size(bytes: Option<u64>) -> String {
  bytes.map_or_else(|| "?".to_string(), human_bytes)
}

fn fmt_delta(delta: Option<i64>) -> String {
  delta.map_or_else(String::new, |d| {
    if d == 0 {
      return "±0".to_string();
    }
    let sign = if d > 0 { "+" } else { "-" };
    format!("{sign}{}", human_bytes(d.unsigned_abs()))
  })
}

/// `+added -removed ~changed`, or empty when not requested. Pure.
fn fmt_changes(counts: ChangeRow) -> String {
  counts.map_or_else(String::new, |(a, r, c)| format!("+{a} -{r} ~{c}"))
}

/// First 12 chars of a git rev (short form). Pure.
fn short_rev(rev: &str) -> String {
  rev.chars().take(12).collect()
}

/// Trim an RFC 3339 timestamp to `YYYY-MM-DD HH:MM` for a compact list.
/// Byte-boundary-safe; returns the input unchanged if it's shorter than
/// expected. Pure.
fn short_date(rfc: &str) -> String {
  match (rfc.get(..10), rfc.get(11..16), rfc.as_bytes().get(10)) {
    (Some(day), Some(hm), Some(b'T')) => format!("{day} {hm}"),
    _ => rfc.to_string(),
  }
}

fn print_list(rows: &[Row], changes: Option<&[ChangeRow]>) {
  let deltas = size_deltas(&rows.iter().map(|r| r.size).collect::<Vec<_>>());
  println!(
    "Générations ({}) — → courante, ● bootée{} :",
    rows.len(),
    if changes.is_some() { " · Δ paquets +a -r ~c" } else { "" }
  );
  // Newest first for reading; deltas were computed in ascending order.
  for (i, r) in rows.iter().enumerate().rev() {
    let mark = format!(
      "{}{}",
      if r.current { "→" } else { " " },
      if r.booted { "●" } else { " " },
    );
    let chg = changes
      .and_then(|c| c.get(i))
      .map_or_else(String::new, |o| fmt_changes(*o));
    println!(
      "{mark} {:>4}  {:>16}  {:>9}  {:>9}  {:<22}  {:<12} {}",
      r.number,
      short_date(&r.date),
      fmt_size(r.size),
      fmt_delta(deltas[i]),
      r.nixos_version,
      r.kernel,
      chg,
    );
  }
}

fn print_detail(info: &generations::GenerationInfo, booted: bool) {
  let mut tags = Vec::new();
  if info.current {
    tags.push("courante");
  }
  if booted {
    tags.push("bootée");
  }
  let tag = if tags.is_empty() {
    String::new()
  } else {
    format!("  [{}]", tags.join(", "))
  };
  println!("Génération {}{tag}", info.number);
  println!("  date       : {}", short_date(&info.date));
  println!("  nixos      : {}", info.nixos_version);
  println!("  kernel     : {}", info.kernel_version);
  println!("  closure    : {}", info.closure_size);
  if let Some(rev) = &info.configuration_revision {
    println!("  config rev : {}", short_rev(rev));
  }
  if let Some(specs) = &info.specialisations
    && !specs.is_empty()
  {
    println!("  spéciales  : {}", specs.join(", "));
  }
}

#[cfg(test)]
#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
mod tests {
  use super::*;

  #[test]
  fn gen_link_path_builds_the_expected_link() {
    let p = gen_link_path(Path::new("/nix/var/nix/profiles/system"), 42);
    assert_eq!(p, Path::new("/nix/var/nix/profiles/system-42-link"));
  }

  #[test]
  fn gen_link_path_respects_a_custom_profile_name() {
    let p = gen_link_path(Path::new("/nix/var/nix/profiles/foo"), 7);
    assert_eq!(p, Path::new("/nix/var/nix/profiles/foo-7-link"));
  }

  #[test]
  fn previous_generation_picks_largest_below() {
    let nums = [237, 238, 242, 244];
    assert_eq!(previous_generation(&nums, 242), Some(238));
    assert_eq!(previous_generation(&nums, 244), Some(242));
    assert_eq!(previous_generation(&nums, 237), None); // oldest
    assert_eq!(previous_generation(&nums, 999), Some(244));
  }

  #[test]
  fn diff_counts_adds_removes_changes() {
    let old: HashMap<String, Vec<u64>> = [
      ("firefox".to_string(), vec![130]),
      ("git".to_string(), vec![2, 54]),
      ("gone".to_string(), vec![1]),
    ]
    .into_iter()
    .collect();
    let new: HashMap<String, Vec<u64>> = [
      ("firefox".to_string(), vec![131]), // changed
      ("git".to_string(), vec![2, 54]),   // unchanged
      ("hello".to_string(), vec![2]),     // added
    ]
    .into_iter()
    .collect();
    assert_eq!(diff_counts(&old, &new), (1, 1, 1));
  }

  #[test]
  fn diff_counts_empty_sides() {
    let empty = HashMap::new();
    let one: HashMap<String, Vec<u64>> =
      std::iter::once(("a".to_string(), vec![1])).collect();
    assert_eq!(diff_counts(&empty, &one), (1, 0, 0));
    assert_eq!(diff_counts(&one, &empty), (0, 1, 0));
  }

  #[test]
  fn fmt_changes_formats_or_blanks() {
    assert_eq!(fmt_changes(Some((1, 2, 3))), "+1 -2 ~3");
    assert_eq!(fmt_changes(None), "");
  }

  #[test]
  fn short_rev_takes_twelve() {
    assert_eq!(short_rev("0dd31dbabcdef0123456"), "0dd31dbabcde");
    assert_eq!(short_rev("short"), "short");
  }

  #[test]
  fn size_deltas_first_is_none_then_signed_diffs() {
    let sizes =
      vec![Some(1_000_000_000), Some(1_500_000_000), Some(1_200_000_000)];
    assert_eq!(size_deltas(&sizes), vec![None, Some(500_000_000), Some(-300_000_000)]);
  }

  #[test]
  fn size_deltas_none_when_a_side_is_unknown() {
    let sizes = vec![Some(1_000), None, Some(2_000)];
    assert_eq!(size_deltas(&sizes), vec![None, None, None]);
  }

  #[test]
  fn parse_closure_sizes_array_shape() {
    let json = r#"[{"path":"/nix/store/a","closureSize":2048},
                   {"path":"/nix/store/b","closureSize":4096}]"#;
    let m = parse_closure_sizes(json);
    assert_eq!(m.get("/nix/store/a"), Some(&2048));
    assert_eq!(m.get("/nix/store/b"), Some(&4096));
  }

  #[test]
  fn parse_closure_sizes_object_shape() {
    let json = r#"{"/nix/store/a":{"closureSize":123}}"#;
    assert_eq!(parse_closure_sizes(json).get("/nix/store/a"), Some(&123));
  }

  #[test]
  fn parse_closure_sizes_tolerates_garbage() {
    assert!(parse_closure_sizes("not json").is_empty());
  }

  #[test]
  fn fmt_delta_signs_zero_and_blanks() {
    assert_eq!(fmt_delta(None), "");
    assert_eq!(fmt_delta(Some(0)), "±0");
    assert_eq!(fmt_delta(Some(1_073_741_824)), "+1.0 GB");
    assert_eq!(fmt_delta(Some(-1_073_741_824)), "-1.0 GB");
    assert_eq!(fmt_delta(Some(31_457_280)), "+30 MB");
  }

  #[test]
  fn fmt_size_unknown_is_question_mark() {
    assert_eq!(fmt_size(None), "?");
    assert_eq!(fmt_size(Some(1_073_741_824)), "1.0 GB");
    assert_eq!(fmt_size(Some(31_457_280)), "30 MB");
  }

  #[test]
  fn short_date_trims_to_minute() {
    assert_eq!(short_date("2026-08-17T08:48:37.417521418Z"), "2026-08-17 08:48");
    assert_eq!(short_date("2026-08-17"), "2026-08-17");
  }

  #[test]
  fn short_date_is_byte_boundary_safe() {
    let s = "0123456789T2345é";
    assert_eq!(short_date(s), s);
  }
}
