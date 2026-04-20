//! `cheni upgrade` command.
//!
//! Full system upgrade: update all flake inputs, rebuild, clean
//! obsolete pins, and optionally garbage-collect old generations.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

use anyhow::{Context, Result};
use colored::Colorize;
use tracing::debug;

use crate::nix::{config, pins};

/// One input update parsed out of `nix flake update`'s chatty stderr.
#[derive(Debug, Clone, PartialEq, Eq)]
struct InputUpdate {
    name: String,
    old_date: String,
    new_date: String,
}

/// Options for `cheni upgrade`.
pub struct UpgradeOptions {
    /// Run garbage collection after the rebuild (default: off).
    /// This DELETES old generations — you won't be able to rollback!
    pub gc: bool,
    /// Skip cleanup of obsolete pins.
    pub no_clean_pins: bool,
    /// Skip the preview + confirmation step.
    pub yes: bool,
}

/// Run `cheni upgrade`.
///
/// Full system upgrade, broken into numbered steps:
/// 1. Update flake inputs + refresh major-constrained freezes
/// 2. Preview changes
/// 3. Rebuild the system
/// 4. Clean obsolete pins
/// 5. (optional, with --gc) Garbage-collect old generations
pub fn run(opts: UpgradeOptions) -> Result<()> {
    let started = Instant::now();
    let nix_config = config::detect()?;
    let config_path = nix_config.flake_dir.to_str()
        .context("Config path is not valid UTF-8")?;
    let total_steps = if opts.gc { 5 } else { 4 };

    println!("{}\n", "=== cheni upgrade ===".bold());

    print_step(1, total_steps, "Updating flake inputs");
    let context = update_flake_inputs(&nix_config.flake_dir)?;
    refresh_constrained_freezes_step(&nix_config.flake_dir);
    print_separator();

    print_step(2, total_steps, "Previewing changes");
    let stats = match preview_and_confirm(config_path, &nix_config.hostname, opts.yes, &context)? {
        Some(s) => s,
        None => return Ok(()),
    };
    print_separator();

    print_step(3, total_steps, "Rebuilding system");
    rebuild_system(config_path)?;
    print_separator();

    print_step(4, total_steps, "Checking obsolete pins");
    run_pin_cleanup_step(&nix_config.flake_dir, opts.no_clean_pins)?;

    if opts.gc {
        print_separator();
        print_step(5, total_steps, "Collecting garbage (> 30 days)");
        run_gc_step(opts.yes)?;
    }

    print_separator();
    print_final_summary(started.elapsed(), &stats, &context);
    if !opts.gc {
        println!(
            "{}",
            "  Old generations kept for rollback. Use --gc to reclaim disk space later.".dimmed()
        );
    }
    Ok(())
}

/// Render `[N/total] Title` in a consistent shape across the run.
fn print_step(n: usize, total: usize, title: &str) {
    println!("{} {}", format!("[{}/{}]", n, total).dimmed(), title.bold());
}

/// Horizontal rule between steps. Keeps the output skimmable — each
/// step becomes a visually distinct block rather than running into
/// its neighbours.
fn print_separator() {
    println!("{}", "───────────────────────────────────────────".dimmed());
}

/// Step 1: refresh every flake input. Bails if `nix flake update`
/// fails. Captures the stderr so we can print a clean summary of
/// which inputs bumped to which date, instead of leaking the raw
/// multiline URL/narHash chatter nix emits by default.
fn update_flake_inputs(flake_dir: &Path) -> Result<UpgradeContext> {
    let output = Command::new("nix")
        .args(["flake", "update"])
        .current_dir(flake_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| crate::nix::tools::tool_error("nix", e))?;

    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        anyhow::bail!("nix flake update failed");
    }

    // nix flake update prints its narrative on stderr.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let updates = parse_flake_update_events(&stderr);
    print_flake_update_summary(&updates);
    Ok(UpgradeContext {
        inputs_updated: updates.len(),
        git_tree_dirty: detect_dirty_tree_warning(&stderr),
    })
}

