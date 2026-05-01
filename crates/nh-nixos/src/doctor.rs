//! `nh os doctor` — sanity checks on the cheni-fork setup.
//!
//! Reports per-check `ok` / `warn` / `error` rows so the user can
//! quickly spot what needs attention. The MVP intentionally skips
//! checks that need a Nix evaluation (obsolete-pin detection,
//! nixpkgs-latest overlay shape, etc.) — those need the
//! repology/version-query layers planned for phase 5b.
//!
//! Best-effort throughout: a check that can't even run produces a
//! `warn` row with the underlying error, never aborts the whole
//! report.

use std::{
  fs,
  path::{Path, PathBuf},
  process::Command,
  time::{Duration, SystemTime},
};

use color_eyre::eyre::Result;

use crate::{
  args::OsDoctorArgs, freezes, pins, timeline, version_cache,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
  Ok,
  Warn,
  Err,
}

struct Check {
  severity: Severity,
  name: String,
  message: String,
  hint: Option<String>,
}

impl OsDoctorArgs {
  /// Run `nh os doctor`. Returns Ok always — the exit status only
  /// reflects orchestration failures (none currently expected). The
  /// presence of `warn`/`error` rows in the report is the signal
  /// the user reads, not a non-zero exit.
  ///
  /// # Errors
  ///
  /// Reserved for future check failures that warrant an exit-code
  /// signal (e.g. when piping to scripts). Currently always `Ok(())`.
  pub fn run(self) -> Result<()> {
    let flake_dir =
      pins::resolve_flake_dir(self.flake_dir.as_deref()).ok();
    let mut checks = Vec::new();

    checks.push(check_nix_in_path());
    checks.push(check_git_in_path());
    if let Some(ref dir) = flake_dir {
      checks.push(check_flake_nix_present(dir));
      checks.push(check_pins_readable(dir));
      checks.push(check_freezes_readable(dir));
      checks.push(check_flake_lock_age(dir));
    } else {
      checks.push(Check {
        severity: Severity::Warn,
        name: "flake-dir".to_string(),
        message:
          "Could not locate your NixOS flake (tried --flake-dir, \
           $NH_FLAKE, $CHENI_CONFIG, ~/nixos-config, /etc/nixos)."
            .to_string(),
        hint: Some(
          "Pass --flake-dir, set $NH_FLAKE, or put your flake at \
           ~/nixos-config."
            .to_string(),
        ),
      });
    }
    checks.push(check_timeline_readable());
    checks.push(check_version_cache_loadable());
    checks.push(check_active_rebuild_lock(flake_dir.as_deref()));

    print_report(&checks, self.brief);
    Ok(())
  }
}

// ── Individual checks ──────────────────────────────────────────────

fn check_nix_in_path() -> Check {
  match Command::new("nix").arg("--version").output() {
    Ok(o) if o.status.success() => Check {
      severity: Severity::Ok,
      name: "nix".to_string(),
      message: String::from_utf8_lossy(&o.stdout)
        .lines()
        .next()
        .unwrap_or("ok")
        .trim()
        .to_string(),
      hint: None,
    },
    _ => Check {
      severity: Severity::Err,
      name: "nix".to_string(),
      message: "`nix` is not in PATH or returned non-zero".to_string(),
      hint: Some(
        "cheni-fork needs Nix. Install it via your distribution or \
         from https://nixos.org/download.html"
          .to_string(),
      ),
    },
  }
}

fn check_git_in_path() -> Check {
  match Command::new("git").arg("--version").output() {
    Ok(o) if o.status.success() => Check {
      severity: Severity::Ok,
      name: "git".to_string(),
      message: String::from_utf8_lossy(&o.stdout)
        .lines()
        .next()
        .unwrap_or("ok")
        .trim()
        .to_string(),
      hint: None,
    },
    _ => Check {
      severity: Severity::Warn,
      name: "git".to_string(),
      message: "`git` is not in PATH".to_string(),
      hint: Some(
        "Flakes evaluate against git-tracked files only. Without \
         git, `nh os switch` may build the wrong tree on a dirty \
         checkout."
          .to_string(),
      ),
    },
  }
}

fn check_flake_nix_present(flake_dir: &Path) -> Check {
  let path = flake_dir.join("flake.nix");
  if path.is_file() {
    Check {
      severity: Severity::Ok,
      name: "flake.nix".to_string(),
      message: format!("present at {}", path.display()),
      hint: None,
    }
  } else {
    Check {
      severity: Severity::Err,
      name: "flake.nix".to_string(),
      message: format!("not found at {}", path.display()),
      hint: Some(
        "Initialise a flake there or point --flake-dir at the \
         right place."
          .to_string(),
      ),
    }
  }
}

