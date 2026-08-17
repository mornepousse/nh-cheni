//! Corpus of nix EVAL errors that `error_clarify` did not recognize
//! with a specific clarifier — the ones that fell through to the generic
//! eval-summary fallback. Recorded so their frequency can guide which
//! new recognizer is worth adding next (`nh os errors`).
//!
//! Build failures are deliberately NOT recorded: a build failure already
//! gets the right treatment (its `.drv` + a `nix log` pointer), so it is
//! not a recognition gap.
//!
//! Storage: `$XDG_CACHE_HOME/cheni/error-corpus.json`, a map of a
//! normalized signature to its occurrence count, first/last-seen
//! timestamps, and a sample. Recording is best-effort and gated by
//! `CHENI_NO_ERROR_CORPUS`.
//!
//! # Helpers used
//!
//! - `cheni_util::cache::file` — the on-disk path.
//! - `cheni_util::atomic::write` — tmp + fsync + rename, 0o600.
//! - `cheni_util::time::now_rfc3339` — timestamps.
//! - `error_clarify::strip_ansi` — colour stripping during normalization.

use std::{
  collections::BTreeMap,
  fs,
  path::{Path, PathBuf},
};

use color_eyre::eyre::Result;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::{
  args::OsErrorsArgs,
  cheni_util::{atomic, cache, time},
  error_clarify::strip_ansi,
};

const CORPUS_FILE: &str = "error-corpus.json";
const SAMPLE_MAX: usize = 500;
const MAX_ENTRIES: usize = 1000;
const ENV_DISABLE: &str = "CHENI_NO_ERROR_CORPUS";

/// One clustered unrecognized error.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
  pub count:      u64,
  pub first_seen: String,
  pub last_seen:  String,
  pub sample:     String,
}

/// signature → entry. `BTreeMap` for a stable on-disk key order.
type Corpus = BTreeMap<String, Entry>;

#[must_use]
fn path() -> PathBuf {
  cache::file(CORPUS_FILE)
}

/// Reduce an error summary to a stable signature by erasing volatile
/// bits — store hashes and `line:col` positions — and collapsing
/// whitespace, so the same error clusters across runs and machines.
/// Pure.
#[must_use]
#[expect(
  clippy::expect_used,
  reason = "regex literals are compile-time-verifiable invariants"
)]
pub fn normalize(summary: &str) -> String {
  use std::sync::LazyLock;
  static STORE_HASH: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"/nix/store/[0-9a-z]{32}-").expect("valid regex")
  });
  static LINE_COL: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r":\d+:\d+").expect("valid regex"));
  let s = strip_ansi(summary);
  let s = STORE_HASH.replace_all(&s, "/nix/store/<hash>-");
  let s = LINE_COL.replace_all(&s, ":L:C");
  // Drop any remaining control chars (bell, stray ESC, NUL) so the
  // signature is terminal-safe when `nh os errors` prints it; keep
  // whitespace controls, which `split_whitespace` collapses next.
  let s: String =
    s.chars().filter(|c| !c.is_control() || c.is_whitespace()).collect();
  s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(s: &str, max: usize) -> String {
  if s.chars().count() > max {
    s.chars().take(max).collect::<String>() + "…"
  } else {
    s.to_string()
  }
}

/// Record one unrecognized eval-error summary. Best-effort: never
/// panics, never propagates — a corpus write must not disturb the error
/// the user is actually looking at. No-op when `CHENI_NO_ERROR_CORPUS`
/// is set.
pub fn record(summary: &str) {
  if std::env::var_os(ENV_DISABLE).is_some() {
    return;
  }
  if let Err(e) = record_at(&path(), summary) {
    debug!("error-corpus: {e}");
  }
}

