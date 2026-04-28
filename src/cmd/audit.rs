//! `cheni audit` — single-shot health overview.
//!
//! Composes the structured reports from `doctor` (health), `check`
//! (updates), and `status` (state) into one ordered output that
//! prioritises action: TL;DR verdict → errors → updates → state →
//! next-action tip.
//!
//! See `docs/superpowers/specs/2026-04-28-cheni-audit-design.md`.

use anyhow::Result;
use colored::Colorize;
use serde::Serialize;

use crate::nix::config;

/// Severity of the overall audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AuditVerdict {
    /// No errors, no warnings, no actionable updates.
    Clear,
    /// Warnings only, or actionable updates available.
    Warnings,
    /// At least one blocking error in health.
    Errors,
}

/// One health issue (warning or error) surfaced by `doctor`.
#[derive(Debug, Clone, Serialize)]
pub struct HealthIssue {
    pub name: String,
    pub message: String,
    pub hint: Option<String>,
}

/// Health section of the audit, derived from `doctor`.
#[derive(Debug, Default, Serialize)]
pub struct HealthReport {
    pub errors: Vec<HealthIssue>,
    pub warnings: Vec<HealthIssue>,
    pub passed: usize,
}

/// One flake input that has an update available.
#[derive(Debug, Clone, Serialize)]
pub struct FlakeInputUpdate {
    pub name: String,
    pub current: Option<String>,
    pub latest_remote_date: Option<String>,
}

/// Updates section, derived from `check`.
#[derive(Debug, Default, Serialize)]
pub struct UpdatesReport {
    pub up_to_date: usize,
    pub minor: usize,
    pub major: usize,
    pub newer: usize,
    pub unknown: usize,
    pub frozen: usize,
    pub flake_inputs_with_update: Vec<FlakeInputUpdate>,
}

/// State section, derived from `status`.
#[derive(Debug, Default, Serialize)]
pub struct StateReport {
    pub pins_count: usize,
    pub freezes_count: usize,
    pub flake_dir: std::path::PathBuf,
}

/// The full audit report, ready to render.
#[derive(Debug, Serialize)]
pub struct AuditReport {
    pub health: HealthReport,
    pub updates: UpdatesReport,
    pub state: StateReport,
    pub verdict: AuditVerdict,
    pub next_action: Option<String>,
}

/// Dérive le verdict global à partir de la santé et des mises à jour.
pub(crate) fn compute_verdict(health: &HealthReport, updates: &UpdatesReport) -> AuditVerdict {
    if !health.errors.is_empty() {
        return AuditVerdict::Errors;
    }
    let actionable_updates = updates.minor + updates.major
        + updates.flake_inputs_with_update.len();
    if !health.warnings.is_empty() || actionable_updates > 0 {
        return AuditVerdict::Warnings;
    }
    AuditVerdict::Clear
}

/// Détermine l'action la plus prioritaire à suggérer à l'utilisateur.
/// Retourne None quand le verdict est Clear (aucune action nécessaire).
pub(crate) fn compute_next_action(report: &AuditReport) -> Option<String> {
    if let Some(err) = report.health.errors.first() {
        return Some(format!("Address `{}` first — it blocks rebuild.", err.name));
    }
    if report.updates.major > 0 {
        return Some(
            "Run `cheni check --details` to see major updates, then `cheni upgrade` to take them.".into(),
        );
    }
    if !report.updates.flake_inputs_with_update.is_empty() {
        return Some(
            "Run `cheni upgrade` to take the flake-input updates listed above.".into(),
        );
    }
    if let Some(warn) = report.health.warnings.first() {
        return Some(format!("Optional: address `{}` (warning).", warn.name));
    }
    None
}

/// Options controlling audit's output.
#[derive(Debug, Default)]
#[allow(dead_code)]
pub struct AuditOptions {
    pub brief: bool,
    pub json: bool,
}

/// Run `cheni audit`.
///
/// Appelle `collect_health`, `collect_updates`, `collect_state` (la version
/// updates est async à cause du batch eval sous-jacent).
/// Compose un `AuditReport` et le rend.
#[allow(dead_code)]
pub async fn run(opts: AuditOptions) -> Result<()> {
    let nix_config = config::detect()?;

    // Short-circuit pour les flakes entièrement non initialisées — health
    // surfacera l'erreur ; lancer les deux autres produirait surtout du bruit.
    if !config::is_initialized(&nix_config.flake_dir) {
        let health = crate::cmd::doctor::collect_health(&nix_config.flake_dir)?;
        let report = AuditReport {
            health,
            updates: UpdatesReport::default(),
            state: StateReport {
                pins_count: 0,
                freezes_count: 0,
                flake_dir: nix_config.flake_dir.clone(),
            },
            verdict: AuditVerdict::Errors,
            next_action: Some("Run `cheni init` to initialise the flake.".to_string()),
        };
        return render(&report, &opts);
    }

    // Trois collectes — updates est async, les autres sync. On n'a pas besoin
    // de parallélisme : le batch eval domine et tourne dans spawn_blocking.
    // Orchestration séquentielle pour la lisibilité.
    let health = crate::cmd::doctor::collect_health(&nix_config.flake_dir)?;
    let updates = crate::cmd::check::collect_updates(&nix_config).await?;
    let state = crate::cmd::status::collect_state(&nix_config)?;

    let verdict = compute_verdict(&health, &updates);
    let mut report = AuditReport {
        health,
        updates,
        state,
        verdict,
        next_action: None,
    };
    report.next_action = compute_next_action(&report);

    render(&report, &opts)
}

