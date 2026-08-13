//! `nh os pkg <name>` — inspect a package: installed vs available.
//!
//! Answers the everyday question "what version of this do I have, and
//! what's in nixpkgs?" without the `readlink -f $(which foo)` dance. It
//! shows, side by side:
//!   - the **installed** version (resolved from the current system), and
//!     the store path it lives in;
//!   - the **available** version in nixpkgs at the `flake.lock` rev;
//!   - a verdict (up to date / update available / installed is newer);
//!   - the nixpkgs `meta.description`;
//!   - whether the package is pinned or frozen via cheni.
//!
//! Installed resolution is best-effort, two strategies in order:
//!   1. `which <name>` in `$PATH`, canonicalized to its `/nix/store`
//!      path (this is exactly the `readlink` hack, done in Rust);
//!   2. fallback: scan the system profile's references
//!      (`nix-store -q --references /run/current-system/sw`) for a
//!      `-<name>-<version>` store name.
//!
//! A package that is neither on `$PATH` nor a direct system-profile
//! reference (e.g. a pure home-manager package with no same-named
//! binary) shows as "non installé". Documented limitation.
//!
//! # Helpers used (jump table for navigation)
//!
//! - `check::read_input_locked(dir, "nixpkgs")` + `check::query_pkg_version`
//!   / `check::query_pkg_description` — available version + description via
//!   a pure `fetchTree` `nix eval` (KDE6 `kdePackages.<name>` retry lives
//!   there).
//! - `versioning::{parse_version, compare_versions}` — the up-to-date
//!   verdict.
//! - `pins::{resolve_flake_dir, read}` / `freezes::read` — flake-dir
//!   resolution and the pin/freeze badges.
//! - `cheni_util::validation::package_name` — validate `name` BEFORE it
//!   flows into a `nix eval` expression or a store lookup.

use std::{
  env,
  path::{Path, PathBuf},
  process::Command,
};

use color_eyre::eyre::Result;

use crate::{
  args::OsPkgArgs,
  check,
  cheni_util::validation,
  freezes,
  pins,
  versioning::{VersionDiff, compare_versions, parse_version},
};

/// System profile whose references we scan as the installed-resolution
/// fallback.
const SYSTEM_SW: &str = "/run/current-system/sw";

/// A package resolved as installed on the current system.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Installed {
  version: String,
  store_path: PathBuf,
}

/// The nixpkgs side: version + description at the locked rev. Both
/// optional (package may be absent, or present without a description).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Available {
  version: Option<String>,
  description: Option<String>,
}

impl OsPkgArgs {
  /// Run `nh os pkg <name>`.
  ///
  /// # Errors
  ///
  /// Returns an error if `name` fails validation or the flake-dir can't
  /// be resolved. Per-source lookup failures (not installed, not in
  /// nixpkgs, `nix` unavailable) are tolerated and reported inline.
  pub fn run(self) -> Result<()> {
    // Validation BEFORE any splice/lookup: `name` reaches a Nix eval
    // expression and store-path lookups. Enforce the strict
    // Nix-attribute charset ([A-Za-z0-9_.+-]) HERE at the call site —
    // `package_name` alone still admits spaces/`;`/backticks, and the
    // only thing keeping those out of the eval today is `check.rs`'s
    // independent revalidation two calls away. Make the invariant local.
    validation::package_name(&self.name)?;
    validation::nix_attr_path(&self.name)?;
    let flake_dir = pins::resolve_flake_dir(self.flake_dir.as_deref())?;

    let installed = gather_installed(&self.name);
    let available = gather_available(&flake_dir, &self.name);

    print_pkg(&self.name, installed.as_ref(), &available, &flake_dir);
    Ok(())
  }
}

/// Resolve the installed version: `$PATH` binary first, then a scan of
/// the system profile's references.
fn gather_installed(name: &str) -> Option<Installed> {
  installed_via_path(name).or_else(|| installed_via_profile(name))
}

/// `which <name>` → canonicalize → parse the `/nix/store` name.
fn installed_via_path(name: &str) -> Option<Installed> {
  let bin = which(name)?;
  let canon = std::fs::canonicalize(&bin).ok()?;
  let sn = store_name(&canon)?;
  let version = store_version_for(&sn, name)
    .or_else(|| store_version_any(&sn).map(|(_, v)| v))?;
  Some(Installed {
    version,
    store_path: Path::new("/nix/store").join(&sn),
  })
}

