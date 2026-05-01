//! Per-package freezes — the inverse of pins.
//!
//! A frozen package is held at a specific `nixpkgs` revision while the
//! rest of the system continues to update. The on-disk file
//! `<flake-dir>/package-freezes.json` is keyed by package name and
//! carries the `rev` + `narHash` the package is pinned at, plus
//! diagnostic version/date fields.
//!
//! The format and semantics are unchanged from the wrapper-era cheni
//! (so an existing freeze file keeps working), with two MVP omissions
//! tracked for follow-up:
//!
//! - **`major_constraint`** is not yet wired through the new
//!   subcommand — strict freeze only. The on-disk schema still accepts
//!   the field (deserializes when present), so a wrapper-era freeze
//!   with `--major N` round-trips correctly through the new code path.
//! - **Reject-if-pinned** cross-check is deferred until phase-3 lands
//!   the version-cache + smart UX layer that needs to know about both
//!   sides anyway.

use std::{
  collections::BTreeMap,
  fs,
  io::Write,
  path::Path,
  process::Command,
};

use color_eyre::eyre::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::debug;

const FREEZES_FILE: &str = "package-freezes.json";

/// One frozen package entry. Field names match the wrapper-era schema
/// because the user's existing flake overlay reads them by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreezeEntry {
  /// Full nixpkgs git revision the package is held at.
  pub rev: String,
  /// `nix flake prefetch` narHash for the corresponding tarball
  /// (`sha256-…` SRI form). Lets the overlay's `fetchTree` be pure.
  #[serde(rename = "narHash")]
  pub nar_hash: String,
  /// Installed version at freeze time (diagnostic only).
  #[serde(default)]
  pub version: String,
  /// ISO `YYYY-MM-DD` date when the freeze was created (diagnostic).
  #[serde(default)]
  pub frozen_at: String,
  /// Major-version constraint. Always serialised when set so the
  /// wrapper-era format is preserved on round-trip; the new subcommand
  /// doesn't *write* this field yet (always `None` from new freezes).
  #[serde(
    default,
    skip_serializing_if = "Option::is_none",
    rename = "majorConstraint"
  )]
  pub major_constraint: Option<u32>,
}

/// Map form — `BTreeMap` so the on-disk JSON key order is deterministic.
pub type Freezes = BTreeMap<String, FreezeEntry>;

/// Read the current freeze map. Empty/missing/whitespace-only file
/// degrades to an empty map (matches `pins::read`).
pub fn read(flake_dir: &Path) -> Result<Freezes> {
  let path = flake_dir.join(FREEZES_FILE);
  if !path.exists() {
    debug!("no {} found", FREEZES_FILE);
    return Ok(BTreeMap::new());
  }
  let content = fs::read_to_string(&path)
    .with_context(|| format!("reading {}", path.display()))?;
  if content.trim().is_empty() {
    debug!("{} is empty, treating as no freezes", FREEZES_FILE);
    return Ok(BTreeMap::new());
  }
  serde_json::from_str(&content).with_context(|| {
    format!(
      "{} is not valid JSON.\n  Path: {}\n  Expected: {{ \"name\": \
       {{ \"rev\": \"...\", \"narHash\": \"sha256-...\", ... }} \
       }}\n  Fix: edit the file, or reset with: echo '{{}}' > {}",
      FREEZES_FILE,
      path.display(),
      path.display()
    )
  })
}

/// Atomically write the freeze map. Same rationale as `pins::write` —
/// the overlay reads this file at every Nix eval, so a half-written
/// state would break system rebuilds.
pub fn write(flake_dir: &Path, freezes: &Freezes) -> Result<()> {
  let path = flake_dir.join(FREEZES_FILE);
  let body = serde_json::to_string_pretty(freezes)
    .context("serializing freezes")?;
  atomic_write(&path, format!("{body}\n").as_bytes())?;
  debug!("wrote {} freezes to {}", freezes.len(), FREEZES_FILE);
  Ok(())
}

