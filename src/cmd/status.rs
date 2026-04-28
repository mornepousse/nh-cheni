//! `cheni status` command.
//!
//! Shows the current state of the system at a glance: where the config
//! lives, which generation is active, the age of every flake input,
//! the active pins, and a "Suggestions" section listing the next safe
//! actions to take. The goal is for a user who has lost track of where
//! they are to read this output and know what to do.

use std::path::Path;

use anyhow::Result;
use colored::Colorize;

use crate::nix::{config, freezes, pins};

use super::obsolete::count_obsolete_pins;

/// Run `cheni status`. `brief` collapses the output to just the
/// suggestions section: drops the config block, the flake inputs
/// table, the pins block, and the freezes block. Useful for piping
/// into a shell prompt or status bar where only anomalies matter.
pub fn run(brief: bool) -> Result<()> {
    let nix_config = config::detect()?;
    let current_pins = pins::read(&nix_config.flake_dir)?;
    let current_freezes = freezes::read(&nix_config.flake_dir)?;
    let lock_path = nix_config.flake_dir.join("flake.lock");
    let obsolete_count = if current_pins.is_empty() {
        0
    } else {
        count_obsolete_pins(&lock_path, &current_pins)
    };
    let active = read_active_generation();
    let lock_newer_than_active = is_lock_newer_than_active(&lock_path, &active);

    if !brief {
        println!("{}\n", "=== cheni status ===".bold());
        print_config_section(&nix_config, &active);
        print_flake_inputs_section(&lock_path);
        print_pins_section(&current_pins, obsolete_count);
        print_freezes_section(&current_freezes);
    }
    print_suggestions(
        &nix_config,
        obsolete_count,
        lock_newer_than_active,
        &current_pins,
    );
    if !brief {
        println!();
    }
    Ok(())
}

/// Compare flake.lock's mtime against the active generation's symlink
/// mtime. True when the lock changed but no rebuild has happened yet —
/// the typical "you ran `cheni pin --flakes`, now run `cheni build`" case.
fn is_lock_newer_than_active(
    lock_path: &Path,
    active: &Option<(u32, String)>,
) -> bool {
    let lock_modified = lock_path.metadata().ok().and_then(|m| m.modified().ok());
    let active_modified = active.as_ref().and_then(|(n, _)| {
        std::fs::symlink_metadata(format!("/nix/var/nix/profiles/system-{}-link", n))
            .ok()
            .and_then(|m| m.modified().ok())
    });
    matches!((lock_modified, active_modified), (Some(l), Some(a)) if l > a)
}

fn print_config_section(nix_config: &config::NixConfig, active: &Option<(u32, String)>) {
    println!(
        "  {:<18} {}",
        "Config:".dimmed(),
        nix_config.flake_dir.display()
    );
    println!("  {:<18} {}", "Hostname:".dimmed(), nix_config.hostname);

    let categories = config::list_module_categories(&nix_config.flake_dir);
    if !categories.is_empty() {
        println!("  {:<18} {}", "Modules:".dimmed(), categories.join(", "));
    }
    if let Some((num, age)) = active {
        println!(
            "  {:<18} #{} ({})",
            "Active generation:".dimmed(),
            num.to_string().bold(),
            age
        );
    }
}

fn print_flake_inputs_section(lock_path: &Path) {
    println!();
    println!("  {}", "Flake inputs:".bold());
    for (name, age, days, rev) in read_all_root_inputs(lock_path) {
        let rev_str = if rev.is_empty() { String::new() } else { format!(" ({})", rev) };
        // nixpkgs is the foundation everything else is compared
        // against — call it out in yellow once it crosses the
        // 3-day "drifting" threshold so it doesn't blend into the
        // dimmed list. Other inputs stay dimmed because their age
        // matters less for daily flow.
        let highlight = (name == "nixpkgs" || name == "nixpkgs-latest") && days >= 3;
        let age_styled = if highlight { age.yellow() } else { age.dimmed() };
        println!("    {:<22} {}{}", name, age_styled, rev_str.dimmed());
    }
}

fn print_pins_section(current_pins: &[String], obsolete_count: usize) {
    println!();
    if current_pins.is_empty() {
        println!("  {:<18} {}", "Pins:".bold(), "no active pins".dimmed());
        return;
    }
    let header = if obsolete_count > 0 {
        format!("{} active ({} obsolete)", current_pins.len(), obsolete_count)
            .red()
            .to_string()
    } else {
        format!("{} active", current_pins.len()).yellow().to_string()
    };
    println!("  {:<18} {}", "Pins:".bold(), header);
    for name in current_pins {
        println!("    {} {}", "→".yellow(), name);
    }
}