/// The disk-touching core, path injected for parallel-safe testing.
///
/// # Errors
///
/// Returns an error if the corpus can't be read or written.
fn record_at(path: &Path, summary: &str) -> Result<()> {
  let sig = normalize(summary);
  if sig.is_empty() {
    return Ok(());
  }
  let mut corpus = read_at(path)?;
  let now = time::now_rfc3339();
  corpus
    .entry(sig)
    .and_modify(|e| {
      e.count += 1;
      e.last_seen.clone_from(&now);
    })
    .or_insert_with(|| Entry {
      count:      1,
      first_seen: now.clone(),
      last_seen:  now.clone(),
      sample:     truncate(summary.trim(), SAMPLE_MAX),
    });
  cap(&mut corpus);
  write_at(path, &corpus)
}

/// Bound growth: over the cap, drop the least-seen signatures (lowest
/// count, ties broken by oldest `last_seen`). Pure.
fn cap(corpus: &mut Corpus) {
  if corpus.len() <= MAX_ENTRIES {
    return;
  }
  let mut ranked: Vec<(String, u64, String)> = corpus
    .iter()
    .map(|(k, e)| (k.clone(), e.count, e.last_seen.clone()))
    .collect();
  ranked.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.2.cmp(&b.2)));
  let drop = corpus.len() - MAX_ENTRIES;
  for (k, _, _) in ranked.into_iter().take(drop) {
    corpus.remove(&k);
  }
}

/// Read the corpus. A missing file is an empty corpus; a corrupt file is
/// tolerated (treated as empty) so a bad write never wedges recording.
///
/// # Errors
///
/// Returns an error only if the file exists but cannot be read.
pub fn read() -> Result<Corpus> {
  read_at(&path())
}

fn read_at(path: &Path) -> Result<Corpus> {
  if !path.exists() {
    return Ok(Corpus::new());
  }
  let content = fs::read_to_string(path)?;
  Ok(serde_json::from_str(&content).unwrap_or_default())
}

fn write_at(path: &Path, corpus: &Corpus) -> Result<()> {
  // Create the cache dir (0o700) ourselves — `atomic::write` does not
  // create parents, so on a fresh machine the first write would ENOENT.
  if let Some(parent) = path.parent() {
    cache::ensure_dir(parent)?;
  }
  let json = serde_json::to_vec_pretty(corpus)?;
  atomic::write(path, &json)
}

/// Delete the corpus file. A missing file is not an error.
///
/// # Errors
///
/// Returns the IO error for any failure other than the file being absent.
pub fn clear() -> Result<()> {
  match fs::remove_file(path()) {
    Ok(()) => Ok(()),
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
    Err(e) => Err(e.into()),
  }
}

/// Signatures ranked most-frequent first (ties broken by most-recent).
/// Pure given the corpus.
#[must_use]
pub fn ranked(corpus: &Corpus) -> Vec<(&String, &Entry)> {
  let mut v: Vec<_> = corpus.iter().collect();
  v.sort_by(|a, b| {
    b.1.count.cmp(&a.1.count).then_with(|| b.1.last_seen.cmp(&a.1.last_seen))
  });
  v
}

impl OsErrorsArgs {
  /// Run `nh os errors`.
  ///
  /// # Errors
  ///
  /// Returns an error if the corpus can't be read or cleared.
  pub fn run(self) -> Result<()> {
    if self.clear {
      clear()?;
      println!("Corpus d'erreurs vidé.");
      return Ok(());
    }
    print_corpus(&read()?, self.limit.unwrap_or(20));
    Ok(())
  }
}

fn print_corpus(corpus: &Corpus, limit: usize) {
  if corpus.is_empty() {
    println!("Aucune erreur non reconnue collectée pour l'instant.");
    println!(
      "Le corpus se remplit quand une erreur d'éval nix n'est reconnue \
       par aucun clarificateur d'error_clarify."
    );
    return;
  }
  let ranked = ranked(corpus);
  let shown = ranked.len().min(limit);
  println!(
    "Erreurs nix non reconnues — {} motif(s), top {shown} par fréquence :",
    corpus.len()
  );
  for (sig, e) in ranked.iter().take(limit) {
    println!("  {:>4}×  vu {}  {}", e.count, e.last_seen, truncate(sig, 100));
  }
  if corpus.len() > limit {
    println!(
      "  … ({} de plus — `--limit N` pour en voir plus)",
      corpus.len() - limit
    );
  }
}

