//! `cheni audit` — single-shot health overview.
//!
//! Composes the structured reports from `doctor` (health), `check`
//! (updates), and `status` (state) into one ordered output that
//! prioritises action: TL;DR verdict → errors → updates → state →
//! next-action tip.
//!
//! See `docs/superpowers/specs/2026-04-28-cheni-audit-design.md`.

// Types are defined here in the scaffolding task; the orchestrator (Task 6)
// will use them. Allow dead-code until then.
#![allow(dead_code)]

#[allow(unused_imports)]
use anyhow::Result;
use serde::Serialize;

#[allow(unused_imports)]
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

#[cfg(test)]
#[path = "tests/audit.rs"]
mod tests;