/// Add a freeze, replacing any previous entry for `name`. Returns
/// `true` when the name was newly frozen, `false` when we replaced an
/// existing entry.
pub fn add(flake_dir: &Path, name: &str, entry: FreezeEntry) -> Result<bool> {
  validate_package_name(name)?;
  validate_entry(&entry)?;
  let mut freezes = read(flake_dir)?;
  let inserted_new = !freezes.contains_key(name);
  freezes.insert(name.to_string(), entry);
  write(flake_dir, &freezes)?;
  Ok(inserted_new)
}

/// Remove freezes by name. Returns the names that were actually
/// removed.
pub fn remove(flake_dir: &Path, names: &[String]) -> Result<Vec<String>> {
  let mut freezes = read(flake_dir)?;
  let mut removed = Vec::new();
  for name in names {
    if freezes.remove(name).is_some() {
      removed.push(name.clone());
    }
  }
  write(flake_dir, &freezes)?;
  Ok(removed)
}

/// Remove every freeze. Returns how many were removed.
pub fn clear(flake_dir: &Path) -> Result<usize> {
  let freezes = read(flake_dir)?;
  let count = freezes.len();
  write(flake_dir, &BTreeMap::new())?;
  Ok(count)
}

// ── Validation ─────────────────────────────────────────────────────

fn validate_package_name(name: &str) -> Result<()> {
  if name.is_empty() {
    bail!("Package name is empty");
  }
  if name.len() > 128 {
    bail!(
      "Package name '{}…' is suspiciously long ({} chars, max 128)",
      &name.chars().take(20).collect::<String>(),
      name.len()
    );
  }
  if let Some(bad) = name.chars().find(|c| {
    c.is_control()
      || matches!(*c, '\n' | '\r' | '/' | '\\' | '"' | '\'')
  }) {
    bail!(
      "Package name '{}' contains an invalid character ({:?}).",
      name,
      bad
    );
  }
  Ok(())
}

fn validate_entry(entry: &FreezeEntry) -> Result<()> {
  // Git rev: 7..=64 hex chars (40 is normal, longer/shorter accepted
  // for short hashes and future hash-type evolution).
  if entry.rev.len() < 7 || entry.rev.len() > 64 {
    bail!(
      "Freeze rev has an unusual length ({} chars, expected 7..=64): \
       {:?}",
      entry.rev.len(),
      entry.rev
    );
  }
  if !entry.rev.chars().all(|c| c.is_ascii_hexdigit()) {
    bail!("Freeze rev is not a hex git hash: {:?}", entry.rev);
  }
  // narHash: SRI form sha256-… or sha512-…
  if !entry.nar_hash.starts_with("sha256-")
    && !entry.nar_hash.starts_with("sha512-")
  {
    bail!(
      "Freeze narHash should be SRI sha256-… or sha512-…: {:?}",
      entry.nar_hash
    );
  }
  if entry.nar_hash.len() > 200 {
    bail!(
      "Freeze narHash is suspiciously long ({} chars)",
      entry.nar_hash.len()
    );
  }
  if entry
    .nar_hash
    .chars()
    .any(|c| c.is_control() || c == '"' || c == '\\')
  {
    bail!(
      "Freeze narHash contains an invalid character: {:?}",
      entry.nar_hash
    );
  }
  for (field, value) in
    [("version", &entry.version), ("frozen_at", &entry.frozen_at)]
  {
    if value.chars().any(char::is_control) {
      bail!(
        "Freeze {} contains a control character: {:?}",
        field,
        value
      );
    }
    if value.len() > 128 {
      bail!(
        "Freeze {} is suspiciously long ({} chars)",
        field,
        value.len()
      );
    }
  }
  if let Some(n) = entry.major_constraint
    && n > 9999
  {
    bail!(
      "Freeze majorConstraint is implausibly large ({n}). Valid \
       majors are in 0..=9999."
    );
  }
  Ok(())
}