fn check_pins_readable(flake_dir: &Path) -> Check {
  match pins::read(flake_dir) {
    Ok(p) => Check {
      severity: Severity::Ok,
      name: "pins".to_string(),
      message: format!("{} active pin(s)", p.len()),
      hint: None,
    },
    Err(e) => Check {
      severity: Severity::Err,
      name: "pins".to_string(),
      message: format!("read failed: {e}"),
      hint: Some(format!(
        "Inspect or reset {}/package-pins.json",
        flake_dir.display()
      )),
    },
  }
}

fn check_freezes_readable(flake_dir: &Path) -> Check {
  match freezes::read(flake_dir) {
    Ok(f) => Check {
      severity: Severity::Ok,
      name: "freezes".to_string(),
      message: format!("{} active freeze(s)", f.len()),
      hint: None,
    },
    Err(e) => Check {
      severity: Severity::Err,
      name: "freezes".to_string(),
      message: format!("read failed: {e}"),
      hint: Some(format!(
        "Inspect or reset {}/package-freezes.json",
        flake_dir.display()
      )),
    },
  }
}

fn check_flake_lock_age(flake_dir: &Path) -> Check {
  let lock = flake_dir.join("flake.lock");
  if !lock.exists() {
    return Check {
      severity: Severity::Warn,
      name: "flake.lock".to_string(),
      message: "missing".to_string(),
      hint: Some(
        "Run `nix flake update` once to generate the lock file."
          .to_string(),
      ),
    };
  }
  let age_days = match fs::metadata(&lock).and_then(|m| m.modified()) {
    Ok(t) => SystemTime::now()
      .duration_since(t)
      .unwrap_or_default()
      .as_secs()
      / 86_400,
    Err(_) => 0,
  };
  match age_days {
    0..=30 => Check {
      severity: Severity::Ok,
      name: "flake.lock age".to_string(),
      message: format!("{age_days} day(s) old"),
      hint: None,
    },
    31..=90 => Check {
      severity: Severity::Warn,
      name: "flake.lock age".to_string(),
      message: format!("{age_days} day(s) old"),
      hint: Some(
        "Consider `nix flake update` to pull recent nixpkgs fixes."
          .to_string(),
      ),
    },
    _ => Check {
      severity: Severity::Warn,
      name: "flake.lock age".to_string(),
      message: format!("{age_days} day(s) old (very stale)"),
      hint: Some(
        "Stale lock = stale CVE patches. Run `nix flake update` \
         and rebuild."
          .to_string(),
      ),
    },
  }
}

fn check_timeline_readable() -> Check {
  match timeline::read_events() {
    Ok(ev) => Check {
      severity: Severity::Ok,
      name: "timeline".to_string(),
      message: format!("{} event(s) recorded", ev.len()),
      hint: None,
    },
    Err(e) => Check {
      severity: Severity::Warn,
      name: "timeline".to_string(),
      message: format!("read failed: {e}"),
      hint: Some(format!(
        "Inspect or remove {}",
        timeline::timeline_path().display()
      )),
    },
  }
}

fn check_version_cache_loadable() -> Check {
  let path = version_cache::cache_path();
  if !path.exists() {
    return Check {
      severity: Severity::Ok,
      name: "version-cache".to_string(),
      message: "no cache file (will be populated on first use)"
        .to_string(),
      hint: None,
    };
  }
  match version_cache::VersionCache::load(&path) {
    Ok(c) => Check {
      severity: Severity::Ok,
      name: "version-cache".to_string(),
      message: format!("{} cached entries", c.entry_count()),
      hint: None,
    },
    Err(e) => Check {
      severity: Severity::Warn,
      name: "version-cache".to_string(),
      message: format!("load failed: {e}"),
      hint: Some(format!(
        "Reset with: rm {}",
        path.display()
      )),
    },
  }
}

fn check_active_rebuild_lock(flake_dir: Option<&Path>) -> Check {
  // Common nh/nixos-rebuild lock locations, in order of likelihood.
  let candidates = [
    PathBuf::from("/var/lib/nixos-rebuild.lock"),
    PathBuf::from("/var/lock/nixos-rebuild.lock"),
  ];
  let mut maybe_lock: Option<PathBuf> = None;
  for c in candidates {
    if c.exists() {
      maybe_lock = Some(c);
      break;
    }
  }
  if let Some(_dir) = flake_dir {
    // Nothing flake-dir-specific to add yet — left here so the
    // signature is forward-compat with future per-flake locks.
  }
  match maybe_lock {
    None => Check {
      severity: Severity::Ok,
      name: "active rebuild".to_string(),
      message: "none detected".to_string(),
      hint: None,
    },
    Some(lock) => {
      let age_secs = fs::metadata(&lock)
        .and_then(|m| m.modified())
        .map(|t| {
          SystemTime::now()
            .duration_since(t)
            .unwrap_or_default()
            .as_secs()
        })
        .unwrap_or(0);
      // After 1h with no change the lock is almost certainly stale —
      // a real rebuild's lock is touched continuously.
      if age_secs > Duration::from_secs(3600).as_secs() {
        Check {
          severity: Severity::Warn,
          name: "active rebuild".to_string(),
          message: format!(
            "stale lock at {} ({} min old)",
            lock.display(),
            age_secs / 60
          ),
          hint: Some(
            "If no rebuild is running, the lock file can be removed \
             manually."
              .to_string(),
          ),
        }
      } else {
        Check {
          severity: Severity::Warn,
          name: "active rebuild".to_string(),
          message: format!(
            "lock present at {} ({} sec old)",
            lock.display(),
            age_secs
          ),
          hint: Some(
            "Another rebuild may be running. Wait or check `ps aux \
             | grep -E 'nix.*build'`."
              .to_string(),
          ),
        }
      }
    },
  }
}