/// Nix prints `warning: Git tree '<path>' is dirty` (or `warning: dirty
/// Git tree '<path>'` on older nix) when the flake repo has
/// uncommitted changes. Detecting it lets the final summary explain
/// why a "no-op" upgrade still rebuilt artefacts.
fn detect_dirty_tree_warning(stderr: &str) -> bool {
    stderr
        .lines()
        .any(|l| l.contains("Git tree") && l.contains("is dirty")
             || l.contains("dirty Git tree"))
}

/// Parse the `• Updated input 'X':` blocks out of `nix flake update`'s
/// stderr. Returns one `InputUpdate` per input that actually bumped.
///
/// The stanza is:
/// ```text
/// • Updated input 'NAME':
///     'url?…' (YYYY-MM-DD)
///   → 'url?…' (YYYY-MM-DD)
/// ```
fn parse_flake_update_events(stderr: &str) -> Vec<InputUpdate> {
    let mut out = Vec::new();
    let lines: Vec<&str> = stderr.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        if let Some(name) = extract_updated_input_name(line) {
            // Next two lines carry the old / new locator.
            let old_date = lines.get(i + 1).and_then(|l| extract_parenthesised_date(l));
            let new_date = lines.get(i + 2).and_then(|l| extract_parenthesised_date(l));
            if let (Some(old_date), Some(new_date)) = (old_date, new_date) {
                out.push(InputUpdate {
                    name,
                    old_date,
                    new_date,
                });
                i += 3;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// `• Updated input 'cheni':` → `Some("cheni")`.
fn extract_updated_input_name(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let rest = trimmed.strip_prefix("• Updated input '")?;
    let end = rest.find('\'')?;
    Some(rest[..end].to_string())
}

/// Extract `YYYY-MM-DD` from a locator line like
/// `    'github:...?narHash=...' (2026-04-20)`.
fn extract_parenthesised_date(line: &str) -> Option<String> {
    let open = line.rfind('(')?;
    let close = line[open + 1..].find(')')?;
    let body = &line[open + 1..open + 1 + close];
    // Shape check: YYYY-MM-DD.
    if body.len() == 10
        && body.as_bytes()[4] == b'-'
        && body.as_bytes()[7] == b'-'
        && body.chars().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }
        })
    {
        Some(body.to_string())
    } else {
        None
    }
}

/// Render the flake-update outcome as a compact table. Silent when
/// nothing bumped (the separator + "already up to date" header is
/// enough).
fn print_flake_update_summary(updates: &[InputUpdate]) {
    if updates.is_empty() {
        println!("  {}", "Everything already up to date.".dimmed());
        return;
    }
    println!(
        "  {} {} input(s) updated:",
        "✓".green(),
        updates.len().to_string().bold()
    );
    for u in updates {
        println!(
            "    {:<20} {} → {}",
            u.name.bold(),
            u.old_date.dimmed(),
            u.new_date
        );
    }
}

/// Step 1b: refresh any freezes that carry a `--major N` constraint.
///
/// Walks `package-freezes.json`, queries the new nixpkgs rev for each
/// constrained package, and either bumps the freeze (same major, new
/// patch/minor available) or holds it (upstream moved past the major).
/// Non-fatal: a prefetch / eval failure just reports "Unknown" for
/// the entry and leaves the upgrade moving forward.
fn refresh_constrained_freezes_step(flake_dir: &Path) {
    match super::freeze::refresh_constrained_freezes(flake_dir) {
        Ok(outcomes) if !outcomes.is_empty() => {
            super::freeze::print_refresh_summary(&outcomes);
        }
        Ok(_) => {}
        Err(e) => {
            debug!("Freeze refresh skipped: {}", e);
        }
    }
}