/// Render the "Freezes" block. Silent-if-none — unlike the always-shown
/// Pins line, freeze is an occasional-use feature and listing an empty
/// "no freezes" row every time the user runs `cheni status` is just noise.
fn print_freezes_section(current_freezes: &freezes::Freezes) {
    if current_freezes.is_empty() {
        return;
    }
    println!();
    let header = format!("{} held", current_freezes.len()).cyan().to_string();
    println!("  {:<18} {}", "Freezes:".bold(), header);
    for (name, entry) in current_freezes {
        println!(
            "    {} {:<24} {} {}",
            "⏸".cyan(),
            name,
            entry.version.dimmed(),
            format!("(since {})", entry.frozen_at).dimmed()
        );
    }
}

/// Print the actionable next-step suggestions. Falls back to a green
/// "everything clean" line when nothing matches — important so the
/// user always sees a non-empty Suggestions block.
fn print_suggestions(
    nix_config: &config::NixConfig,
    obsolete_count: usize,
    lock_newer_than_active: bool,
    current_pins: &[String],
) {
    println!();
    println!("  {}", "Suggestions:".bold());
    let mut any = false;

    if !config::is_initialized(&nix_config.flake_dir) {
        println!(
            "    {} no nixpkgs-latest input detected — run '{}' to set up",
            "⚠".yellow(),
            "cheni init".bold()
        );
        any = true;
    }
    if obsolete_count > 0 {
        println!(
            "    {} {} obsolete {} — run '{}' to remove",
            "⚠".yellow(),
            obsolete_count,
            crate::util::pluralize(obsolete_count, "pin"),
            "cheni clean".bold()
        );
        any = true;
    }
    if lock_newer_than_active {
        println!(
            "    {} flake.lock is newer than the active generation — run '{}' to apply",
            "⚠".yellow(),
            "cheni build".bold()
        );
        any = true;
    }
    if !current_pins.is_empty() && obsolete_count < current_pins.len() {
        println!(
            "    {} pinned packages waiting — run '{}' to refresh nixpkgs-latest + rebuild",
            "→".cyan(),
            "cheni upgrade --pins-only".bold()
        );
        any = true;
    }
    // Stale nixpkgs hint — same threshold as `cheni check` and
    // `cheni doctor`, kept in sync via `crate::util::format_days_ago`.
    // Read straight from the lock so the suggestion still fires when
    // nixpkgs is not in the in-report inputs section (filtered out
    // upstream as INFRASTRUCTURE).
    if let Some(input) = crate::nix::flake::read_input_by_name(
        &nix_config.flake_dir,
        "nixpkgs",
    ) {
        if input.days_old >= 3 {
            println!(
                "    {} nixpkgs floor is {} — run '{}' to advance",
                "→".cyan(),
                crate::util::format_days_ago(input.days_old).bold(),
                "cheni upgrade".bold()
            );
            any = true;
        }
    }
    // Dirty flake.lock — escalate to a Suggestion line when present
    // so a passive `cheni status` still surfaces the trap. Same
    // wording as the upgrade preflight + doctor check.
    if crate::nix::git::is_flake_lock_dirty(&nix_config.flake_dir) {
        println!(
            "    {} flake.lock has uncommitted bumps — `{}` to inspect, `{}` to discard",
            "⚠".yellow(),
            "git diff flake.lock".bold(),
            "git checkout flake.lock".bold()
        );
        any = true;
    }
    // Self-update hint, pulled from the cache that `cheni check`
    // refreshes asynchronously. Sync read — status never hits the
    // network on its own, so the suggestion only surfaces after a
    // recent `cheni check`. That's fine: status reflects state,
    // check discovers it.
    if let Some(latest) = self_update_hint(&nix_config.flake_dir) {
        println!(
            "    {} cheni {} available — run '{}' to update",
            "→".cyan(),
            latest.bold(),
            "cheni self-update".bold()
        );
        any = true;
    }
    if !any {
        println!(
            "    {} everything looks clean — run '{}' to scan for new updates",
            "✓".green(),
            "cheni check".bold()
        );
    }
}