/// Fallback: `nix-store -q --references /run/current-system/sw`, then
/// match a `-<name>-<version>` store name among the references.
fn installed_via_profile(name: &str) -> Option<Installed> {
  let out = Command::new("nix-store")
    .args(["-q", "--references", SYSTEM_SW])
    .output()
    .ok()?;
  if !out.status.success() {
    return None;
  }
  let text = String::from_utf8(out.stdout).ok()?;
  let refs: Vec<&str> = text.lines().collect();
  find_installed_in_refs(&refs, name)
}

/// Available version + description at the locked `nixpkgs` rev. If the
/// input isn't declared or the package doesn't resolve, both stay
/// `None`. The description eval is skipped unless the version resolved
/// (so we don't pay an eval for a package that isn't there).
fn gather_available(flake_dir: &Path, name: &str) -> Available {
  let Ok(locked) = check::read_input_locked(flake_dir, "nixpkgs") else {
    return Available::default();
  };
  // The input's recorded narHash may be a channel-tarball hash (when
  // `nixpkgs` is `channels.nixos.org/…`), which does NOT match a github
  // `fetchTree` of the same rev — the eval would fail. Re-derive the
  // github narHash from the rev so the query resolves regardless of the
  // input's flavour; fall back to the recorded hash if prefetch fails
  // (e.g. offline).
  let nar_hash = freezes::prefetch_nixpkgs_rev(&locked.rev)
    .unwrap_or(locked.nar_hash);
  let version = check::query_pkg_version(&locked.rev, &nar_hash, name);
  let description = version
    .as_ref()
    .and_then(|_| check::query_pkg_description(&locked.rev, &nar_hash, name));
  // These strings come from `nix eval --raw` (unescaped) — sanitize
  // before they ever reach the terminal.
  Available {
    version: version.map(|v| clean_for_display(&v)),
    description: description.map(|d| clean_for_display(&d)),
  }
}

/// Sanitize a string sourced from `nix eval --raw` before printing it:
/// drop control characters (which could smuggle ANSI escape sequences
/// onto the terminal from a tampered `flake.lock` or anomalous upstream
/// metadata) and cap the length. Pure.
fn clean_for_display(s: &str) -> String {
  let cleaned: String = s.chars().filter(|c| !c.is_control()).collect();
  if cleaned.chars().count() > 200 {
    let mut truncated: String = cleaned.chars().take(200).collect();
    truncated.push('…');
    truncated
  } else {
    cleaned
  }
}

/// Render the readable block.
fn print_pkg(
  name: &str,
  installed: Option<&Installed>,
  available: &Available,
  flake_dir: &Path,
) {
  println!("Package: {name}");
  match installed {
    Some(i) => {
      println!("  installed:  {}", i.version);
      println!("  store path: {}", i.store_path.display());
    },
    None => println!("  installed:  (non installé)"),
  }
  match &available.version {
    Some(v) => println!("  nixpkgs:    {v}"),
    None => println!("  nixpkgs:    (introuvable au rev de flake.lock)"),
  }
  println!(
    "  → {}",
    render_verdict(installed.map(|i| i.version.as_str()), available.version.as_deref())
  );
  if let Some(d) = &available.description {
    println!("  desc:       {d}");
  }

  // cheni pin/freeze badges — read is best-effort, absence is silent.
  if pins::read(flake_dir).is_ok_and(|p| p.iter().any(|n| n == name)) {
    println!("  pin:        épinglé (nixpkgs-latest)");
  }
  if let Some(frozen_at) = freezes::read(flake_dir)
    .ok()
    .and_then(|f| f.get(name).map(|e| e.version.clone()))
  {
    println!("  freeze:     gelé (version {frozen_at})");
  }
}

