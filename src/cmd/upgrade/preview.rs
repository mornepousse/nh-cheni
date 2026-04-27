//! Step 2 of `cheni upgrade`: dry-run evaluation, change classification,
//! preview rendering, and confirmation prompt.
//!
//! Also hosts `print_pending_changes`, the read-only entry point that
//! `cheni check` reuses to surface "what would change" without
//! triggering a rebuild.

use std::process::{Command, Stdio};

use anyhow::Result;
use colored::Colorize;

use super::summary::{
    preview_noop_warning, UpgradeContext, UpgradeStats,
};
use super::UpgradeOptions;

/// How the "old → new" version delta should be styled.
enum Color {
    Cyan,
    Yellow,
}

/// One changed package, ready for display. `old` is `None` when the
/// package isn't currently installed (= new install rather than update).
pub(super) struct PackageChange {
    pub(super) name: String,
    pub(super) old: Option<String>,
    pub(super) new: String,
    pub(super) diff: crate::version::compare::VersionDiff,
}

/// Step 2: evaluate pending changes via `nix build --dry-run`, show a
/// human summary, then ask for confirmation.
///
/// Returns `Ok(Some(stats))` when the caller should proceed with the
/// rebuild, `Ok(None)` when the user cancelled or there's nothing
/// to do. The function may also flip `opts.boot = true` when a
/// critical-component change is detected and the user opts into the
/// boot-mode rebuild — that's why `opts` is a mutable borrow rather
/// than a value or `yes: bool`.
pub(super) fn preview_and_confirm(
    config_path: &str,
    hostname: &str,
    opts: &mut UpgradeOptions,
    context: &UpgradeContext,
) -> Result<Option<UpgradeStats>> {
    let dry_run = run_dry_run(config_path, hostname)?;
    let (to_build, to_fetch) = parse_dry_run_summary(&dry_run);
    if to_build.is_empty() && to_fetch.is_empty() {
        println!("  {}", "Nothing to build or download — already up to date.".green());
        return Ok(None);
    }

    let installed = crate::nix::store::read_installed_packages().unwrap_or_default();
    let fetch_changes = build_changes(&to_fetch, &installed);
    let build_changes_vec = build_changes(&to_build, &installed);
    let critical = detect_critical_component_changes(&fetch_changes, &build_changes_vec);

    if !fetch_changes.is_empty() {
        print_section_from_changes("↓", "to download", &fetch_changes, 20, Color::Cyan);
    }
    if !build_changes_vec.is_empty() {
        println!();
        print_section_from_changes("⚒", "to build locally", &build_changes_vec, 10, Color::Yellow);
    }
    let stats = aggregate_stats(&fetch_changes, &build_changes_vec);

    // If a critical component is moving (dbus → broker, …), nh's
    // activation pre-check will refuse the live switch. Either flip
    // to boot mode now (saves the user the post-failure debug cycle)
    // or proceed and let it fail explicitly if they prefer.
    if !critical.is_empty() && !opts.boot {
        warn_critical_changes_and_offer_boot_mode(&critical, opts)?;
    }

    if let Some(warning) = preview_noop_warning(&stats, context) {
        println!();
        println!("  {} {}", "⚠".yellow().bold(), warning.yellow());
    }

    if opts.yes {
        return Ok(Some(stats));
    }
    println!();
    let prompt_text = if opts.boot {
        "Stage these changes for next boot?"
    } else {
        "Download and apply these changes?"
    };
    if !super::confirm(prompt_text)? {
        println!("\n{}", "  Upgrade cancelled. Flake is already updated.".yellow());
        println!("  Use '{}' to rebuild later.", "cheni upgrade --yes".bold());
        return Ok(None);
    }
    Ok(Some(stats))
}