/// Aggregated counts from the dry-run preview, reused by the final
/// summary. `None` means "no changes, upgrade short-circuited".
#[derive(Debug, Clone, Default)]
pub struct UpgradeStats {
    pub major: usize,
    pub minor: usize,
    pub patch: usize,
    pub new: usize,
    pub artefacts: usize,
}

impl UpgradeStats {
    fn total_packages(&self) -> usize {
        self.major + self.minor + self.patch + self.new
    }
}

/// Signals picked up during the run so the final summary can explain
/// *why* things were (or weren't) rebuilt — not just count them.
#[derive(Default)]
pub struct UpgradeContext {
    /// Number of flake inputs that moved in step 1. Zero means
    /// everything was already up to date.
    pub inputs_updated: usize,
    /// `warning: Git tree '…' is dirty` was seen — the flake's own
    /// git checkout has uncommitted changes, which triggers a
    /// re-evaluation even when no input moved.
    pub git_tree_dirty: bool,
}

/// Step 2: evaluate pending changes via `nix build --dry-run`, show a
/// human summary, then ask for confirmation.
///
/// Returns `Ok(Some(stats))` when the caller should proceed with the
/// rebuild, `Ok(None)` when the user cancelled or there's nothing
/// to do. `yes` skips the prompt for non-interactive use.
fn preview_and_confirm(
    config_path: &str,
    hostname: &str,
    yes: bool,
    context: &UpgradeContext,
) -> Result<Option<UpgradeStats>> {
    let flake_ref = format!(
        "{}#nixosConfigurations.{}.config.system.build.toplevel",
        config_path, hostname
    );
    let preview_output = Command::new("nix")
        .args(["build", &flake_ref, "--dry-run", "--no-link", "--print-build-logs"])
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .output()
        .map_err(|e| crate::nix::tools::tool_error("nix", e))?;

    if !preview_output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&preview_output.stderr));
        anyhow::bail!("Preview evaluation failed. Run 'cheni build' to see details.");
    }

    // nix --dry-run prints its summary on stderr.
    let stderr = String::from_utf8_lossy(&preview_output.stderr);
    let (to_build, to_fetch) = parse_dry_run_summary(&stderr);

    if to_build.is_empty() && to_fetch.is_empty() {
        println!("  {}", "Nothing to build or download — already up to date.".green());
        return Ok(None);
    }

    let stats = print_preview_lists(&to_build, &to_fetch);

    // If we already know this rebuild is going to be pure noise, warn
    // BEFORE the confirmation prompt so the user can skip the wait.
    if let Some(warning) = preview_noop_warning(&stats, context) {
        println!();
        println!("  {} {}", "⚠".yellow().bold(), warning.yellow());
    }

    if yes {
        return Ok(Some(stats));
    }
    println!();
    if !confirm("Download and apply these changes?")? {
        println!("\n{}", "  Upgrade cancelled. Flake is already updated.".yellow());
        println!("  Use '{}' to rebuild later.", "cheni upgrade --yes".bold());
        return Ok(None);
    }
    Ok(Some(stats))
}

fn print_section_from_changes(
    glyph: &str,
    label: &str,
    changes: &[PackageChange],
    display_limit: usize,
    glyph_color: Color,
) {
    // Split into real packages + system artefacts. Packages get the
    // full one-line treatment; artefacts get collapsed.
    let (packages, artefacts): (Vec<_>, Vec<_>) =
        changes.iter().partition(|c| !is_system_artefact(c));

    let glyph_colored = match glyph_color {
        Color::Cyan => glyph.cyan().to_string(),
        Color::Yellow => glyph.yellow().to_string(),
    };

    // Case 1: artefacts only. Nothing actionable — collapse the whole
    // section to one line so it doesn't dominate the preview.
    if packages.is_empty() && !artefacts.is_empty() {
        println!(
            "  {} {} system / home-manager artefact(s) {} ({})",
            glyph_colored,
            artefacts.len().to_string().bold(),
            label,
            artefact_sample(&artefacts).dimmed()
        );
        return;
    }

    // Case 2: packages (with or without a tail of artefacts).
    let header = aggregate_header(&packages);
    let head = format!("  {} {} package(s) {}", glyph_colored, packages.len(), label);
    if header.is_empty() {
        println!("{}:", head);
    } else {
        println!("{} ({}):", head, header.dimmed());
    }
    for change in packages.iter().take(display_limit) {
        println!("    {}", format_change(change));
    }
    if packages.len() > display_limit {
        println!(
            "    {} and {} more package(s)...",
            "...".dimmed(),
            packages.len() - display_limit
        );
    }
    if !artefacts.is_empty() {
        println!(
            "    {} +{} system artefact(s) ({})",
            "…".dimmed(),
            artefacts.len(),
            artefact_sample(&artefacts).dimmed()
        );
    }
}