fn render(report: &AuditReport, opts: &AuditOptions) -> Result<()> {
    if opts.json {
        let json = serde_json::to_string_pretty(report)?;
        println!("{}", json);
        return Ok(());
    }
    if opts.brief {
        render_brief(report);
    } else {
        render_human(report);
    }
    Ok(())
}

fn render_brief(report: &AuditReport) {
    print_verdict_line(report);
    let signals = brief_signals(report);
    for line in signals {
        println!("  · {line}");
    }
}

pub(crate) fn brief_signals(report: &AuditReport) -> Vec<String> {
    let mut signals = Vec::new();
    if !report.health.errors.is_empty() {
        signals.push(format!("health: {} error(s)", report.health.errors.len()));
    }
    if !report.health.warnings.is_empty() {
        signals.push(format!("health: {} warning(s)", report.health.warnings.len()));
    }
    let total_updates = report.updates.minor
        + report.updates.major
        + report.updates.flake_inputs_with_update.len();
    if total_updates > 0 {
        signals.push(format!("updates: {} pending", total_updates));
    }
    signals
}

fn render_human(report: &AuditReport) {
    println!("{}\n", "=== cheni audit ===".bold());
    print_verdict_line(report);
    println!();

    print_health_section(&report.health);
    print_updates_section(&report.updates);
    print_state_section(&report.state);
    if let Some(action) = &report.next_action {
        println!();
        println!("→ Next: {}", action);
    }
}

fn print_verdict_line(report: &AuditReport) {
    let line = match report.verdict {
        AuditVerdict::Clear => format!("{} All clear", "✓".green().bold()),
        AuditVerdict::Warnings => {
            let total = report.health.warnings.len()
                + report.updates.minor
                + report.updates.major
                + report.updates.flake_inputs_with_update.len();
            format!(
                "{} {} warning(s)",
                "⚠".yellow().bold(),
                total.to_string().yellow()
            )
        }
        AuditVerdict::Errors => format!(
            "{} {} error(s)",
            "✗".red().bold(),
            report.health.errors.len().to_string().red()
        ),
    };
    println!("{}", line);
}

fn print_health_section(health: &HealthReport) {
    if health.errors.is_empty() && health.warnings.is_empty() {
        return;
    }
    println!("{}", "Health (doctor):".bold());
    for err in &health.errors {
        println!("  {} {}", "✗".red(), err.message);
        if let Some(hint) = &err.hint {
            println!("    {} {}", "→".dimmed(), hint.dimmed());
        }
    }
    for warn in &health.warnings {
        println!("  {} {}", "⚠".yellow(), warn.message);
        if let Some(hint) = &warn.hint {
            println!("    {} {}", "→".dimmed(), hint.dimmed());
        }
    }
    if health.passed > 0 {
        println!(
            "  {} {} other check(s) passed",
            "✓".green(),
            health.passed.to_string().green()
        );
    }
    println!();
}

fn print_updates_section(updates: &UpdatesReport) {
    println!("{}", "Updates available (check):".bold());
    println!(
        "  {} {} | {} {} | {} {} | {} {} | {} {}",
        "Up to date:".dimmed(),
        updates.up_to_date.to_string().green(),
        "Minor:".dimmed(),
        updates.minor.to_string().yellow(),
        "Major:".dimmed(),
        updates.major.to_string().red(),
        "Newer:".dimmed(),
        updates.newer.to_string().cyan(),
        "Unknown:".dimmed(),
        updates.unknown.to_string().dimmed(),
    );
    if !updates.flake_inputs_with_update.is_empty() {
        println!();
        println!("  {}", "Flake inputs needing update:".bold());
        for input in &updates.flake_inputs_with_update {
            let current = input.current.as_deref().unwrap_or("?");
            let latest = input.latest_remote_date.as_deref().unwrap_or("?");
            println!(
                "    {:<24} {:<12} → latest {}",
                input.name,
                current,
                latest.cyan()
            );
        }
    }
    println!();
}

fn print_state_section(state: &StateReport) {
    if state.pins_count == 0 && state.freezes_count == 0 {
        return;
    }
    println!("{}", "State (status):".bold());
    println!(
        "  {} pin(s) · {} freeze(s) · config {}",
        state.pins_count.to_string().cyan(),
        state.freezes_count.to_string().cyan(),
        state.flake_dir.display().to_string().dimmed(),
    );
    println!();
}

#[cfg(test)]
#[path = "tests/audit.rs"]
mod tests;