// ── nixpkgs rev + narHash detection (for the freeze command) ───────

/// Read the locked rev of the `nixpkgs` input from
/// `<flake-dir>/flake.lock`. Returns an error with an actionable
/// message when the lock file is missing or doesn't declare the
/// `nixpkgs` input.
pub fn read_nixpkgs_rev(flake_dir: &Path) -> Result<String> {
  let lock_path = flake_dir.join("flake.lock");
  let content = fs::read_to_string(&lock_path).with_context(|| {
    format!(
      "reading {} — did you run `nix flake update` at least once?",
      lock_path.display()
    )
  })?;
  let lock: Value = serde_json::from_str(&content)
    .with_context(|| format!("parsing {} as JSON", lock_path.display()))?;

  // flake.lock layout:
  //   { "nodes": { "<input-name>": { "locked": { "rev": "..." } } } }
  // The root node's "inputs" maps logical names to node names; for
  // nixpkgs the node is usually called "nixpkgs" too but we resolve
  // through root.inputs to be safe.
  let root_inputs = lock
    .pointer("/nodes/root/inputs")
    .and_then(Value::as_object)
    .ok_or_else(|| {
      color_eyre::eyre::eyre!(
        "{} has no /nodes/root/inputs object",
        lock_path.display()
      )
    })?;
  let nixpkgs_node = root_inputs
    .get("nixpkgs")
    .and_then(|v| v.as_str())
    .unwrap_or("nixpkgs");
  let rev = lock
    .pointer(&format!("/nodes/{nixpkgs_node}/locked/rev"))
    .and_then(Value::as_str)
    .ok_or_else(|| {
      color_eyre::eyre::eyre!(
        "Could not find /nodes/{}/locked/rev in {}.\nIs the nixpkgs \
         input declared in your flake?",
        nixpkgs_node,
        lock_path.display()
      )
    })?
    .to_string();
  Ok(rev)
}

/// Prefetch the `nixpkgs` tarball for `rev` and return its narHash
/// (`sha256-…` SRI form). Shells out to `nix flake prefetch --json`.
/// Skipped (returns an error) when `nix` isn't on PATH so the test
/// suite stays sandbox-friendly.
pub fn prefetch_nixpkgs_rev(rev: &str) -> Result<String> {
  let url = format!("github:NixOS/nixpkgs/{rev}");
  let output = Command::new("nix")
    .args(["flake", "prefetch", "--json", &url])
    .output()
    .with_context(|| {
      format!("spawning `nix flake prefetch --json {url}`")
    })?;
  if !output.status.success() {
    bail!(
      "nix flake prefetch failed (exit {}): {}",
      output.status,
      String::from_utf8_lossy(&output.stderr).trim()
    );
  }
  let parsed: Value = serde_json::from_slice(&output.stdout)
    .context("parsing `nix flake prefetch --json` stdout")?;
  // Newer nix returns `{ "hash": "sha256-...", "storePath": "..." }`.
  // Older nix used `narHash`. Accept either.
  let hash = parsed
    .get("hash")
    .or_else(|| parsed.get("narHash"))
    .and_then(Value::as_str)
    .ok_or_else(|| {
      color_eyre::eyre::eyre!(
        "`nix flake prefetch --json` returned no hash field: {parsed}"
      )
    })?;
  Ok(hash.to_string())
}

// ── Atomic write (duplicate of pins::atomic_write — extract once we
//    pull a third caller into a shared cheni-util crate, phase 3+) ──

fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
  let parent = path.parent().unwrap_or_else(|| Path::new("."));
  let tmp_name = format!(
    "{}.tmp.{}",
    path.file_name().and_then(|n| n.to_str()).unwrap_or("nh-fzs-tmp"),
    std::process::id()
  );
  let tmp = parent.join(tmp_name);
  {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
      use std::os::unix::fs::OpenOptionsExt;
      opts.mode(0o600);
    }
    let mut file = opts
      .open(&tmp)
      .with_context(|| format!("opening {} for write", tmp.display()))?;
    file
      .write_all(content)
      .with_context(|| format!("writing {}", tmp.display()))?;
    let _ = file.sync_all();
  }
  fs::rename(&tmp, path).with_context(|| {
    format!("renaming {} → {}", tmp.display(), path.display())
  })?;
  Ok(())
}