/// Pick up to 3 names from the artefact bucket for a curiosity
/// preview. Entries with an empty `name` (unparseable) fall back to
/// the raw `new` string.
fn artefact_sample(artefacts: &[&PackageChange]) -> String {
    let names: Vec<&str> = artefacts
        .iter()
        .take(3)
        .map(|c| if c.name.is_empty() { c.new.as_str() } else { c.name.as_str() })
        .collect();
    if artefacts.len() > 3 {
        format!("{}, …", names.join(", "))
    } else {
        names.join(", ")
    }
}

/// Count real packages by diff bucket (major/minor/patch/new) and
/// render as "N major, M minor, K new", omitting zero slots. Artefacts
/// are counted separately and don't carry a meaningful `major/minor/
/// patch` classification, so they're excluded from this header.
fn aggregate_header(packages: &[&PackageChange]) -> String {
    use crate::version::compare::VersionDiff;
    let mut major = 0;
    let mut minor = 0;
    let mut patch = 0;
    let mut new = 0;
    for c in packages {
        if c.old.is_none() {
            new += 1;
            continue;
        }
        match c.diff {
            VersionDiff::Major => major += 1,
            VersionDiff::Minor => minor += 1,
            VersionDiff::Equal | VersionDiff::Newer => patch += 1,
        }
    }
    let mut parts: Vec<String> = Vec::new();
    if major > 0 {
        parts.push(format!("{} major", major));
    }
    if minor > 0 {
        parts.push(format!("{} minor", minor));
    }
    if patch > 0 {
        parts.push(format!("{} patch", patch));
    }
    if new > 0 {
        parts.push(format!("{} new", new));
    }
    parts.join(", ")
}

/// How the "old → new" version delta should be styled.
enum Color {
    Cyan,
    Yellow,
}

/// One changed package, ready for display. `old` is `None` when the
/// package isn't currently installed (= new install rather than update).
struct PackageChange {
    name: String,
    old: Option<String>,
    new: String,
    diff: crate::version::compare::VersionDiff,
}

/// Top-level renderer of the "to fetch" + "to build" blocks. Builds
/// `PackageChange` lists once (so the downstream `UpgradeStats`
/// aggregates match what the user saw) and emits them through
/// `print_section_from_changes`.
///
/// Each section is split between real packages (the ones the user
/// actually thinks of as software) and "system artefacts" (home-manager
/// internal files, nixos-system closures, completion caches…). The
/// artefacts get collapsed into a single-line tally so the preview
/// stays readable even when home-manager rebuilds a dozen generated
/// files on every upgrade.
fn print_preview_lists(to_build: &[String], to_fetch: &[String]) -> UpgradeStats {
    let installed = crate::nix::store::read_installed_packages().unwrap_or_default();
    let fetch_changes = build_changes(to_fetch, &installed);
    let build_changes_vec = build_changes(to_build, &installed);

    if !fetch_changes.is_empty() {
        print_section_from_changes("↓", "to download", &fetch_changes, 20, Color::Cyan);
    }
    if !build_changes_vec.is_empty() {
        println!();
        print_section_from_changes("⚒", "to build locally", &build_changes_vec, 10, Color::Yellow);
    }

    aggregate_stats(&fetch_changes, &build_changes_vec)
}