/// Echo `cheni check`'s self-update hint when it's relevant: the user
/// pinned cheni at a release tag, the cache says a strictly newer tag
/// is available, and the comparison parses cleanly. Returns `None` in
/// every other case so the suggestions block stays quiet.
fn self_update_hint(flake_dir: &Path) -> Option<String> {
    let current_tag = super::self_update::read_cheni_tag(flake_dir).ok()?;
    if !crate::release::is_release_tag(&current_tag) {
        return None;
    }
    let latest = crate::release::cached_latest_release_tag()?;
    if latest == current_tag {
        return None;
    }
    let cur_v = crate::version::parse::parse_version(
        current_tag.strip_prefix('v').unwrap_or(&current_tag),
    );
    let lat_v =
        crate::version::parse::parse_version(latest.strip_prefix('v').unwrap_or(&latest));
    if lat_v <= cur_v {
        return None;
    }
    Some(latest)
}

/// Look up the currently active generation number + a human-readable age.
fn read_active_generation() -> Option<(u32, String)> {
    let target = std::fs::read_link("/nix/var/nix/profiles/system").ok()?;
    let name = target.file_name()?.to_str()?;
    let num: u32 = name
        .strip_prefix("system-")?
        .strip_suffix("-link")?
        .parse()
        .ok()?;

    let modified = std::fs::symlink_metadata(format!(
        "/nix/var/nix/profiles/system-{}-link",
        num
    ))
    .ok()?
    .modified()
    .ok()?;
    let age = format_age(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs()
            .saturating_sub(
                modified
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()?
                    .as_secs(),
            )
            / 86400,
    );
    Some((num, age))
}

/// List every root-level flake input with its age string, age in
/// days (for threshold checks), and short rev.
fn read_all_root_inputs(lock_path: &Path) -> Vec<(String, String, u64, String)> {
    let content = match std::fs::read_to_string(lock_path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let lock: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let root_inputs = lock
        .get("nodes")
        .and_then(|n| n.get("root"))
        .and_then(|r| r.get("inputs"))
        .and_then(|i| i.as_object());

    let names: Vec<String> = match root_inputs {
        Some(m) => m.keys().cloned().collect(),
        None => return Vec::new(),
    };

    let mut out = Vec::new();
    for name in names {
        let ts = get_input_timestamp(&lock, &name).unwrap_or(0);
        let rev = get_input_rev(&lock, &name).unwrap_or_default();
        let (age, days) = if ts == 0 {
            ("?".to_string(), 0)
        } else {
            let days = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .saturating_sub(ts)
                / 86400;
            (format_age(days), days)
        };
        out.push((name, age, days, rev));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Thin alias to `crate::util::format_days_ago` so the local call
/// sites stay short. The shared formatter keeps every "Xd ago"
/// surface in lockstep across `status` / `check` / `doctor` /
/// interactive banner.
fn format_age(days: u64) -> String {
    crate::util::format_days_ago(days)
}

/// Resolve a root input name to its actual node.
/// Handles indirection: root.inputs[name] may point to "nixpkgs_4".
fn resolve_node<'a>(lock: &'a serde_json::Value, name: &str) -> Option<&'a serde_json::Value> {
    let root_input = lock.get("nodes")?
        .get("root")?
        .get("inputs")?
        .get(name)?;

    let node_name = root_input.as_str().unwrap_or(name);
    lock.get("nodes")?.get(node_name)
}

/// Extract lastModified timestamp for a flake input (resolves via root).
fn get_input_timestamp(lock: &serde_json::Value, name: &str) -> Option<u64> {
    resolve_node(lock, name)?
        .get("locked")?
        .get("lastModified")?
        .as_u64()
}

/// Extract the short rev for a flake input (resolves via root).
fn get_input_rev(lock: &serde_json::Value, name: &str) -> Option<String> {
    let rev = resolve_node(lock, name)?
        .get("locked")?
        .get("rev")?
        .as_str()?;

    // Char-based truncation so a malformed / non-ASCII rev can't panic.
    // Git SHAs are hex so this is equivalent to byte-slicing in practice,
    // but the explicit form matches flake.rs::short_hash and stays safe
    // if a locked input ever lands with a non-standard 'rev' field.
    Some(rev.chars().take(12).collect())
}

/// Collect status's structured data, suitable for `cheni audit`.
#[allow(dead_code)]
pub(crate) fn collect_state(
    nix_config: &config::NixConfig,
) -> anyhow::Result<crate::cmd::audit::StateReport> {
    let pins = pins::read(&nix_config.flake_dir).unwrap_or_default();
    let freezes = freezes::read(&nix_config.flake_dir).unwrap_or_default();
    Ok(crate::cmd::audit::StateReport {
        pins_count: pins.len(),
        freezes_count: freezes.len(),
        flake_dir: nix_config.flake_dir.clone(),
    })
}
