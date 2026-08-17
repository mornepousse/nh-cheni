//! `nh os gen` — browse NixOS generations.
//!
//! Two modes:
//!   - list (default): every generation with its date, closure size, and
//!     the size delta versus the previous generation, current one marked;
//!   - `--diff A [B]`: the package-level diff between generations A and B
//!     (B defaults to the current generation), via the same `dix` engine
//!     nh shows at switch time — but on demand, between any two.
//!
//! `nh os info` (upstream) already lists generations + closure size; this
//! adds the size delta and, crucially, the retrospective diff. Upstream
//! `generations.rs` is reused read-only (`describe`); raw closure sizes
//! for the delta are fetched here (upstream only exposes formatted ones)
//! so `generations.rs` stays untouched.

use std::{
  collections::HashMap,
  path::{Path, PathBuf},
  process::Command,
};

use color_eyre::eyre::{Result, bail};
use tracing::debug;

use crate::{args::OsGenArgs, generations};

const DEFAULT_PROFILE: &str = "/nix/var/nix/profiles/system";

impl OsGenArgs {
  /// Run `nh os gen`.
  ///
  /// # Errors
  ///
  /// Returns an error if the profile is missing, a requested generation
  /// doesn't exist, or the `dix` diff can't be produced.
  pub fn run(self) -> Result<()> {
    let profile =
      PathBuf::from(self.profile.as_deref().unwrap_or(DEFAULT_PROFILE));
    self.diff.as_ref().map_or_else(
      || run_list(&profile),
      |sel| run_diff(&profile, sel),
    )
  }
}

/// One generation row for the list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Row {
  number:        u64,
  date:          String,
  nixos_version: String,
  current:       bool,
  /// Closure size in bytes; `None` when `path-info` couldn't resolve it.
  size:          Option<u64>,
}

fn run_list(profile: &Path) -> Result<()> {
  let links = gen_links(profile)?;
  if links.is_empty() {
    println!("Aucune génération trouvée sous {}.", profile.display());
    return Ok(());
  }
  let refs: Vec<&Path> = links.iter().map(PathBuf::as_path).collect();
  let sizes = raw_closure_sizes(&refs);

  let mut rows: Vec<Row> = links
    .iter()
    .filter_map(|link| {
      // Pass a dummy size so `describe` doesn't re-run `nix path-info`
      // per generation (N+1) — `Row.size` comes from the single batched
      // `raw_closure_sizes` call above; `info.closure_size` is unused.
      let info = generations::describe(link, Some(String::new()))?;
      Some(Row {
        number:        info.number,
        date:          info.date,
        nixos_version: info.nixos_version,
        current:       info.current,
        size:          sizes.get(link).copied(),
      })
    })
    .collect();
  rows.sort_by_key(|r| r.number);
  print_list(&rows);
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

/// Trim an RFC 3339 timestamp to `YYYY-MM-DD HH:MM` for a compact list.
/// Pure; returns the input unchanged if it's shorter than expected.
fn short_date(rfc: &str) -> String {
  // "2026-08-17T08:48:37.417Z" → "2026-08-17 08:48".
  // Byte-boundary-safe (`.get`) — no panic on unexpected multibyte input.
  match (rfc.get(..10), rfc.get(11..16), rfc.as_bytes().get(10)) {
    (Some(day), Some(hm), Some(b'T')) => format!("{day} {hm}"),
    _ => rfc.to_string(),
  }
}

fn print_list(rows: &[Row]) {
  let deltas = size_deltas(&rows.iter().map(|r| r.size).collect::<Vec<_>>());
  println!("Générations ({}) — taille closure et Δ vs précédente :", rows.len());
  // Newest first for reading; deltas were computed in ascending order.
  for (i, r) in rows.iter().enumerate().rev() {
    let marker = if r.current { "→" } else { " " };
    println!(
      "{marker} {:>4}  {:>16}  {:>9}  {:>9}  {}",
      r.number,
      short_date(&r.date),
      fmt_size(r.size),
      fmt_delta(deltas[i]),
      r.nixos_version,
    );
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
  fn size_deltas_first_is_none_then_signed_diffs() {
    let sizes =
      vec![Some(1_000_000_000), Some(1_500_000_000), Some(1_200_000_000)];
    assert_eq!(size_deltas(&sizes), vec![None, Some(500_000_000), Some(-300_000_000)]);
  }

  #[test]
  fn size_deltas_none_when_a_side_is_unknown() {
    let sizes = vec![Some(1_000), None, Some(2_000)];
    // row1: prev Some / cur None → None; row2: prev None / cur Some → None
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
    // a small delta reads in MB, not a misleading "0.0 GB"
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
    // shorter / unexpected input is returned unchanged
    assert_eq!(short_date("2026-08-17"), "2026-08-17");
  }

  #[test]
  fn short_date_is_byte_boundary_safe() {
    // byte 16 lands mid-'é' (2 bytes at 15-16): byte-indexed slicing
    // would panic here; `.get` must fall back to the input unchanged.
    let s = "0123456789T2345é";
    assert_eq!(short_date(s), s);
  }
}