/// The up-to-date verdict from the two version strings.
fn render_verdict(installed: Option<&str>, available: Option<&str>) -> String {
  match (installed, available) {
    (Some(i), Some(a)) => {
      match compare_versions(&parse_version(i), &parse_version(a)) {
        VersionDiff::Equal => "à jour (nixpkgs a la même version)".to_string(),
        VersionDiff::Minor | VersionDiff::Major => {
          format!("mise à jour dispo : {i} → {a}")
        },
        VersionDiff::Newer => {
          format!("installé plus récent que nixpkgs ({i} > {a})")
        },
      }
    },
    (Some(_), None) => {
      "pas trouvé dans nixpkgs au rev de flake.lock".to_string()
    },
    (None, Some(a)) => format!("non installé (dispo dans nixpkgs : {a})"),
    (None, None) => "introuvable (ni installé, ni dans nixpkgs)".to_string(),
  }
}

// ── Store-path parsing (pure) ──────────────────────────────────────

/// The store name after the `<hash>-` prefix. `abc123-gparted-1.8.1`
/// → `gparted-1.8.1`. `None` if there's no `-` (malformed).
fn strip_hash(store_name: &str) -> Option<&str> {
  store_name.split_once('-').map(|(_, rest)| rest)
}

/// The `/nix/store/<name>` directory name of a canonicalized path.
/// `/nix/store/abc-gparted-1.8.1/bin/gparted` → `abc-gparted-1.8.1`.
fn store_name(path: &Path) -> Option<String> {
  let rel = path.strip_prefix("/nix/store").ok()?;
  rel
    .components()
    .next()
    .map(|c| c.as_os_str().to_string_lossy().into_owned())
}

/// Extract the version when the package name is KNOWN: the store name
/// (post-hash) must be exactly `<name>-<version>` and the version must
/// start with a digit. Exact, so it can't misattribute a dashed name.
fn store_version_for(store_name: &str, name: &str) -> Option<String> {
  let rest = strip_hash(store_name)?;
  let after = rest.strip_prefix(name)?.strip_prefix('-')?;
  after
    .starts_with(|c: char| c.is_ascii_digit())
    .then(|| after.to_string())
}

/// Heuristic split when the pname is UNKNOWN: version starts at the
/// first `-`-segment that begins with a digit. `abc-gtk+3-3.24.0`
/// → (`gtk+3`, `3.24.0`). Returns `None` if no such segment, or if the
/// very first segment is already numeric (no room for a pname).
fn store_version_any(store_name: &str) -> Option<(String, String)> {
  let rest = strip_hash(store_name)?;
  let segs: Vec<&str> = rest.split('-').collect();
  let idx = segs
    .iter()
    .position(|s| s.starts_with(|c: char| c.is_ascii_digit()))?;
  if idx == 0 {
    return None;
  }
  Some((segs[..idx].join("-"), segs[idx..].join("-")))
}

/// First reference whose store name is `<hash>-<name>-<version>`.
fn find_installed_in_refs(refs: &[&str], name: &str) -> Option<Installed> {
  refs.iter().find_map(|r| {
    let sn = store_name(Path::new(r))?;
    let version = store_version_for(&sn, name)?;
    Some(Installed {
      version,
      store_path: Path::new("/nix/store").join(&sn),
    })
  })
}

// ── `$PATH` lookup (impure, thin) ──────────────────────────────────

/// First executable-looking `<dir>/<name>` on `$PATH`.
fn which(name: &str) -> Option<PathBuf> {
  let path = env::var_os("PATH")?;
  env::split_paths(&path)
    .map(|dir| dir.join(name))
    .find(|cand| cand.is_file())
}

#[cfg(test)]
#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
mod tests {
  use super::*;

  #[test]
  fn strip_hash_drops_the_leading_hash_segment() {
    assert_eq!(strip_hash("abc123-gparted-1.8.1"), Some("gparted-1.8.1"));
    assert_eq!(strip_hash("nohyphen"), None);
  }

  #[test]
  fn store_name_extracts_the_store_dir() {
    assert_eq!(
      store_name(Path::new("/nix/store/abc-gparted-1.8.1/bin/gparted"))
        .as_deref(),
      Some("abc-gparted-1.8.1")
    );
    // the store dir itself (a reference, no trailing path)
    assert_eq!(
      store_name(Path::new("/nix/store/xyz-firefox-130.0")).as_deref(),
      Some("xyz-firefox-130.0")
    );
    // outside the store → None
    assert_eq!(store_name(Path::new("/usr/bin/gparted")), None);
  }

