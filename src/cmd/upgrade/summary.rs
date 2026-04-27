//! Final-summary rendering and shared upgrade-state structs.
//!
//! `UpgradeStats` carries the per-bucket counts the preview computes,
//! `UpgradeContext` carries the run-level signals (inputs that moved,
//! dirty git tree). Both flow through to the closing "✓ Upgrade
//! complete …" line so it can explain *why* a rebuild happened, not
//! just count what changed.

use colored::Colorize;

/// Aggregated counts from the dry-run preview, reused by the final
/// summary. `None` means "no changes, upgrade short-circuited".
#[derive(Debug, Clone, Default)]
pub(crate) struct UpgradeStats {
    pub(crate) major: usize,
    pub(crate) minor: usize,
    pub(crate) patch: usize,
    pub(crate) new: usize,
    pub(crate) artefacts: usize,
}

impl UpgradeStats {
    pub(crate) fn total_packages(&self) -> usize {
        self.major + self.minor + self.patch + self.new
    }
}

/// Signals picked up during the run so the final summary can explain
/// *why* things were (or weren't) rebuilt — not just count them.
#[derive(Default)]
pub(crate) struct UpgradeContext {
    /// Number of flake inputs that moved in step 1. Zero means
    /// everything was already up to date.
    pub(crate) inputs_updated: usize,
    /// `warning: Git tree '…' is dirty` was seen — the flake's own
    /// git checkout has uncommitted changes, which triggers a
    /// re-evaluation even when no input moved.
    pub(crate) git_tree_dirty: bool,
}

impl UpgradeContext {
    /// Whether this context explains why an artefacts-only rebuild
    /// happened — used to collapse the headline to "nothing changed".
    fn explains_artefacts_only(&self) -> bool {
        self.inputs_updated == 0
    }
}

/// Render the final "✓ Upgrade complete in X — Y packages changed"
/// line with the counts captured at preview time. In boot mode the
/// completion banner switches wording — the new generation is on
/// disk and registered with the bootloader, but it isn't live yet,
/// so the user needs the explicit "reboot to activate" prompt.
pub(crate) fn print_final_summary(
    elapsed: std::time::Duration,
    stats: &UpgradeStats,
    context: &UpgradeContext,
    boot: bool,
) {
    let headline = render_summary_headline(stats, context);
    let banner = if boot { "Upgrade staged for next boot" } else { "Upgrade complete" };
    println!(
        "{} {} in {} — {}.",
        "✓".green().bold(),
        banner.bold(),
        format_elapsed(elapsed).dimmed(),
        headline
    );
    if boot {
        println!(
            "  {} {}",
            "→".cyan(),
            "Run 'sudo reboot' to activate the new generation.".bold()
        );
    }
    if let Some(reason) = explain_no_op_rebuild(stats, context) {
        println!("  {} {}", "ⓘ".cyan(), reason);
    }
}

/// Build the human-readable tail of the "✓ Upgrade complete …"
/// sentence. Pure so it's trivially testable.
pub(super) fn render_summary_headline(stats: &UpgradeStats, context: &UpgradeContext) -> String {
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
pub(super) fn explain_no_op_rebuild(stats: &UpgradeStats, context: &UpgradeContext) -> Option<String> {
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
pub(super) fn preview_noop_warning(stats: &UpgradeStats, context: &UpgradeContext) -> Option<String> {
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

/// Local alias to the shared `crate::util::format_elapsed`.
fn format_elapsed(d: std::time::Duration) -> String {
    crate::util::format_elapsed(d)
}

#[cfg(test)]
#[path = "tests/summary.rs"]
mod tests;