// ── Reporting ─────────────────────────────────────────────────────

fn print_report(checks: &[Check], brief: bool) {
  let mut errors = Vec::new();
  let mut warnings = Vec::new();
  let mut oks = Vec::new();
  for c in checks {
    match c.severity {
      Severity::Err => errors.push(c),
      Severity::Warn => warnings.push(c),
      Severity::Ok => oks.push(c),
    }
  }

  if !errors.is_empty() {
    println!("\nErrors:");
    for c in &errors {
      print_check(c);
    }
  }
  if !warnings.is_empty() {
    println!("\nWarnings:");
    for c in &warnings {
      print_check(c);
    }
  }
  if !brief && !oks.is_empty() {
    println!("\nOK:");
    for c in &oks {
      print_check(c);
    }
  } else if brief && !oks.is_empty() {
    println!("\nOK: {} check(s) passed.", oks.len());
  }
  println!(
    "\nSummary: {} ok, {} warn, {} error.",
    oks.len(),
    warnings.len(),
    errors.len()
  );
}

fn print_check(c: &Check) {
  let tag = match c.severity {
    Severity::Ok => "[ok]",
    Severity::Warn => "[warn]",
    Severity::Err => "[error]",
  };
  println!("  {tag} {} — {}", c.name, c.message);
  if let Some(ref h) = c.hint {
    for line in h.lines() {
      println!("         hint: {line}");
    }
  }
}

#[cfg(test)]
#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
mod tests {
  use super::*;
  use tempfile::TempDir;

  fn fake_flake_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("flake.nix"), b"# fake").unwrap();
    dir
  }

  #[test]
  fn check_flake_nix_present_ok_when_file_exists() {
    let dir = fake_flake_dir();
    let c = check_flake_nix_present(dir.path());
    assert_eq!(c.severity, Severity::Ok);
  }

  #[test]
  fn check_flake_nix_present_err_when_file_missing() {
    let dir = TempDir::new().unwrap();
    let c = check_flake_nix_present(dir.path());
    assert_eq!(c.severity, Severity::Err);
    assert!(c.hint.is_some());
  }

  #[test]
  fn check_pins_readable_handles_missing_file() {
    let dir = fake_flake_dir();
    let c = check_pins_readable(dir.path());
    assert_eq!(c.severity, Severity::Ok);
    assert!(c.message.contains("0 active"));
  }

  #[test]
  fn check_pins_readable_reports_count() {
    let dir = fake_flake_dir();
    fs::write(
      dir.path().join("package-pins.json"),
      br#"["a","b","c"]"#,
    )
    .unwrap();
    let c = check_pins_readable(dir.path());
    assert_eq!(c.severity, Severity::Ok);
    assert!(c.message.contains("3 active"));
  }

  #[test]
  fn check_pins_readable_err_on_bad_json() {
    let dir = fake_flake_dir();
    fs::write(dir.path().join("package-pins.json"), b"not json")
      .unwrap();
    let c = check_pins_readable(dir.path());
    assert_eq!(c.severity, Severity::Err);
  }

  #[test]
  fn check_freezes_readable_reports_count() {
    let dir = fake_flake_dir();
    fs::write(
      dir.path().join("package-freezes.json"),
      br#"{"foo":{"rev":"abcdef0123456789abcdef0123456789abcdef01","narHash":"sha256-AAAA="}}"#,
    )
    .unwrap();
    let c = check_freezes_readable(dir.path());
    assert_eq!(c.severity, Severity::Ok);
    assert!(c.message.contains("1 active"));
  }

  #[test]
  fn check_flake_lock_age_warn_when_missing() {
    let dir = fake_flake_dir();
    let c = check_flake_lock_age(dir.path());
    assert_eq!(c.severity, Severity::Warn);
    assert!(c.message.contains("missing"));
  }

  #[test]
  fn check_flake_lock_age_ok_for_fresh_lock() {
    let dir = fake_flake_dir();
    fs::write(dir.path().join("flake.lock"), b"{}").unwrap();
    let c = check_flake_lock_age(dir.path());
    assert_eq!(c.severity, Severity::Ok);
  }

  #[test]
  fn check_active_rebuild_lock_ok_when_no_lock() {
    // The real /var/lib/nixos-rebuild.lock might or might not
    // exist on the test host. Just ensure the function returns
    // a Check without panicking.
    let _ = check_active_rebuild_lock(None);
  }
}