/// Return `true` for entries that aren't user-facing packages:
/// home-manager generated files, nixos-system closures, shell
/// completion caches, etc. Classifying them out of the main list
/// keeps the preview focused on things the user can make a
/// decision about.
fn is_system_artefact(c: &PackageChange) -> bool {
    // `build_changes` sets `name = ""` when `split_name_version`
    // couldn't pull a version — almost always means it's a
    // generated file rather than a package with a real semver.
    if c.name.is_empty() {
        // The raw entry is in `c.new` in that case. Still accept it
        // as a real package if it has version-looking digits
        // (salvage for obscure packages like `firefox-149`), otherwise
        // it's an artefact.
        let has_trailing_digit = c.new.chars().last().is_some_and(|ch| ch.is_ascii_digit());
        return !has_trailing_digit;
    }
    is_system_artefact_name(&c.name)
}

/// Pure half of `is_system_artefact`: name-based classification.
/// Kept as a free function for testing. The list grows as we
/// encounter new artefact shapes in real rebuild logs.
fn is_system_artefact_name(name: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "hm_",
        "home-manager-",
        "home-configuration-",
        "nixos-system-",
        "system-path",
        "closure-info",
        "initrd-linux-",
        "linux-",  // linux-<ver>-modules / -shrunk / …
        "user-environment",
    ];
    const EXACTS: &[&str] = &[
        "options.json",
        "man-cache",
        "man-paths",
        "etc",
        "boot.json",
        "firmware",
    ];
    const SUFFIXES: &[&str] = &[
        "-fish-completions",
        "-bash-completions",
        "-zsh-completions",
        "-completions",
        ".manpath",
        ".dirs",
        "-manpage",
    ];
    if EXACTS.contains(&name) {
        return true;
    }
    if PREFIXES.iter().any(|p| name.starts_with(p)) {
        return true;
    }
    if SUFFIXES.iter().any(|s| name.ends_with(s)) {
        return true;
    }
    false
}

/// Collapse fetch+build changes into `UpgradeStats` for the final
/// summary line. Each package is counted once with its version-diff
/// classification.
fn aggregate_stats(
    fetch: &[PackageChange],
    build: &[PackageChange],
) -> UpgradeStats {
    use crate::version::compare::VersionDiff;
    let mut stats = UpgradeStats::default();
    for c in fetch.iter().chain(build.iter()) {
        if is_system_artefact(c) {
            stats.artefacts += 1;
            continue;
        }
        if c.old.is_none() {
            stats.new += 1;
            continue;
        }
        match c.diff {
            VersionDiff::Major => stats.major += 1,
            VersionDiff::Minor => stats.minor += 1,
            VersionDiff::Equal | VersionDiff::Newer => stats.patch += 1,
        }
    }
    stats
}

/// Render the final "✓ Upgrade complete in X — Y packages changed"
/// line with the counts captured at preview time.
fn print_final_summary(
    elapsed: std::time::Duration,
    stats: &UpgradeStats,
    context: &UpgradeContext,
) {
    let headline = render_summary_headline(stats, context);
    println!(
        "{} {} in {} — {}.",
        "✓".green().bold(),
        "Upgrade complete".bold(),
        format_elapsed(elapsed).dimmed(),
        headline
    );
    if let Some(reason) = explain_no_op_rebuild(stats, context) {
        println!("  {} {}", "ⓘ".cyan(), reason);
    }
}