#[cfg(test)]
#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
mod tests {
  use super::*;

  #[test]
  fn normalize_erases_store_hashes() {
    let a = normalize(
      "coerce /nix/store/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-foo-1.0 x",
    );
    let b = normalize(
      "coerce /nix/store/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb-foo-1.0 x",
    );
    assert_eq!(a, b, "same error, different store hash → one signature");
    assert!(a.contains("/nix/store/<hash>-foo-1.0"), "got: {a}");
  }

  #[test]
  fn normalize_erases_line_col_and_collapses_whitespace() {
    let a = normalize("error at file.nix:12:5\n  detail");
    let b = normalize("error at file.nix:99:1   detail");
    assert_eq!(a, b);
    assert_eq!(a, "error at file.nix:L:C detail");
  }

  #[test]
  fn normalize_drops_non_whitespace_control_chars() {
    // bell (0x07) and a stray ESC (0x1b not starting a CSI) must not
    // survive into the terminal-printed signature.
    let sig = normalize("boom\u{07}\u{1b}end");
    assert_eq!(sig, "boomend");
    assert!(!sig.chars().any(char::is_control));
  }

  #[test]
  fn ranked_orders_by_count_desc() {
    let mut c = Corpus::new();
    c.insert(
      "rare".into(),
      Entry { count: 1, first_seen: "t0".into(), last_seen: "t0".into(), sample: String::new() },
    );
    c.insert(
      "common".into(),
      Entry { count: 9, first_seen: "t0".into(), last_seen: "t1".into(), sample: String::new() },
    );
    let r = ranked(&c);
    assert_eq!(r[0].0, "common");
    assert_eq!(r[1].0, "rare");
  }

  #[test]
  fn cap_drops_least_seen_over_the_limit() {
    let mut c = Corpus::new();
    for i in 0..(MAX_ENTRIES + 3) {
      // count 0 for the first 3 (least-seen), higher for the rest
      let count = u64::from(i >= 3);
      c.insert(
        format!("sig-{i:05}"),
        Entry { count, first_seen: "t".into(), last_seen: "t".into(), sample: String::new() },
      );
    }
    cap(&mut c);
    assert_eq!(c.len(), MAX_ENTRIES);
    // the three count-0 entries were dropped
    assert!(!c.contains_key("sig-00000"));
    assert!(!c.contains_key("sig-00002"));
    assert!(c.contains_key("sig-00003"));
  }

  #[test]
  fn record_at_dedups_and_counts() {
    let dir = tempfile::TempDir::new().unwrap();
    let p = dir.path().join(CORPUS_FILE);
    record_at(&p, "undefined variable 'foo'").unwrap();
    record_at(&p, "undefined variable 'foo'").unwrap();
    record_at(&p, "infinite recursion encountered").unwrap();
    let c = read_at(&p).unwrap();
    assert_eq!(c.len(), 2);
    assert_eq!(c[&normalize("undefined variable 'foo'")].count, 2);
    assert_eq!(c[&normalize("infinite recursion encountered")].count, 1);
  }

  #[test]
  fn record_at_ignores_empty_signature() {
    let dir = tempfile::TempDir::new().unwrap();
    let p = dir.path().join(CORPUS_FILE);
    record_at(&p, "   \n  ").unwrap();
    assert!(read_at(&p).unwrap().is_empty());
    assert!(!p.exists(), "no file written for an empty signature");
  }

  #[cfg(unix)]
  #[test]
  fn record_at_writes_file_mode_0600() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::TempDir::new().unwrap();
    let p = dir.path().join(CORPUS_FILE);
    record_at(&p, "some error").unwrap();
    let mode = fs::metadata(&p).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
  }
}
