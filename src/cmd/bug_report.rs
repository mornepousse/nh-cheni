//! `cheni bug-report` command.
//!
//! Gathers the diagnostic information a maintainer needs to triage an
//! issue — version, OS, NixOS state, doctor results — and prints it as
//! plain markdown so the user can paste the whole block into a new
//! issue without having to figure out what to include.

use std::path::Path;
use std::process::Command;

use anyhow::Result;

/// Run `cheni bug-report`.
///
/// Writes the report to stdout. Intentionally no colour — the output is
/// meant to be redirected or copy-pasted verbatim.
pub fn run() -> Result<()> {
    // Turn colour off globally for the duration — doctor, status, etc.
    // all inherit it and would otherwise emit ANSI escapes that make the
    // markdown unreadable in a GitLab/GitHub comment.
    colored::control::set_override(false);

    let version = env!("CARGO_PKG_VERSION");
    let git = env!("GIT_SHORT_HASH");

    println!("# cheni bug report");
    println!();
    println!(
        "<!-- Paste this into https://gitlab.com/harrael/cheni/-/issues/new --> "
    );
    println!("<!-- Then add a description of what you were trying to do below. -->");
    println!();

    // ── Environment ─────────────────────────────────────
    println!("## Environment");
    println!();
    println!("- **cheni**: `{} ({})`", version, git);
    if let Some(os) = read_os_release() {
        println!("- **OS**: `{}`", os);
    }
    if let Some(kernel) = uname_release() {
        println!("- **Kernel**: `{}`", kernel);
    }
    if let Some(arch) = uname_machine() {
        println!("- **Arch**: `{}`", arch);
    }
    if let Some(hn) = hostname() {
        println!("- **Hostname**: `{}`", hn);
    }
    if let Some(nh) = program_version("nh", &["--version"]) {
        println!("- **nh**: `{}`", nh);
    }
    if let Some(nvd) = program_version("nvd", &["--version"]) {
        println!("- **nvd**: `{}`", nvd);
    }
    if let Some(nix) = program_version("nix", &["--version"]) {
        println!("- **nix**: `{}`", nix);
    }
    println!();

    // ── Config overview ─────────────────────────────────
    println!("## Config overview");
    println!();
    match crate::nix::config::detect() {
        Ok(cfg) => {
            println!("- **Flake dir**: `{}`", cfg.flake_dir.display());
            println!("- **Hostname**: `{}`", cfg.hostname);
            println!(
                "- **Initialized**: `{}`",
                crate::nix::config::is_initialized(&cfg.flake_dir)
            );
            let categories = crate::nix::config::list_module_categories(&cfg.flake_dir);
            if !categories.is_empty() {
                println!("- **Module categories**: `{}`", categories.join(", "));
            }
            match crate::nix::pins::read(&cfg.flake_dir) {
                Ok(pins) => {
                    println!("- **Active pins**: `{}`", pins.len());
                    if !pins.is_empty() {
                        println!("  - `{}`", pins.join("`, `"));
                    }
                }
                Err(e) => println!("- **Active pins**: _error reading pins: {}_", e),
            }
            print_flake_inputs_summary(&cfg.flake_dir);
        }
        Err(e) => {
            println!("_Could not detect NixOS config: {}_", e);
        }
    }
    println!();

    // ── Doctor output ───────────────────────────────────
    println!("## Doctor");
    println!();
    println!("```");
    // Re-run doctor but capture output. Simpler: just re-call it.
    // Its output goes to stdout which is what we want here.
    if let Err(e) = super::doctor::run() {
        println!("_cheni doctor failed: {}_", e);
    }
    println!("```");
    println!();

    // ── Cache state ─────────────────────────────────────
    let cache = crate::api::cache::stats();
    println!("## Repology cache");
    println!();
    if !cache.exists {
        println!("- _No cache file_");
    } else {
        println!("- **Entries**: {}", cache.total_entries);
        println!("- **Null entries**: {}", cache.null_entries);
        println!(
            "- **Age**: {}s ({}m)",
            cache.age_secs,
            cache.age_secs / 60
        );
    }
    println!();

    // ── What happened ───────────────────────────────────
    println!("## What happened?");
    println!();
    println!("<!-- Describe: -->");
    println!("<!-- 1. What command you ran -->");
    println!("<!-- 2. What you expected -->");
    println!("<!-- 3. What actually happened (paste the output if possible) -->");
    println!();

    Ok(())
}

// ── helpers ────────────────────────────────────────────

fn read_os_release() -> Option<String> {
    let content = std::fs::read_to_string("/etc/os-release").ok()?;
    content
        .lines()
        .find_map(|l| l.strip_prefix("PRETTY_NAME="))
        .map(|s| s.trim_matches('"').to_string())
}

fn uname_release() -> Option<String> {
    Command::new("uname")
        .arg("-r")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
}

fn uname_machine() -> Option<String> {
    Command::new("uname")
        .arg("-m")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
}

fn hostname() -> Option<String> {
    Command::new("hostname")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
}

fn program_version(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    Some(stdout.lines().next()?.trim().to_string())
}

fn print_flake_inputs_summary(flake_dir: &Path) {
    let inputs = match crate::nix::flake::read_flake_inputs(flake_dir) {
        Ok(i) => i,
        Err(_) => return,
    };
    if inputs.is_empty() {
        return;
    }
    println!("- **Flake inputs** ({}):", inputs.len());
    for i in &inputs {
        let repo = match (&i.repo_type, &i.repo_owner, &i.repo_name) {
            (Some(t), Some(o), Some(r)) => format!("{}:{}/{}", t, o, r),
            _ => "?".to_string(),
        };
        println!("  - `{}` → `{}`", i.name, repo);
    }
}