/// Build the human-readable tail of the "✓ Upgrade complete …"
/// sentence. Pure so it's trivially testable.
fn render_summary_headline(stats: &UpgradeStats, context: &UpgradeContext) -> String {
    let packages = stats.total_packages();
    let mut parts: Vec<String> = Vec::new();
    if stats.major > 0 {
        parts.push(format!("{} major", stats.major).yellow().bold().to_string());
    }
    if stats.minor > 0 {
        parts.push(format!("{} minor", stats.minor));
    }
    if stats.patch > 0 {
        parts.push(format!("{} patch", stats.patch).dimmed().to_string());
    }
    if stats.new > 0 {
        parts.push(format!("{} new", stats.new).green().to_string());
    }
    let breakdown = if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join(", "))
    };

    match (packages, stats.artefacts) {
        (0, 0) => "nothing changed".to_string(),
        // Artefacts-only with a known cause collapses to "nothing
        // changed" — the artefacts are just re-evaluation fallout
        // that the follow-up line will explain.
        (0, _) if context.explains_artefacts_only() => "nothing changed".to_string(),
        (0, a) => format!(
            "no user-facing package changes ({} system artefact{} rebuilt)",
            a,
            if a == 1 { "" } else { "s" },
        ),
        (p, 0) => format!(
            "{} package{} changed{}",
            p,
            if p == 1 { "" } else { "s" },
            breakdown,
        ),
        (p, a) => format!(
            "{} package{} changed{}, {} system artefact{} rebuilt",
            p,
            if p == 1 { "" } else { "s" },
            breakdown,
            a,
            if a == 1 { "" } else { "s" },
        ),
    }
}

/// When the rebuild did *nothing* user-facing but still produced
/// derivations, explain why — the user just spent 40 seconds and
/// deserves to know whether it was pointless. Returns `None` if
/// there's nothing useful to say.
fn explain_no_op_rebuild(stats: &UpgradeStats, context: &UpgradeContext) -> Option<String> {
    // Only fire the hint when there were no real package changes and
    // at least some artefacts were rebuilt — otherwise the headline
    // is already self-explanatory.
    if stats.total_packages() > 0 || stats.artefacts == 0 {
        return None;
    }
    match (context.inputs_updated, context.git_tree_dirty) {
        (0, true) => Some(format!(
            "Flake inputs unchanged but your config git tree is dirty — {} system artefact{} \
             re-evaluated to match the uncommitted state.",
            stats.artefacts,
            if stats.artefacts == 1 { " was" } else { "s were" },
        )),
        (0, false) => Some(format!(
            "Flake inputs unchanged; {} system artefact{} re-evaluated (home-manager internals).",
            stats.artefacts,
            if stats.artefacts == 1 { "" } else { "s" },
        )),
        _ => None, // inputs changed — the artefacts have an obvious cause
    }
}

/// Pre-confirmation warning: rebuild is predicted to be pure noise.
/// Returns `None` when the rebuild has a genuine cause (real package
/// changes, or flake inputs that moved).
fn preview_noop_warning(stats: &UpgradeStats, context: &UpgradeContext) -> Option<String> {
    if stats.total_packages() > 0 || stats.artefacts == 0 {
        return None;
    }
    if context.inputs_updated > 0 {
        return None;
    }
    if context.git_tree_dirty {
        Some(format!(
            "No package will change. {} system artefact{} are being rebuilt because your \
             nixos-config git tree is dirty — commit or stash your changes to skip this.",
            stats.artefacts,
            if stats.artefacts == 1 { "" } else { "s" },
        ))
    } else {
        Some(format!(
            "No package will change. {} system artefact{} are home-manager internals \
             re-evaluating — safe to skip.",
            stats.artefacts,
            if stats.artefacts == 1 { "" } else { "s" },
        ))
    }
}

impl UpgradeContext {
    /// Whether this context explains why an artefacts-only rebuild
    /// happened — used to collapse the headline to "nothing changed".
    fn explains_artefacts_only(&self) -> bool {
        self.inputs_updated == 0
    }
}

/// Format `Duration` as `MmSs` or `Ss` — just the live-log feel, not
/// sub-second precision.
fn format_elapsed(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    }
}

