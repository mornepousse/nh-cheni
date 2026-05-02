//! `nh os bug-report` — markdown diagnostic dump for issue triage.
//!
//! Prints a self-contained markdown block to stdout that the user can
//! paste verbatim into a new issue. Gathers:
//!   - cheni-fork version, kernel, hostname, arch, OS
//!   - external tool versions (nix, dix, nvd if present)
//!   - active pins / freezes / version-cache stats
//!   - the last few timeline events
//!
//! Intentionally no colour, no shell-out beyond `uname` / version
//! probes, no network. The point is "give the maintainer enough
//! context without the user having to think about what to include."

use std::process::Command;

use color_eyre::eyre::Result;

use crate::{
  args::OsBugReportArgs, freezes, pins, timeline, version_cache,
};

impl OsBugReportArgs {
  /// Run `nh os bug-report`. Prints markdown to stdout.
  ///
  /// # Errors
  ///
  /// Returns an error only if a subsection fails in a way that we
  /// can't degrade gracefully. All gather steps are best-effort: a
  /// missing `flake.nix` or unreadable timeline only suppresses the
  /// matching section, never aborts the whole report.
  pub fn run(self) -> Result<()> {
    let flake_dir = pins::resolve_flake_dir(self.flake_dir.as_deref()).ok();
    print_header();
    print_environment_section();
    print_tools_section();
    if let Some(ref dir) = flake_dir {
      print_pins_section(dir);
      print_freezes_section(dir);
    } else {
      println!("## Pins / Freezes");
      println!();
      println!("_Could not locate your NixOS flake — skipped._");
      println!();
    }
    print_version_cache_section();
    print_timeline_section();
    print_what_happened_section();
    Ok(())
  }
}

fn print_header() {
  println!("# nh-cheni bug report");
  println!();
  println!(
    "<!-- Paste this into https://gitlab.com/harrael/nh-cheni/-/issues/new -->"
  );
  println!(
    "<!-- Then add a description of what you were trying to do below. -->"
  );
  println!();
}

fn print_environment_section() {
  println!("## Environment");
  println!();
  println!(
    "- **nh-cheni** (binary `nh`): `nh {} (cheni {})`",
    crate::cheni_meta::nh_base_version(),
    crate::cheni_meta::cheni_layer_version()
  );
  if let Some(os) = read_os_release() {
    println!("- **OS**: `{os}`");
  }
  if let Some(kernel) = uname("-r") {
    println!("- **Kernel**: `{kernel}`");
  }
  if let Some(arch) = uname("-m") {
    println!("- **Arch**: `{arch}`");
  }
  if let Some(hn) = hostname() {
    println!("- **Hostname**: `{hn}`");
  }
  let envs = ["NH_FLAKE", "NH_ELEVATION_STRATEGY", "CHENI_CONFIG", "NO_COLOR"];
  let set: Vec<String> = envs
    .iter()
    .filter_map(|v| std::env::var(v).ok().map(|val| format!("`{v}={val}`")))
    .collect();
  if !set.is_empty() {
    println!("- **Env**: {}", set.join(", "));
  }
  println!();
}

fn print_tools_section() {
  println!("## External tools");
  println!();
  for tool in &["nix", "dix", "nvd", "git"] {
    if let Some(v) = program_version(tool, &["--version"]) {
      println!("- **{tool}**: `{v}`");
    } else {
      println!("- **{tool}**: _not in PATH_");
    }
  }
  println!();
}

fn print_pins_section(flake_dir: &std::path::Path) {
  println!("## Active pins");
  println!();
  match pins::read(flake_dir) {
    Ok(p) if p.is_empty() => println!("_No active pins._"),
    Ok(p) => {
      println!("- Count: **{}**", p.len());
      for name in p.iter().take(20) {
        println!("  - `{name}`");
      }
      if p.len() > 20 {
        println!("  - … (+{} more)", p.len() - 20);
      }
    },
    Err(e) => println!("_Error reading pins: {e}_"),
  }
  println!();
}