// ── Subcommand entry points ────────────────────────────────────────

use crate::{
  args::{OsFreezeArgs, OsUnfreezeArgs},
  pins::resolve_flake_dir,
};

impl OsFreezeArgs {
  /// Run `nh os freeze`. With no `name`, lists current freezes; with
  /// a name, queries the current nixpkgs rev + narHash and stores a
  /// new freeze entry.
  ///
  /// # Errors
  ///
  /// Returns an error if the flake dir can't be resolved, `flake.lock`
  /// can't be read, `nix flake prefetch` fails, or the file write
  /// fails.
  pub fn run(self) -> Result<()> {
    let flake_dir = resolve_flake_dir(self.flake_dir.as_deref())?;
    let Some(name) = self.name else {
      return list_freezes(&flake_dir);
    };
    freeze_one(&flake_dir, &name, self.version.as_deref())
  }
}

impl OsUnfreezeArgs {
  /// Run `nh os unfreeze`.
  ///
  /// # Errors
  ///
  /// Returns an error if the flake dir can't be resolved, the freezes
  /// file can't be read or written, or arguments are invalid.
  pub fn run(self) -> Result<()> {
    let flake_dir = resolve_flake_dir(self.flake_dir.as_deref())?;
    if self.all {
      let count = clear(&flake_dir)?;
      if count == 0 {
        println!("No freezes to clear.");
      } else {
        crate::timeline::record(
          "unfreeze-all",
          None,
          serde_json::json!({"count": count}),
        );
        println!("Cleared {count} freeze(s).");
      }
      return Ok(());
    }
    if self.names.is_empty() {
      bail!("Specify package names to unfreeze, or pass --all.");
    }
    let removed = remove(&flake_dir, &self.names)?;
    for name in &removed {
      crate::timeline::record(
        "unfreeze",
        Some(name),
        serde_json::json!({}),
      );
    }
    if removed.is_empty() {
      println!("None of the requested packages were frozen.");
    } else {
      println!("Unfrozen {}: {}", removed.len(), removed.join(", "));
    }
    Ok(())
  }
}

fn list_freezes(flake_dir: &Path) -> Result<()> {
  let freezes = read(flake_dir)?;
  if freezes.is_empty() {
    println!("No active freezes.");
    println!("Freeze a package: `nh os freeze <name>`");
    return Ok(());
  }
  println!("Active freezes ({}):", freezes.len());
  for (name, entry) in &freezes {
    let short = entry.rev.chars().take(7).collect::<String>();
    let mode = entry
      .major_constraint
      .map(|n| format!(" [major {n}]"))
      .unwrap_or_default();
    let version_tag = if entry.version.is_empty() {
      String::new()
    } else {
      format!(" {}", entry.version)
    };
    let date_tag = if entry.frozen_at.is_empty() {
      String::new()
    } else {
      format!(" (since {})", entry.frozen_at)
    };
    println!("  - {name}{version_tag}{mode}{date_tag} — rev {short}");
  }
  Ok(())
}