  #[test]
  fn store_version_for_exact_name_match() {
    assert_eq!(
      store_version_for("abc-gparted-1.8.1", "gparted").as_deref(),
      Some("1.8.1")
    );
    // dashed version part is kept whole
    assert_eq!(
      store_version_for("h-foo-1.2.0-unstable-2024", "foo").as_deref(),
      Some("1.2.0-unstable-2024")
    );
  }

  #[test]
  fn store_version_for_rejects_wrong_name_or_nonnumeric() {
    // name is a prefix but the remainder isn't a version
    assert_eq!(store_version_for("h-gtk+3-3.24.0", "gtk"), None);
    // different package entirely
    assert_eq!(store_version_for("h-firefox-130.0", "gparted"), None);
    // no version segment
    assert_eq!(store_version_for("h-gparted", "gparted"), None);
  }

  #[test]
  fn store_version_any_heuristic_split() {
    assert_eq!(
      store_version_any("h-gtk+3-3.24.0"),
      Some(("gtk+3".to_string(), "3.24.0".to_string()))
    );
    assert_eq!(
      store_version_any("h-nss-cacert-3.98"),
      Some(("nss-cacert".to_string(), "3.98".to_string()))
    );
    assert_eq!(
      store_version_any("h-python3-3.11.5"),
      Some(("python3".to_string(), "3.11.5".to_string()))
    );
  }

  #[test]
  fn store_version_any_none_when_no_version_or_pname() {
    assert_eq!(store_version_any("h-justaname"), None);
    // first post-hash segment already numeric → no pname room
    assert_eq!(store_version_any("h-4ti2"), None);
  }

  #[test]
  fn find_installed_in_refs_matches_the_named_package() {
    let refs = [
      "/nix/store/aaa-glibc-2.40",
      "/nix/store/bbb-gparted-1.8.1",
      "/nix/store/ccc-gtk+3-3.24.0",
    ];
    let got = find_installed_in_refs(&refs, "gparted").unwrap();
    assert_eq!(got.version, "1.8.1");
    assert_eq!(
      got.store_path,
      Path::new("/nix/store/bbb-gparted-1.8.1")
    );
  }

  #[test]
  fn find_installed_in_refs_none_when_absent() {
    let refs = ["/nix/store/aaa-glibc-2.40"];
    assert!(find_installed_in_refs(&refs, "gparted").is_none());
  }

  #[test]
  fn verdict_up_to_date_when_equal() {
    assert_eq!(
      render_verdict(Some("1.8.1"), Some("1.8.1")),
      "à jour (nixpkgs a la même version)"
    );
  }

  #[test]
  fn verdict_update_available_when_nixpkgs_newer() {
    assert_eq!(
      render_verdict(Some("1.8.0"), Some("1.8.1")),
      "mise à jour dispo : 1.8.0 → 1.8.1"
    );
    assert_eq!(
      render_verdict(Some("1.8.0"), Some("2.0.0")),
      "mise à jour dispo : 1.8.0 → 2.0.0"
    );
  }

  #[test]
  fn verdict_installed_newer_than_nixpkgs() {
    assert_eq!(
      render_verdict(Some("2.0.0"), Some("1.9.0")),
      "installé plus récent que nixpkgs (2.0.0 > 1.9.0)"
    );
  }

  #[test]
  fn clean_for_display_strips_control_chars() {
    // ESC (0x1b) is a control char → dropped, defanging the escape.
    assert_eq!(clean_for_display("git\u{1b}[31m2.0"), "git[31m2.0");
    assert_eq!(clean_for_display("plain 1.2.3"), "plain 1.2.3");
    // newlines and tabs are control chars too
    assert_eq!(clean_for_display("a\nb\tc"), "abc");
  }

  #[test]
  fn clean_for_display_truncates_overlong() {
    let out = clean_for_display(&"x".repeat(250));
    assert_eq!(out.chars().count(), 201); // 200 kept + ellipsis
    assert!(out.ends_with('…'));
  }

  #[test]
  fn verdict_missing_sides() {
    assert_eq!(
      render_verdict(Some("1.0"), None),
      "pas trouvé dans nixpkgs au rev de flake.lock"
    );
    assert_eq!(
      render_verdict(None, Some("1.0")),
      "non installé (dispo dans nixpkgs : 1.0)"
    );
    assert_eq!(
      render_verdict(None, None),
      "introuvable (ni installé, ni dans nixpkgs)"
    );
  }
}
