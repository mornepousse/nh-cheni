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

    print_header();
    print_environment_section();
    print_config_section();
    print_doctor_section();
    print_cache_section();
    print_what_happened_section();
    Ok(())
}

fn print_header() {
    println!("# cheni bug report");
    println!();
    println!("<!-- Paste this into https://gitlab.com/harrael/cheni/-/issues/new --> ");
    println!("<!-- Then add a description of what you were trying to do below. -->");
    println!();
}

/// Versions, kernel, arch, plus any cheni-relevant env overrides — the
/// minimal "where am I running" snapshot a maintainer needs first.
fn print_environment_section() {
    println!("## Environment");
    println!();
    println!("- **cheni**: `{}`", env!("GIT_DESCRIBE"));
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
    for tool in &["nh", "nvd", "nix"] {
        if let Some(v) = program_version(tool, &["--version"]) {
            println!("- **{}**: `{}`", tool, v);
        }
    }
    let env_vars = ["CHENI_CONFIG", "CHENI_HTTP_TIMEOUT", "NO_COLOR"];
    let set_vars: Vec<String> = env_vars
        .iter()
        .filter_map(|v| std::env::var(v).ok().map(|val| format!("`{}={}`", v, val)))
        .collect();
    if !set_vars.is_empty() {
        println!("- **Env overrides**: {}", set_vars.join(", "));
    }
    println!();
}

/// Flake dir + hostname + init state + active pins + flake inputs.
/// Falls back to a single italics line if config detection fails so
/// the reader knows we tried.
fn print_config_section() {
    println!("## Config overview");
    println!();
    let cfg = match crate::nix::config::detect() {
        Ok(c) => c,
        Err(e) => {
            println!("_Could not detect NixOS config: {}_", e);
            println!();
            return;
        }
    };
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
    println!();
}

/// Re-runs `cheni doctor` inside a code fence. doctor() prints to
/// stdout itself, so we just bracket it with the fence markers — no
/// capture buffer needed.
fn print_doctor_section() {
    println!("## Doctor");
    println!();
    println!("```");
    if let Err(e) = super::doctor::run(false) {
        println!("_cheni doctor failed: {}_", e);
    }
    println!("```");
    println!();
}

fn print_cache_section() {
    let path = crate::nix::version_cache::cache_path();
    println!("## Version cache");
    println!();
    if !path.exists() {
        println!("- _No cache file_");
        println!();
        return;
    }
    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    match crate::nix::version_cache::VersionCache::load(&path) {
        Ok(cache) => {
            println!("- **Path**: `{}`", path.display());
            println!("- **Entries**: {}", cache.entry_count());
            println!("- **Size**: {} B ({:.2} KiB)", bytes, bytes as f64 / 1024.0);
        }
        Err(e) => {
            println!("- _Load failed: {}_", e);
        }
    }
    println!();
}

fn print_what_happened_section() {
    println!("## What happened?");
    println!();
    println!("<!-- Describe: -->");
    println!("<!-- 1. What command you ran -->");
    println!("<!-- 2. What you expected -->");
    println!("<!-- 3. What actually happened (paste the output if possible) -->");
    println!();
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