fn freeze_one(
  flake_dir: &Path,
  name: &str,
  version_override: Option<&str>,
) -> Result<()> {
  validate_package_name(name)?;

  let rev = read_nixpkgs_rev(flake_dir)?;
  println!("nixpkgs rev: {}", &rev[..rev.len().min(7)]);
  println!("Prefetching tarball for narHash (this needs network)…");
  let nar_hash = prefetch_nixpkgs_rev(&rev)?;
  println!("narHash: {nar_hash}");

  let entry = FreezeEntry {
    rev,
    nar_hash,
    version: version_override.unwrap_or("").to_string(),
    frozen_at: today_iso(),
    major_constraint: None,
  };
  let new = add(flake_dir, name, entry)?;
  crate::timeline::record(
    if new { "freeze" } else { "refreeze" },
    Some(name),
    serde_json::json!({"version": version_override.unwrap_or("")}),
  );
  let verb = if new { "Froze" } else { "Re-froze" };
  println!(
    "{verb} {name}. Run `nh os switch` to apply (your flake's overlay \
     reads {}).",
    flake_dir.join(FREEZES_FILE).display()
  );
  Ok(())
}

/// Today as `YYYY-MM-DD` (UTC). Manual decomposition to avoid pulling
/// `chrono` for one call — matches the wrapper-era pattern.
fn today_iso() -> String {
  let secs = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_secs())
    .unwrap_or(0);
  let days = secs / 86_400;
  let mut year = 1970i64;
  let mut remaining = days as i64;
  loop {
    let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
    let year_days = if leap { 366 } else { 365 };
    if remaining < year_days {
      break;
    }
    remaining -= year_days;
    year += 1;
  }
  let leap = (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0);
  let months =
    [31, if leap { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  let mut month = 1u32;
  for &m in &months {
    if remaining < m {
      break;
    }
    remaining -= m;
    month += 1;
  }
  let day = remaining as u32 + 1;
  format!("{year:04}-{month:02}-{day:02}")
}

#[cfg(test)]
#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
mod tests {
  use super::*;
  use tempfile::TempDir;

  fn fake_flake_dir() -> TempDir {
    let dir = TempDir::new().expect("creating tempdir");
    fs::write(dir.path().join("flake.nix"), b"# fake").unwrap();
    dir
  }

  fn good_entry() -> FreezeEntry {
    FreezeEntry {
      rev: "0123456789abcdef0123456789abcdef01234567".to_string(),
      nar_hash: "sha256-AAAA1111BBBB2222CCCC3333DDDD4444EEEE5555FFFF="
        .to_string(),
      version: "1.2.3".to_string(),
      frozen_at: "2026-05-01".to_string(),
      major_constraint: None,
    }
  }

  #[test]
  fn read_returns_empty_when_file_absent() {
    let dir = fake_flake_dir();
    assert_eq!(read(dir.path()).unwrap(), BTreeMap::new());
  }

  #[test]
  fn read_returns_empty_for_blank_file() {
    let dir = fake_flake_dir();
    fs::write(dir.path().join(FREEZES_FILE), b"   \n  ").unwrap();
    assert_eq!(read(dir.path()).unwrap(), BTreeMap::new());
  }

  #[test]
  fn read_parses_object_with_entry() {
    let dir = fake_flake_dir();
    fs::write(
      dir.path().join(FREEZES_FILE),
      br#"{"firefox":{"rev":"0123456789abcdef0123456789abcdef01234567","narHash":"sha256-AAAA="}}"#,
    )
    .unwrap();
    let m = read(dir.path()).unwrap();
    assert_eq!(m.len(), 1);
    assert!(m.contains_key("firefox"));
    assert!(m["firefox"].rev.starts_with("0123"));
    assert_eq!(m["firefox"].version, ""); // serde default
  }

  #[test]
  fn read_returns_error_on_invalid_json() {
    let dir = fake_flake_dir();
    fs::write(dir.path().join(FREEZES_FILE), b"not json").unwrap();
    assert!(read(dir.path()).is_err());
  }

  #[test]
  fn write_serializes_entry() {
    let dir = fake_flake_dir();
    let mut m = BTreeMap::new();
    m.insert("foo".to_string(), good_entry());
    write(dir.path(), &m).unwrap();
    let body =
      fs::read_to_string(dir.path().join(FREEZES_FILE)).unwrap();
    assert!(body.contains("foo"));
    assert!(body.contains("narHash"));
    assert!(body.ends_with('\n'));
  }

  #[test]
  fn write_uses_0600_permissions_unix() {
    let dir = fake_flake_dir();
    let mut m = BTreeMap::new();
    m.insert("x".to_string(), good_entry());
    write(dir.path(), &m).unwrap();
    #[cfg(unix)]
    {
      use std::os::unix::fs::PermissionsExt;
      let mode = fs::metadata(dir.path().join(FREEZES_FILE))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
      assert_eq!(mode, 0o600);
    }
  }

  #[test]
  fn add_returns_true_for_new_entry_false_for_replacement() {
    let dir = fake_flake_dir();
    assert!(add(dir.path(), "foo", good_entry()).unwrap());
    assert!(!add(dir.path(), "foo", good_entry()).unwrap());
  }

  #[test]
  fn add_validates_name_and_entry() {
    let dir = fake_flake_dir();
    assert!(add(dir.path(), "foo/bar", good_entry()).is_err());
    let mut bad = good_entry();
    bad.rev = "nothex".to_string();
    assert!(add(dir.path(), "foo", bad).is_err());
    let mut bad2 = good_entry();
    bad2.nar_hash = "md5-doesntcount".to_string();
    assert!(add(dir.path(), "foo", bad2).is_err());
  }

  #[test]
  fn remove_drops_present_names_only() {
    let dir = fake_flake_dir();
    add(dir.path(), "foo", good_entry()).unwrap();
    add(dir.path(), "bar", good_entry()).unwrap();
    let removed =
      remove(dir.path(), &["bar".into(), "missing".into()]).unwrap();
    assert_eq!(removed, vec!["bar".to_string()]);
    let after = read(dir.path()).unwrap();
    assert_eq!(after.len(), 1);
    assert!(after.contains_key("foo"));
  }

  #[test]
  fn clear_returns_count() {
    let dir = fake_flake_dir();
    add(dir.path(), "a", good_entry()).unwrap();
    add(dir.path(), "b", good_entry()).unwrap();
    assert_eq!(clear(dir.path()).unwrap(), 2);
    assert!(read(dir.path()).unwrap().is_empty());
  }

  #[test]
  fn read_nixpkgs_rev_works_on_minimal_lock() {
    let dir = fake_flake_dir();
    fs::write(
      dir.path().join("flake.lock"),
      br#"{"nodes":{"root":{"inputs":{"nixpkgs":"nixpkgs"}},"nixpkgs":{"locked":{"rev":"deadbeefcafebabe1234567890abcdef12345678"}}}}"#,
    )
    .unwrap();
    let rev = read_nixpkgs_rev(dir.path()).unwrap();
    assert_eq!(rev, "deadbeefcafebabe1234567890abcdef12345678");
  }

  #[test]
  fn read_nixpkgs_rev_errors_when_missing_lock() {
    let dir = fake_flake_dir(); // flake.nix only, no lock
    assert!(read_nixpkgs_rev(dir.path()).is_err());
  }

  #[test]
  fn read_nixpkgs_rev_errors_when_no_nixpkgs_input() {
    let dir = fake_flake_dir();
    fs::write(
      dir.path().join("flake.lock"),
      br#"{"nodes":{"root":{"inputs":{}}}}"#,
    )
    .unwrap();
    assert!(read_nixpkgs_rev(dir.path()).is_err());
  }

  #[test]
  fn today_iso_format_is_well_formed() {
    let s = today_iso();
    assert_eq!(s.len(), 10);
    assert_eq!(&s[4..5], "-");
    assert_eq!(&s[7..8], "-");
    assert!(s.starts_with("20"));
  }

  #[test]
  fn round_trip_preserves_major_constraint() {
    let dir = fake_flake_dir();
    let mut entry = good_entry();
    entry.major_constraint = Some(127);
    add(dir.path(), "foo", entry.clone()).unwrap();
    let read_back = read(dir.path()).unwrap();
    assert_eq!(read_back["foo"].major_constraint, Some(127));
  }
}
