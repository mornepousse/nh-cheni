//! Per-package pins to a `nixpkgs-latest` overlay.
//!
//! cheni-specific feature carried over from the wrapper-era. The pin
//! state lives in `<flake-dir>/package-pins.json` as a flat JSON array
//! of package names. The user's `flake.nix` is expected to declare an
//! overlay that reads this file and routes the listed packages from
//! a `nixpkgs-latest` input instead of the default `nixpkgs`.
//!
//! This module owns:
//! - the on-disk state file (read/write/atomic)
//! - the `nh os pin` and `nh os unpin` subcommand implementations
//!
//! No nh-upstream code is touched. The overlay-injection mechanism on
//! the user's flake-side is unchanged from the wrapper era, so an
//! existing `package-pins.json` keeps working through the migration.

use std::{
  fs,
  path::{Path, PathBuf},
};

use color_eyre::eyre::{Context, Result, bail};
use tracing::debug;

use crate::cheni_util::{atomic, validation};

const PINS_FILE: &str = "package-pins.json";

/// Read the current list of pinned packages.
///
/// Empty file is treated as "no pins" rather than a parse error — a
/// user who manually empties the JSON shouldn't see a serde error on
/// their next rebuild.
pub fn read(flake_dir: &Path) -> Result<Vec<String>> {
  let path = flake_dir.join(PINS_FILE);

  if !path.exists() {
    debug!("no {} found", PINS_FILE);
    return Ok(Vec::new());
  }

  let content = fs::read_to_string(&path)
    .with_context(|| format!("Failed to read {}", path.display()))?;

  if content.trim().is_empty() {
    debug!("{} is empty, treating as no pins", PINS_FILE);
    return Ok(Vec::new());
  }

  let pins: Vec<String> =
    serde_json::from_str(&content).with_context(|| {
      format!(
        "{} is not valid JSON.\n  Path: {}\n  Expected: a JSON array of \
         package names, e.g. [\"firefox\", \"mesa\"]\n  Fix: edit the file, \
         or reset with: echo '[]' > {}",
        PINS_FILE,
        path.display(),
        path.display()
      )
    })?;

  debug!("loaded {} pins", pins.len());
  Ok(pins)
}

/// Write the list of pinned packages atomically.
///
/// The file is read by the user's overlay at every Nix evaluation, so
/// a half-written/truncated state would break system rebuilds. Tmp +
/// rename guarantees readers see either old or new content, never a
/// mix.
pub fn write(flake_dir: &Path, pins: &[String]) -> Result<()> {
  let path = flake_dir.join(PINS_FILE);
  let body = serde_json::to_string_pretty(pins)
    .context("serializing pins to JSON")?;
  atomic::write(&path, format!("{body}\n").as_bytes())?;
  debug!("wrote {} pins to {}", pins.len(), PINS_FILE);
  Ok(())
}

/// Add packages to the pin list. Returns the names actually added
/// (duplicates skipped).
pub fn add(flake_dir: &Path, names: &[String]) -> Result<Vec<String>> {
  for name in names {
    validation::package_name(name)?;
  }

  let mut pins = read(flake_dir)?;
  let mut added = Vec::new();
  for name in names {
    if !pins.contains(name) {
      pins.push(name.clone());
      added.push(name.clone());
    } else {
      debug!("{} already pinned", name);
    }
  }
  pins.sort();
  write(flake_dir, &pins)?;
  Ok(added)
}

/// Remove packages from the pin list. Returns the names actually
/// removed.
pub fn remove(flake_dir: &Path, names: &[String]) -> Result<Vec<String>> {
  let mut pins = read(flake_dir)?;
  let mut removed = Vec::new();
  for name in names {
    if pins.contains(name) {
      pins.retain(|p| p != name);
      removed.push(name.clone());
    }
  }
  write(flake_dir, &pins)?;
  Ok(removed)
}

/// Remove all pins. Returns the count cleared.
pub fn clear(flake_dir: &Path) -> Result<usize> {
  let pins = read(flake_dir)?;
  let count = pins.len();
  write(flake_dir, &[])?;
  Ok(count)
}