/// Surface the critical component change and let the user opt into
/// boot mode for this run. When `opts.yes` is set the message is
/// purely advisory — non-interactive callers stay in switch mode and
/// will see the activation refusal at rebuild time, where the
/// `Pre-switch check` diagnose pattern points at the recovery path.
fn warn_critical_changes_and_offer_boot_mode(
    critical: &[String],
    opts: &mut UpgradeOptions,
) -> Result<()> {
    println!();
    println!("  {} {}", "⚠".yellow().bold(), "Critical component change detected:".yellow());
    for c in critical {
        println!("      · {}", c);
    }
    println!(
        "    {}",
        "nh's activation pre-check will refuse the live switch."
            .dimmed()
    );
    println!(
        "    {}",
        "Recommended: stage for next boot instead — `cheni upgrade --boot`."
            .dimmed()
    );

    if opts.yes {
        // --yes preserves the strict semantics chosen at invocation.
        // We've shown the warning; the user (or the script) made
        // their choice up front.
        return Ok(());
    }
    println!();
    if super::confirm("Switch to boot mode for this rebuild?")? {
        opts.boot = true;
        println!("  {}", "boot mode engaged — will use `nh os boot`.".dimmed());
    } else {
        println!(
            "  {}",
            "staying in switch mode — activation will likely fail; rerun with `--boot` to recover."
                .dimmed()
        );
    }
    Ok(())
}

/// Inspect the preview's package-change set for components whose
/// implementation swap triggers nixos-rebuild's switchInhibitor
/// pre-check.
///
/// Today we recognise the dbus → dbus-broker swap (the common case
/// — every NixOS desktop hits it once when nixpkgs flips the
/// default). The list grows as we encounter new triggers in real
/// rebuilds.
pub(super) fn detect_critical_component_changes(
    fetch: &[PackageChange],
    build: &[PackageChange],
) -> Vec<String> {
    let mut out = Vec::new();
    let landing = |name: &str| -> bool {
        fetch.iter().chain(build.iter()).any(|c| {
            let n = if c.name.is_empty() { c.new.as_str() } else { c.name.as_str() };
            n == name
        })
    };
    if landing("dbus-broker") {
        out.push("dbus-broker landing (dbus implementation swap)".to_string());
    }
    out
}

/// Run `nix build --dry-run` against the current flake state, parse
/// the output, and render the per-bucket breakdown.
///
/// Shared between `cheni upgrade` step 2 and the read-only `cheni
/// preview` command. Returns `Ok(None)` when nothing would change
/// (callers print their own "up to date" affordance and skip),
/// `Ok(Some(stats))` with the bucket counts otherwise. Bails on a
/// failed evaluation — we'd rather surface the error than render
/// silence.
pub(crate) fn print_pending_changes(
    config_path: &str,
    hostname: &str,
) -> Result<Option<UpgradeStats>> {
    let stderr = run_dry_run(config_path, hostname)?;
    let (to_build, to_fetch) = parse_dry_run_summary(&stderr);

    if to_build.is_empty() && to_fetch.is_empty() {
        println!("  {}", "Nothing to build or download — already up to date.".green());
        return Ok(None);
    }
    Ok(Some(print_preview_lists(&to_build, &to_fetch)))
}