/// Match each dry-run entry against the currently-installed set,
/// computing the `{name, old, new, diff}` tuple used by the renderer.
/// Entries whose store name can't be split into `name-version` are
/// shown with an empty `name` and the raw entry as `new` — better
/// than dropping them silently.
fn build_changes(
    entries: &[String],
    installed: &[crate::nix::store::StorePackage],
) -> Vec<PackageChange> {
    use crate::nix::store::split_name_version;
    use crate::version::{compare::compare_versions, parse::parse_version};

    entries
        .iter()
        .map(|entry| {
            let (name, new_ver) = split_name_version(entry)
                .unwrap_or_else(|| (String::new(), entry.clone()));
            let old = installed
                .iter()
                .find(|p| p.name == name)
                .map(|p| p.version.clone());
            let diff = match old.as_deref() {
                Some(old_ver) => compare_versions(&parse_version(old_ver), &parse_version(&new_ver)),
                None => crate::version::compare::VersionDiff::Equal,
            };
            PackageChange {
                name,
                old,
                new: new_ver,
                diff,
            }
        })
        .collect()
}

/// Render a single change as a one-liner. `major` bumps get a yellow
/// arrow so they stand out at a glance; `patch` is dimmed.
fn format_change(c: &PackageChange) -> String {
    use crate::version::compare::VersionDiff;
    let name = if c.name.is_empty() {
        c.new.clone()
    } else {
        c.name.clone()
    };
    let tag = match (&c.old, &c.diff) {
        (None, _) => "new".green().to_string(),
        (Some(_), VersionDiff::Major) => "major".yellow().bold().to_string(),
        (Some(_), VersionDiff::Minor) => "minor".to_string(),
        (Some(_), VersionDiff::Newer) => "downgrade".magenta().to_string(),
        (Some(_), _) => "patch".dimmed().to_string(),
    };
    match &c.old {
        Some(old) => format!(
            "{:<28} {} → {}  [{}]",
            name.bold(),
            old.dimmed(),
            c.new,
            tag
        ),
        None => format!("{:<28} {} {}  [{}]", name.bold(), "→".dimmed(), c.new, tag),
    }
}

/// Step 2: invoke `nh os switch` with the activation step inline.
///
/// Uses the merged-pipe streamer so `/nix/store/<hash>-...` noise is
/// stripped from the output live. On failure, the raw (non-prettified)
/// buffer is fed to the diagnose pattern library so the user gets an
/// actionable hint along with the raw error.
fn rebuild_system(config_path: &str) -> Result<()> {
    let out = crate::output::stream::run_streaming(
        "nh",
        &["os", "switch", config_path],
        None,
    )?;
    if !out.status.success() {
        crate::cmd::diagnose::print_hints_for(&out.raw_buffer);
        anyhow::bail!("System rebuild failed. Fix the issue and run 'cheni build' again.");
    }
    Ok(())
}

/// Step 3: either clean obsolete pins or announce the skip — `no_clean`
/// decides which branch is taken so the step label stays aligned.
fn run_pin_cleanup_step(flake_dir: &Path, no_clean: bool) -> Result<()> {
    if no_clean {
        println!("  {}", "Skipping pin cleanup (--no-clean-pins)".dimmed());
        return Ok(());
    }
    clean_obsolete_pins(flake_dir)
}

/// Step 4: GC generations older than 30 days (only when --gc is set —
/// the rollback guarantee comes from keeping this off by default).
///
/// Previews via `--dry-run` first so the user sees the scope of the
/// deletion (and how many store paths it'll reclaim) before sudo kicks
/// in for the real run. `yes` bypasses the confirmation.
fn run_gc_step(yes: bool) -> Result<()> {
    println!(
        "  {} This will delete old generations — rollback won't work past this point!",
        "!".yellow()
    );

    let preview = crate::nix::gc::preview(&["--delete-older-than", "30d"])?;
    if preview.paths == 0 {
        println!("  {}", "Nothing older than 30 days to collect.".dimmed());
        return Ok(());
    }
    println!(
        "  {} store path(s) would be removed.",
        preview.paths.to_string().bold()
    );

    if !yes && !confirm("Proceed with garbage collection?")? {
        println!("{}", "  Cancelled — old generations kept.".yellow());
        return Ok(());
    }

    let status = Command::new("sudo")
        .args(["nix-collect-garbage", "--delete-older-than", "30d"])
        .status()
        .map_err(|e| crate::nix::tools::tool_error("sudo", e))
        .context("running nix-collect-garbage")?;
    if !status.success() {
        println!("{}", "  (garbage collection skipped or failed)".dimmed());
    }
    Ok(())
}