/// Resolve which directory holds the user's NixOS flake.
///
/// Priority order:
///   1. `--flake-dir` CLI arg (when `cli` is `Some`)
///   2. `$NH_FLAKE` env var, when it points to a local path with a
///      `flake.nix`
///   3. `$CHENI_CONFIG` env var (back-compat with the wrapper era)
///   4. `~/nixos-config`
///   5. `/etc/nixos`
///
/// Returns an actionable error when none match — the user shouldn't
/// have to grep this code to figure out where to point us.
pub fn resolve_flake_dir(cli: Option<&Path>) -> Result<PathBuf> {
  if let Some(p) = cli {
    if has_flake(p) {
      return Ok(p.to_path_buf());
    }
    bail!(
      "--flake-dir '{}' does not contain a flake.nix",
      p.display()
    );
  }
  for var in ["NH_FLAKE", "CHENI_CONFIG"] {
    if let Ok(s) = std::env::var(var) {
      let p = PathBuf::from(&s);
      if has_flake(&p) {
        debug!("using ${} = {}", var, p.display());
        return Ok(p);
      }
    }
  }
  for fallback in ["~/nixos-config", "/etc/nixos"] {
    let expanded = if let Some(rest) = fallback.strip_prefix("~/") {
      // Use $HOME directly rather than pulling in the `dirs` crate
      // for one call. Matches what every shell does anyway.
      std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(rest))
    } else {
      Some(PathBuf::from(fallback))
    };
    if let Some(p) = expanded
      && has_flake(&p)
    {
      debug!("using fallback {}", p.display());
      return Ok(p);
    }
  }
  bail!(
    "Could not find your NixOS flake. Try one of:\n  --flake-dir \
     <path>\n  export NH_FLAKE=<path>\n  put flake.nix at \
     ~/nixos-config or /etc/nixos"
  )
}

fn has_flake(p: &Path) -> bool {
  p.is_dir() && p.join("flake.nix").is_file()
}

// ── Subcommand entry points ────────────────────────────────────────

use crate::args::{OsPinArgs, OsUnpinArgs};

impl OsPinArgs {
  /// Run `nh os pin`. With no `names`, lists current pins; with
  /// names, adds them.
  ///
  /// # Errors
  ///
  /// Returns an error if the flake dir can't be resolved, the pins
  /// file can't be read or written, or any name fails validation.
  pub fn run(self) -> Result<()> {
    let flake_dir = resolve_flake_dir(self.flake_dir.as_deref())?;
    if self.names.is_empty() {
      let pins = read(&flake_dir)?;
      if pins.is_empty() {
        println!("No active pins.");
        println!("Pin a package: `nh os pin <name>`");
      } else {
        println!("Active pins ({}):", pins.len());
        for name in &pins {
          println!("  - {name}");
        }
      }
      return Ok(());
    }
    let added = add(&flake_dir, &self.names)?;
    for name in &added {
      crate::timeline::record(
        "pin",
        Some(name),
        serde_json::json!({"flake_dir": flake_dir.display().to_string()}),
      );
    }
    if added.is_empty() {
      println!("All requested packages were already pinned.");
    } else {
      println!("Pinned {}: {}", added.len(), added.join(", "));
      println!(
        "Run `nh os switch` to apply (your flake's overlay reads \
         {}).",
        flake_dir.join(PINS_FILE).display()
      );
    }
    Ok(())
  }
}

impl OsUnpinArgs {
  /// Run `nh os unpin`. With `--all`, clears every pin; otherwise
  /// removes the listed names.
  ///
  /// # Errors
  ///
  /// Returns an error if the flake dir can't be resolved, the pins
  /// file can't be read or written, or arguments are invalid.
  pub fn run(self) -> Result<()> {
    let flake_dir = resolve_flake_dir(self.flake_dir.as_deref())?;
    if self.all {
      let count = clear(&flake_dir)?;
      if count == 0 {
        println!("No pins to clear.");
      } else {
        crate::timeline::record(
          "unpin-all",
          None,
          serde_json::json!({"count": count}),
        );
        println!("Cleared {count} pin(s).");
      }
      return Ok(());
    }
    if self.names.is_empty() {
      bail!("Specify package names to unpin, or pass --all.");
    }
    let removed = remove(&flake_dir, &self.names)?;
    for name in &removed {
      crate::timeline::record("unpin", Some(name), serde_json::json!({}));
    }
    if removed.is_empty() {
      println!("None of the requested packages were pinned.");
    } else {
      println!("Unpinned {}: {}", removed.len(), removed.join(", "));
    }
    Ok(())
  }
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

  #[test]
  fn read_returns_empty_when_file_absent() {
    let dir = fake_flake_dir();
    assert_eq!(read(dir.path()).unwrap(), Vec::<String>::new());
  }