/// Run `nix build --dry-run --no-link --print-build-logs` and return
/// the captured stderr (where nix prints its dry-run summary).
fn run_dry_run(config_path: &str, hostname: &str) -> Result<String> {
    let flake_ref = format!(
        "{}#nixosConfigurations.{}.config.system.build.toplevel",
        config_path, hostname
    );
    let out = Command::new("nix")
        .args(["build", &flake_ref, "--dry-run", "--no-link", "--print-build-logs"])
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .output()
        .map_err(|e| crate::nix::tools::tool_error("nix", e))?;
    if !out.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&out.stderr));
        anyhow::bail!("Preview evaluation failed. Run 'cheni build' to see details.");
    }
    Ok(String::from_utf8_lossy(&out.stderr).into_owned())
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
            "  {} {} system / home-manager {} {} ({})",
            glyph_colored,
            artefacts.len().to_string().bold(),
            crate::util::pluralize(artefacts.len(), "artefact"),
            label,
            artefact_sample(&artefacts).dimmed()
        );
        return;
    }

    // Case 2: packages (with or without a tail of artefacts).
    let header = aggregate_header(&packages);
    let head = format!(
        "  {} {} {} {}",
        glyph_colored,
        packages.len(),
        crate::util::pluralize(packages.len(), "package"),
        label
    );
    if header.is_empty() {
        println!("{}:", head);
    } else {
        println!("{} ({}):", head, header.dimmed());
    }
    for change in packages.iter().take(display_limit) {
        println!("    {}", format_change(change));
    }
    if packages.len() > display_limit {
        let remaining = packages.len() - display_limit;
        println!(
            "    {} and {} more {}...",
            "...".dimmed(),
            remaining,
            crate::util::pluralize(remaining, "package")
        );
    }
    if !artefacts.is_empty() {
        println!(
            "    {} +{} system {} ({})",
            "…".dimmed(),
            artefacts.len(),
            crate::util::pluralize(artefacts.len(), "artefact"),
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
pub(super) fn aggregate_header(packages: &[&PackageChange]) -> String {
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
pub(super) fn is_system_artefact(c: &PackageChange) -> bool {
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
    // Kernel-artefact discriminant — the kernel's modules / shrunk /
    // module-shrunk variants split into the same name as the kernel
    // itself ("linux-zen" / "linux-libre" / "linux-hardened" / …),
    // and only the version field carries the suffix that distinguishes
    // them. Without this check we'd either bucket the bare kernel as
    // artefact (when `linux-` was a blanket PREFIX) or surface every
    // kernel-modules churn as a "real package change".
    if has_kernel_artefact_version_suffix(&c.new) {
        return true;
    }
    is_system_artefact_name(&c.name)
}

/// True for version strings that carry a kernel build-artefact
/// suffix (`6.19.12-modules`, `6.19.12-shrunk`,
/// `6.19.12-modules-shrunk`, …). Lets the bare kernel
/// (`linux-zen-6.19.12` → name="linux-zen", new="6.19.12") show up
/// as a real package while its modules/shrunk siblings stay collapsed
/// in the artefacts tally. Pure for testability.
pub(super) fn has_kernel_artefact_version_suffix(version: &str) -> bool {
    const KERNEL_ARTEFACT_SUFFIXES: &[&str] = &[
        "-modules",
        "-modules-shrunk",
        "-shrunk",
    ];
    KERNEL_ARTEFACT_SUFFIXES.iter().any(|s| version.ends_with(s))
}

/// Pure half of `is_system_artefact`: name-based classification.
/// Kept as a free function for testing. The list grows as we
/// encounter new artefact shapes in real rebuild logs.
pub(super) fn is_system_artefact_name(name: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "hm_",
        "home-manager-",
        "home-configuration-",
        "nixos-system-",
        "system-path",
        "closure-info",
        "initrd-linux-",
        // NB: `linux-` is intentionally absent. It used to be here
        // as a blanket "modules / shrunk / …" catch, which also
        // swallowed the bare kernel itself (`linux-zen-6.19.12`),
        // `linux-firmware`, `linux-pam`, etc. The kernel-artefact
        // distinction now happens via `has_kernel_artefact_version_suffix`
        // on the version segment, where the `-modules`/`-shrunk`
        // markers actually live after `split_name_version`.
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
pub(super) fn aggregate_stats(
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

/// Match each dry-run entry against the currently-installed set,
/// computing the `{name, old, new, diff}` tuple used by the renderer.
/// Entries whose store name can't be split into `name-version` are
/// shown with an empty `name` and the raw entry as `new` — better
/// than dropping them silently.
pub(super) fn build_changes(
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

/// Parse the summary output of `nix build --dry-run`.
///
/// Returns (to_build, to_fetch) — lists of package names.
/// Example output:
///   these 3 derivations will be built:
///     /nix/store/abc-foo-1.0.drv
///     /nix/store/def-bar-2.0.drv
///   these 5 paths will be fetched (12.3 MiB download, ...):
///     /nix/store/xyz-baz-3.0
pub(super) fn parse_dry_run_summary(stderr: &str) -> (Vec<String>, Vec<String>) {
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
pub(super) fn extract_store_name(path: &str) -> Option<String> {
    let after_prefix = path.strip_prefix("/nix/store/")?;
    // Skip 32-char hash + hyphen
    if after_prefix.len() < 34 {
        return None;
    }
    let name = &after_prefix[33..];
    // Strip trailing .drv
    Some(name.trim_end_matches(".drv").to_string())
}

#[cfg(test)]
#[path = "tests/preview.rs"]
mod tests;