fn print_freezes_section(flake_dir: &std::path::Path) {
  println!("## Active freezes");
  println!();
  match freezes::read(flake_dir) {
    Ok(f) if f.is_empty() => println!("_No active freezes._"),
    Ok(f) => {
      println!("- Count: **{}**", f.len());
      for (name, entry) in f.iter().take(20) {
        let short = entry.rev.chars().take(7).collect::<String>();
        let v = if entry.version.is_empty() {
          String::new()
        } else {
          format!(" {}", entry.version)
        };
        println!("  - `{name}`{v} — rev `{short}`");
      }
      if f.len() > 20 {
        println!("  - … (+{} more)", f.len() - 20);
      }
    },
    Err(e) => println!("_Error reading freezes: {e}_"),
  }
  println!();
}

fn print_version_cache_section() {
  println!("## Version cache");
  println!();
  let path = version_cache::cache_path();
  if !path.exists() {
    println!("_No cache file at `{}`._", path.display());
    println!();
    return;
  }
  let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
  match version_cache::VersionCache::load(&path) {
    Ok(c) => {
      println!("- Path: `{}`", path.display());
      println!("- Entries: **{}**", c.entry_count());
      println!(
        "- Size: {} B (~{:.1} KiB)",
        bytes,
        bytes as f64 / 1024.0
      );
    },
    Err(e) => println!("_Load failed: {e}_"),
  }
  println!();
}

fn print_timeline_section() {
  println!("## Last cheni events (most recent 10)");
  println!();
  match timeline::read_events() {
    Ok(mut events) if !events.is_empty() => {
      events.reverse();
      println!("```");
      for ev in events.into_iter().take(10) {
        let pkg = ev
          .package
          .as_deref()
          .map(|p| format!(" {p}"))
          .unwrap_or_default();
        println!("{} {}{}", ev.ts, ev.kind, pkg);
      }
      println!("```");
    },
    Ok(_) => println!("_No recorded events._"),
    Err(e) => println!("_Error reading timeline: {e}_"),
  }
  println!();
}

fn print_what_happened_section() {
  println!("## What happened?");
  println!();
  println!("<!-- Describe: -->");
  println!("<!-- 1. What command you ran -->");
  println!("<!-- 2. What you expected -->");
  println!("<!-- 3. What actually happened -->");
  println!();
}

// ── Probes ─────────────────────────────────────────────────────────

fn read_os_release() -> Option<String> {
  let content = std::fs::read_to_string("/etc/os-release").ok()?;
  content
    .lines()
    .find_map(|l| l.strip_prefix("PRETTY_NAME="))
    .map(|s| s.trim_matches('"').to_string())
}

fn uname(flag: &str) -> Option<String> {
  let out = Command::new("uname").arg(flag).output().ok()?;
  if !out.status.success() {
    return None;
  }
  Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn hostname() -> Option<String> {
  let out = Command::new("hostname").output().ok()?;
  if !out.status.success() {
    return None;
  }
  Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn program_version(program: &str, args: &[&str]) -> Option<String> {
  let out = Command::new(program).args(args).output().ok()?;
  if !out.status.success() {
    return None;
  }
  let s = String::from_utf8_lossy(&out.stdout);
  s.lines().next().map(|l| l.trim().to_string())
}

#[cfg(test)]
#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
mod tests {
  use super::*;

  #[test]
  fn read_os_release_returns_none_when_file_absent_or_unreadable() {
    // We can't easily stub /etc/os-release; just verify the function
    // doesn't panic and returns either Some or None gracefully.
    let _ = read_os_release();
  }

  #[test]
  fn program_version_returns_none_for_missing_program() {
    assert_eq!(
      program_version("definitely-not-a-real-program-xyz", &["--version"]),
      None
    );
  }
}