/// Parse the summary output of `nix build --dry-run`.
///
/// Returns (to_build, to_fetch) — lists of package names.
/// Example output:
///   these 3 derivations will be built:
///     /nix/store/abc-foo-1.0.drv
///     /nix/store/def-bar-2.0.drv
///   these 5 paths will be fetched (12.3 MiB download, ...):
///     /nix/store/xyz-baz-3.0
fn parse_dry_run_summary(stderr: &str) -> (Vec<String>, Vec<String>) {
    let mut to_build = Vec::new();
    let mut to_fetch = Vec::new();

    enum Section {
        None,
        Build,
        Fetch,
    }
    let mut section = Section::None;

    for line in stderr.lines() {
        let trimmed = line.trim();

        // Detect section headers
        if trimmed.contains("derivations will be built") || trimmed.contains("derivation will be built") {
            section = Section::Build;
            continue;
        }
        if trimmed.contains("paths will be fetched") || trimmed.contains("path will be fetched") {
            section = Section::Fetch;
            continue;
        }

        // Parse store paths: /nix/store/<hash>-<name>
        if trimmed.starts_with("/nix/store/") {
            if let Some(name) = extract_store_name(trimmed) {
                match section {
                    Section::Build => to_build.push(name),
                    Section::Fetch => to_fetch.push(name),
                    Section::None => {}
                }
            }
        } else if !trimmed.is_empty() && !trimmed.starts_with("/nix/store/") {
            // Section ended
            section = Section::None;
        }
    }

    (to_build, to_fetch)
}

/// Extract package name + version from a store path.
/// e.g. "/nix/store/abc123-vivaldi-7.9.drv" -> "vivaldi-7.9"
fn extract_store_name(path: &str) -> Option<String> {
    let after_prefix = path.strip_prefix("/nix/store/")?;
    // Skip 32-char hash + hyphen
    if after_prefix.len() < 34 {
        return None;
    }
    let name = &after_prefix[33..];
    // Strip trailing .drv
    Some(name.trim_end_matches(".drv").to_string())
}

/// Wrapper around `util::confirm` that keeps upgrade's default-yes
/// semantic at the call site (the original local helper did the same
/// thing; this version delegates to the shared prompt).
fn confirm(question: &str) -> Result<bool> {
    crate::util::confirm(question, true)
}

/// Remove pins that are now obsolete (nixpkgs caught up with nixpkgs-latest).
fn clean_obsolete_pins(flake_dir: &Path) -> Result<()> {
    let current_pins = pins::read(flake_dir)?;

    if current_pins.is_empty() {
        println!("  No pins to check.");
        return Ok(());
    }

    let lock_path = flake_dir.join("flake.lock");
    let obsolete = super::obsolete::count_obsolete_pins(&lock_path, &current_pins);

    if obsolete == 0 {
        println!("  All {} pin(s) still needed.", current_pins.len());
        return Ok(());
    }

    // If nixpkgs caught up, all pins are obsolete — clear them all
    let removed = pins::clear(flake_dir)?;
    println!(
        "  {} Removed {} obsolete pin(s).",
        "✓".green(),
        removed
    );
    debug!("Cleaned {} obsolete pins", removed);

    Ok(())
}

#[cfg(test)]
#[path = "tests/upgrade.rs"]
mod tests;