  #[test]
  fn read_returns_empty_for_blank_file() {
    let dir = fake_flake_dir();
    fs::write(dir.path().join(PINS_FILE), b"   \n  ").unwrap();
    assert_eq!(read(dir.path()).unwrap(), Vec::<String>::new());
  }

  #[test]
  fn read_parses_array() {
    let dir = fake_flake_dir();
    fs::write(dir.path().join(PINS_FILE), br#"["foo","bar"]"#).unwrap();
    assert_eq!(
      read(dir.path()).unwrap(),
      vec!["foo".to_string(), "bar".to_string()]
    );
  }

  #[test]
  fn read_returns_error_on_invalid_json() {
    let dir = fake_flake_dir();
    fs::write(dir.path().join(PINS_FILE), b"not json {{{").unwrap();
    assert!(read(dir.path()).is_err());
  }

  #[test]
  fn write_creates_file_with_atomic_pattern() {
    let dir = fake_flake_dir();
    write(dir.path(), &["alpha".into(), "beta".into()]).unwrap();
    let body = fs::read_to_string(dir.path().join(PINS_FILE)).unwrap();
    assert!(body.contains("alpha"));
    assert!(body.contains("beta"));
    assert!(body.ends_with('\n'));
  }

  #[test]
  fn write_uses_0600_permissions_unix() {
    let dir = fake_flake_dir();
    write(dir.path(), &["x".into()]).unwrap();
    #[cfg(unix)]
    {
      use std::os::unix::fs::PermissionsExt;
      let mode = fs::metadata(dir.path().join(PINS_FILE))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
      assert_eq!(mode, 0o600);
    }
  }

  #[test]
  fn add_returns_only_new_names() {
    let dir = fake_flake_dir();
    write(dir.path(), &["existing".into()]).unwrap();
    let added = add(
      dir.path(),
      &["existing".into(), "fresh".into(), "fresh".into()],
    )
    .unwrap();
    // Duplicate "fresh" within the same call is added once.
    assert_eq!(added, vec!["fresh".to_string()]);
    let stored = read(dir.path()).unwrap();
    assert_eq!(stored, vec!["existing", "fresh"]); // sorted
  }

  #[test]
  fn add_rejects_invalid_names() {
    let dir = fake_flake_dir();
    assert!(add(dir.path(), &["foo/bar".into()]).is_err());
    assert!(add(dir.path(), &[String::new()]).is_err());
    assert!(add(dir.path(), &["with\nnewline".into()]).is_err());
    let too_long = "x".repeat(200);
    assert!(add(dir.path(), &[too_long]).is_err());
  }

  #[test]
  fn remove_only_drops_present_names() {
    let dir = fake_flake_dir();
    write(dir.path(), &["a".into(), "b".into(), "c".into()]).unwrap();
    let removed = remove(dir.path(), &["b".into(), "missing".into()])
      .unwrap();
    assert_eq!(removed, vec!["b".to_string()]);
    assert_eq!(read(dir.path()).unwrap(), vec!["a", "c"]);
  }

  #[test]
  fn clear_returns_count() {
    let dir = fake_flake_dir();
    write(dir.path(), &["a".into(), "b".into()]).unwrap();
    assert_eq!(clear(dir.path()).unwrap(), 2);
    assert_eq!(read(dir.path()).unwrap(), Vec::<String>::new());
  }

  #[test]
  fn validate_accepts_typical_nixpkgs_names() {
    // The validation logic moved to cheni_util::validation; this
    // test now covers the integration: add() must accept the same
    // names through the new path.
    let dir = fake_flake_dir();
    for n in &[
      "firefox",
      "linuxKernel",
      "kdePackages.kate",
      "gcc-13",
      "libfoo_2",
      "openssl_3+",
    ] {
      assert!(
        add(dir.path(), &[(*n).to_string()]).is_ok(),
        "should accept {n}"
      );
    }
  }

  #[test]
  fn resolve_flake_dir_honours_explicit_arg() {
    let dir = fake_flake_dir();
    let resolved = resolve_flake_dir(Some(dir.path())).unwrap();
    assert_eq!(resolved, dir.path());
  }

  #[test]
  fn resolve_flake_dir_rejects_arg_without_flake() {
    let dir = TempDir::new().unwrap(); // no flake.nix inside
    assert!(resolve_flake_dir(Some(dir.path())).is_err());
  }
}
