# cheni audit Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `cheni audit` — a single-shot health overview that consolidates `doctor`, `check`, and `status` into one ordered report with verdict line, next-action tip, `--brief`, and `--json`.

**Architecture:** Extract `collect_health() / collect_updates() / collect_state()` from the three existing commands so each returns structured data instead of just printing. Add a new `src/cmd/audit.rs` orchestrator that calls the three collectors (one async, two sync) and composes a unified `AuditReport`. The existing `doctor::run` / `check::run` / `status::run` continue to wrap `collect + print` for backwards compat.

**Tech Stack:** Rust 2021, tokio, anyhow, serde, colored. No new dependencies.

**Spec source:** `docs/superpowers/specs/2026-04-28-cheni-audit-design.md`

---

## Préambule — État des lieux

Avant la première tâche, lire :

- `CLAUDE.md` (conventions code + scope cheni)
- Le spec ci-dessus
- Les commits récents Phase 1 / Phase 2 a11y :
  - `5124a23` (clap Example: lines)
  - `7ed31bd` (diagnose colors + empty)
  - `473efa9` (check verdict-first — exact pattern à étendre)
  - `d20bcd4` (--brief on check/upgrade/history)

L'engineer doit aussi noter :
- `doctor::run` est **sync** ; `check::run` est **async** (à cause de `tokio::task::spawn_blocking` pour `nix eval`) ; `status::run` est **sync**
- `doctor::CheckResult { severity, name, message, hint }` existe déjà — proche de notre `HealthIssue` cible
- `check::Classification` agrège déjà les counts — proche de notre `UpdatesReport` cible
- `audit` doit donc être `async fn run(...)` pour pouvoir `.await` la collecte updates

L'état git : working tree clean sur la branche `cheni-audit`, fork main avec le spec déjà commité (`e205c5c`).

---

### Task 1: Definir les types `AuditReport` + sub-reports dans `src/cmd/audit.rs`

**Files:**
- Create: `src/cmd/audit.rs`
- Create: `src/cmd/tests/audit.rs`
- Modify: `src/cmd/mod.rs`

- [ ] **Step 1: Créer le squelette du module avec les types**

Crée `src/cmd/audit.rs` :

```rust
//! `cheni audit` — single-shot health overview.
//!
//! Composes the structured reports from `doctor` (health), `check`
//! (updates), and `status` (state) into one ordered output that
//! prioritises action: TL;DR verdict → errors → updates → state →
//! next-action tip.
//!
//! See `docs/superpowers/specs/2026-04-28-cheni-audit-design.md`.

use anyhow::Result;
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

#[cfg(test)]
#[path = "tests/audit.rs"]
mod tests;
```

- [ ] **Step 2: Wire le module dans `src/cmd/mod.rs`**

Edit `src/cmd/mod.rs` — ajouter `pub mod audit;` en ordre alphabétique (après `mod build;` ou similaire).

- [ ] **Step 3: Test fixture file**

Crée `src/cmd/tests/audit.rs` avec un test qui vérifie que les types compilent et serialisent :

```rust
//! Tests for `cmd::audit`.

use super::*;
use std::path::PathBuf;

fn empty_report() -> AuditReport {
    AuditReport {
        health: HealthReport::default(),
        updates: UpdatesReport::default(),
        state: StateReport {
            pins_count: 0,
            freezes_count: 0,
            flake_dir: PathBuf::from("/tmp/fake-flake"),
        },
        verdict: AuditVerdict::Clear,
        next_action: None,
    }
}

#[test]
fn audit_report_serialises_to_json() {
    let report = empty_report();
    let json = serde_json::to_string(&report).expect("serialise");
    assert!(json.contains("\"verdict\":\"clear\""));
    assert!(json.contains("\"passed\":0"));
}
```

- [ ] **Step 4: Build to verify everything compiles**

```
cargo build
cargo test --lib cmd::audit
cargo clippy --all-targets -- -D warnings
```

Expected: 1 test passes, 0 warnings.

- [ ] **Step 5: Commit**

```
git add src/cmd/audit.rs src/cmd/tests/audit.rs src/cmd/mod.rs
git commit -m "feat(audit): scaffold AuditReport types

Types only (no logic yet). Subsequent tasks extract collect_*
helpers from doctor/check/status and add the orchestrator."
```

---

### Task 2: Extraire `collect_health` depuis `doctor`

**Files:**
- Modify: `src/cmd/doctor.rs`

`doctor::run` already calls `run_all_checks` and bins the results into errors/warnings/ok. Extract the binning into a public function that returns the structured `HealthReport` shape from Task 1.

- [ ] **Step 1: Read the current shape**

```
grep -n "fn run_all_checks\|fn tally_severities\|CheckResult\|Severity" src/cmd/doctor.rs | head
```

Identify where `run_all_checks` is defined and what it returns (`Vec<CheckResult>` per the inspection done before this plan).

- [ ] **Step 2: Add `collect_health`**

In `src/cmd/doctor.rs`, after `run_all_checks`, add:

```rust
/// Collect doctor's findings as structured data, suitable for
/// `cheni audit` composition.
///
/// Mirrors what `run()` does internally, minus printing. Errors / warnings
/// are converted into `audit::HealthIssue` shape.
pub(crate) fn collect_health(flake_dir: &std::path::Path) -> anyhow::Result<crate::cmd::audit::HealthReport> {
    let checks = run_all_checks(flake_dir)?;
    let mut report = crate::cmd::audit::HealthReport::default();
    for c in &checks {
        let issue = crate::cmd::audit::HealthIssue {
            name: c.name.clone(),
            message: c.message.clone(),
            hint: c.hint.clone(),
        };
        match c.severity {
            Severity::Error => report.errors.push(issue),
            Severity::Warning => report.warnings.push(issue),
            Severity::Ok => report.passed += 1,
        }
    }
    Ok(report)
}
```

(Note: this assumes `Severity` and `CheckResult` are accessible from within `doctor.rs`. They are. The function is `pub(crate)` so only the audit module can call it.)

- [ ] **Step 3: Build and verify the existing `doctor` tests still pass**

```
cargo build
cargo test --lib cmd::doctor
cargo clippy --all-targets -- -D warnings
```

Expected: all `doctor` tests still pass; no clippy warnings introduced.

- [ ] **Step 4: Commit**

```
git add src/cmd/doctor.rs
git commit -m "feat(doctor): expose collect_health for audit composition

Extracts the doctor checks into a structured HealthReport that
audit can consume. The existing run() is unchanged in surface."
```

---

### Task 3: Extraire `collect_state` depuis `status`

**Files:**
- Modify: `src/cmd/status.rs`

`status::run` reads pins, freezes, and the flake_dir. Extract a `collect_state` returning the structured `StateReport`.

- [ ] **Step 1: Read the current shape**

```
grep -n "fn run\|read_input_by_name\|pins::read\|freezes::read" src/cmd/status.rs | head
```

- [ ] **Step 2: Add `collect_state`**

In `src/cmd/status.rs`:

```rust
/// Collect status's structured data, suitable for `cheni audit`.
pub(crate) fn collect_state(nix_config: &config::NixConfig) -> anyhow::Result<crate::cmd::audit::StateReport> {
    let pins = crate::nix::pins::read(&nix_config.flake_dir).unwrap_or_default();
    let freezes = crate::nix::freezes::read(&nix_config.flake_dir).unwrap_or_default();
    Ok(crate::cmd::audit::StateReport {
        pins_count: pins.len(),
        freezes_count: freezes.len(),
        flake_dir: nix_config.flake_dir.clone(),
    })
}
```

(`pins::read` returns `Result<Vec<String>>`; `freezes::read` returns `Result<HashMap<String, FreezeEntry>>` or similar — `.unwrap_or_default()` for both is the same pattern `status::run` already uses internally.)

- [ ] **Step 3: Build and verify**

```
cargo build
cargo test --lib cmd::status
cargo clippy --all-targets -- -D warnings
```

- [ ] **Step 4: Commit**

```
git add src/cmd/status.rs
git commit -m "feat(status): expose collect_state for audit composition"
```

---

### Task 4: Extraire `collect_updates` depuis `check`

**Files:**
- Modify: `src/cmd/check.rs`

`check::run` is the most complex of the three — it runs the eval batch via `tokio::task::spawn_blocking`. The collect function must be `async`.

- [ ] **Step 1: Identify the right boundary**

`check::run` does:
1. Apply early flags (`json`, `refresh`)
2. Detect nix_config
3. Init check
4. Gather packages
5. Split frozen
6. `fetch_updates_concurrently` — runs the eval batch + flake input probes
7. `classify_lookups`
8. Render

For `collect_updates`, we want steps 4–7 (the data) and skip 1–3 (flag/setup) + 8 (rendering). The caller (audit) handles surrounding logic.

- [ ] **Step 2: Add `collect_updates`**

In `src/cmd/check.rs`, near the top after the existing struct defs:

```rust
/// Collect check's structured updates data, suitable for `cheni audit`.
///
/// Runs the same eval batch + flake-input probe as `run()`, but returns
/// structured data instead of printing.
pub(crate) async fn collect_updates(
    nix_config: &config::NixConfig,
) -> anyhow::Result<crate::cmd::audit::UpdatesReport> {
    let Some(mut scan) = gather_packages_to_check(nix_config, None)? else {
        return Ok(crate::cmd::audit::UpdatesReport::default());
    };
    let current_freezes = freezes::read(&nix_config.flake_dir)?;
    let frozen_rows = split_out_frozen(&mut scan.packages, &current_freezes);

    // brief=true so fetch_updates_concurrently skips the spinner.
    let (lookup_map, flake_inputs) = fetch_updates_concurrently(nix_config, &scan, true).await?;

    let classification = classify_lookups(
        &scan.packages,
        &lookup_map,
        &scan.names_with_files,
        &nix_config.flake_dir,
    );

    let visible_flake_inputs = filter_visible_flake_inputs(
        &flake_inputs,
        &nix_config.flake_dir,
        scan.active_set.as_ref(),
        None,
    );
    let flake_inputs_with_update: Vec<crate::cmd::audit::FlakeInputUpdate> = visible_flake_inputs
        .iter()
        .filter(|i| i.has_update.unwrap_or(false))
        .map(|i| crate::cmd::audit::FlakeInputUpdate {
            name: i.name.clone(),
            current: i.current_version.clone(),
            latest_remote_date: i.latest_remote_date.clone(),
        })
        .collect();

    Ok(crate::cmd::audit::UpdatesReport {
        up_to_date: classification.up_to_date,
        minor: classification.minor.len(),
        major: classification.major.len(),
        newer: classification.newer.len(),
        unknown: classification.unknown.len(),
        frozen: frozen_rows.len(),
        flake_inputs_with_update,
    })
}
```

**Note on `FlakeInput` field names**: `has_update`, `current_version`, `latest_remote_date` are the typical names from `nix::flake::FlakeInput`. If the actual struct uses different names (e.g. `latest_date`, `current`), adapt accordingly. **The engineer should grep `nix/flake.rs` to verify field names before pasting this code.**

- [ ] **Step 3: Verify visibility on the helpers used**

The collect function uses these private helpers from `check.rs`:
- `gather_packages_to_check`
- `split_out_frozen`
- `fetch_updates_concurrently`
- `classify_lookups`
- `filter_visible_flake_inputs`

They're all in the same file → callable. If any are `pub(crate)` already, fine; if private, fine too (same module). No additional changes needed.

- [ ] **Step 4: Build and verify**

```
cargo build
cargo test --lib cmd::check
cargo clippy --all-targets -- -D warnings
```

If field names mismatch on `FlakeInput`, fix them.

- [ ] **Step 5: Commit**

```
git add src/cmd/check.rs
git commit -m "feat(check): expose collect_updates async for audit composition"
```

---

### Task 5: Implémenter `audit::compute_verdict` + `audit::compute_next_action`

**Files:**
- Modify: `src/cmd/audit.rs`
- Modify: `src/cmd/tests/audit.rs`

The verdict and next-action are pure functions over the report. TDD them first.

- [ ] **Step 1: Add tests in `src/cmd/tests/audit.rs`**

```rust
#[test]
fn verdict_clear_when_no_issues_and_no_updates() {
    let report = empty_report();
    assert_eq!(compute_verdict(&report.health, &report.updates), AuditVerdict::Clear);
}

#[test]
fn verdict_warnings_on_minor_update() {
    let mut report = empty_report();
    report.updates.minor = 1;
    assert_eq!(compute_verdict(&report.health, &report.updates), AuditVerdict::Warnings);
}

#[test]
fn verdict_warnings_on_health_warning() {
    let mut report = empty_report();
    report.health.warnings.push(HealthIssue {
        name: "stale input".into(),
        message: "...".into(),
        hint: None,
    });
    assert_eq!(compute_verdict(&report.health, &report.updates), AuditVerdict::Warnings);
}

#[test]
fn verdict_errors_on_health_error() {
    let mut report = empty_report();
    report.health.errors.push(HealthIssue {
        name: "not init".into(),
        message: "...".into(),
        hint: None,
    });
    assert_eq!(compute_verdict(&report.health, &report.updates), AuditVerdict::Errors);
}

#[test]
fn next_action_points_at_health_error_first() {
    let mut report = empty_report();
    report.health.errors.push(HealthIssue {
        name: "flake.lock dirty".into(),
        message: "...".into(),
        hint: None,
    });
    report.verdict = AuditVerdict::Errors;
    let action = compute_next_action(&report);
    assert!(action.unwrap().contains("flake.lock dirty"));
}

#[test]
fn next_action_suggests_upgrade_on_flake_input_update() {
    let mut report = empty_report();
    report.updates.flake_inputs_with_update.push(FlakeInputUpdate {
        name: "claude-code".into(),
        current: Some("2.1.119".into()),
        latest_remote_date: Some("2026-04-28".into()),
    });
    report.verdict = AuditVerdict::Warnings;
    let action = compute_next_action(&report);
    assert!(action.unwrap().contains("cheni upgrade"));
}

#[test]
fn next_action_none_on_clear() {
    let report = empty_report();
    assert!(compute_next_action(&report).is_none());
}
```

- [ ] **Step 2: Run tests to verify they fail**

```
cargo test --lib cmd::audit
```

Expected: 7 failures (functions don't exist yet).

- [ ] **Step 3: Implement `compute_verdict` and `compute_next_action`**

In `src/cmd/audit.rs`, append:

```rust
/// Derive the overall verdict from health + updates.
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

/// Pick the single most actionable next step given the report.
/// Returns None when verdict is Clear (no action needed).
pub(crate) fn compute_next_action(report: &AuditReport) -> Option<String> {
    if let Some(err) = report.health.errors.first() {
        return Some(format!("Address `{}` first — it blocks rebuild.", err.name));
    }
    if report.updates.major > 0 {
        return Some(
            "Run `cheni check --details` to see major updates, then `cheni upgrade` to take them.".into()
        );
    }
    if !report.updates.flake_inputs_with_update.is_empty() {
        return Some(
            "Run `cheni upgrade` to take the flake-input updates listed above.".into()
        );
    }
    if let Some(warn) = report.health.warnings.first() {
        return Some(format!("Optional: address `{}` (warning).", warn.name));
    }
    None
}
```

- [ ] **Step 4: Run tests to verify they pass**

```
cargo test --lib cmd::audit
```

Expected: 8 tests pass (7 new + the JSON serialise from Task 1).

- [ ] **Step 5: Commit**

```
git add src/cmd/audit.rs src/cmd/tests/audit.rs
git commit -m "feat(audit): compute_verdict + compute_next_action

Pure derivation functions, fully unit-tested. Verdict precedence:
errors > warnings/updates > clear. Next-action precedence: error >
major update > flake-input update > warning > none."
```

---

### Task 6: Implémenter `audit::run` orchestrator

**Files:**
- Modify: `src/cmd/audit.rs`

- [ ] **Step 1: Add the orchestrator function**

Append to `src/cmd/audit.rs`:

```rust
use colored::Colorize;

/// Options controlling audit's output.
#[derive(Debug, Default)]
pub struct AuditOptions {
    pub brief: bool,
    pub json: bool,
}

/// Run `cheni audit`.
///
/// Calls `collect_health`, `collect_updates`, `collect_state` (the
/// updates one is async because of the eval batch underneath).
/// Composes an `AuditReport` and renders it.
pub async fn run(opts: AuditOptions) -> Result<()> {
    let nix_config = config::detect()?;

    // Short-circuit for fully uninitialised flakes — health will surface
    // the error; running the other two would mostly produce noise.
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

    // Three collectes — updates is async, the others sync. We don't need
    // them in parallel; the eval batch dominates and runs in spawn_blocking
    // anyway. Keep the orchestration sequential and readable.
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

fn brief_signals(report: &AuditReport) -> Vec<String> {
    let mut signals = Vec::new();
    if !report.health.errors.is_empty() {
        signals.push(format!("health: {} error(s)", report.health.errors.len()));
    }
    if !report.health.warnings.is_empty() {
        signals.push(format!("health: {} warning(s)", report.health.warnings.len()));
    }
    let total_updates = report.updates.minor + report.updates.major + report.updates.flake_inputs_with_update.len();
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
            format!("{} {} warning(s)", "⚠".yellow().bold(), total.to_string().yellow())
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
        println!("  {} {} other check(s) passed", "✓".green(), health.passed.to_string().green());
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
            println!("    {:<24} {:<12} → latest {}", input.name, current, latest.cyan());
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
```

- [ ] **Step 2: Build and verify**

```
cargo build
cargo clippy --all-targets -- -D warnings
cargo test --lib cmd::audit
```

Expected: all tests pass, no warnings.

- [ ] **Step 3: Commit**

```
git add src/cmd/audit.rs
git commit -m "feat(audit): orchestrator + human/brief/json renderers

Composes the three collect_* helpers into an AuditReport, with
render variants for default human output, --brief one-line summary,
and --json structured output. Short-circuits on uninit flakes."
```

---

### Task 7: Wire `cheni audit` into clap dispatch

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Add the `Audit` variant to the `Commands` enum**

Find the `enum Commands` in `src/main.rs`. Add a new variant alphabetically (between `About` and `Build` or wherever audit fits):

```rust
/// One-shot health overview. Combines doctor + check + status into a
/// single ordered report with a verdict line and a next-action tip.
///
/// Example: cheni audit
#[command(after_help = "Example: cheni audit --brief")]
Audit {
    /// Print a one-line summary instead of the full report.
    #[arg(long)]
    brief: bool,

    /// Output structured JSON for scripts.
    #[arg(long)]
    json: bool,
},
```

- [ ] **Step 2: Add the dispatch arm**

Find the `match cli.command` block in `src/main.rs`. Add an arm for `Audit`:

```rust
Commands::Audit { brief, json } => {
    cmd::audit::run(cmd::audit::AuditOptions { brief, json }).await?;
}
```

(Match indentation/ordering with the existing arms — they're typically alphabetical.)

- [ ] **Step 3: Update the after_help cheatsheet**

In `src/main.rs`, find the `after_help` string at the top of the Cli struct (around line 28). Add a line for `cheni audit` near `cheni status` since they're conceptually adjacent:

```
  cheni audit                    Combined health: doctor + check + status, ordered\n  \
```

Place it between the existing `cheni status` line and the next section header.

- [ ] **Step 4: Build and verify**

```
cargo build
cargo run -- audit --help     # should show the help with Example
cargo run -- --help            # should mention audit in the cheatsheet
cargo clippy --all-targets -- -D warnings
cargo test --lib
```

- [ ] **Step 5: Commit**

```
git add src/main.rs
git commit -m "feat(cli): wire 'cheni audit' subcommand

Dispatches to cmd::audit::run with --brief / --json flags."
```

---

### Task 8: Smoke test + tests sibling pour render helpers

**Files:**
- Modify: `src/cmd/tests/audit.rs`

The pure helpers (`compute_verdict`, `compute_next_action`) are already covered by Task 5. Add tests for `brief_signals` since that's the other pure helper of any complexity.

- [ ] **Step 1: Add tests**

Append to `src/cmd/tests/audit.rs`:

```rust
#[test]
fn brief_signals_empty_when_clear() {
    let report = empty_report();
    assert!(brief_signals(&report).is_empty());
}

#[test]
fn brief_signals_lists_health_warnings() {
    let mut report = empty_report();
    report.health.warnings.push(HealthIssue {
        name: "stale".into(),
        message: "stale".into(),
        hint: None,
    });
    let signals = brief_signals(&report);
    assert_eq!(signals.len(), 1);
    assert!(signals[0].contains("warning"));
}

#[test]
fn brief_signals_lists_pending_updates() {
    let mut report = empty_report();
    report.updates.minor = 2;
    report.updates.flake_inputs_with_update.push(FlakeInputUpdate {
        name: "claude-code".into(),
        current: Some("1".into()),
        latest_remote_date: Some("2".into()),
    });
    let signals = brief_signals(&report);
    let updates_line = signals.iter().find(|s| s.contains("updates")).expect("updates line");
    assert!(updates_line.contains("3 pending"));  // 2 minor + 1 flake-input
}
```

- [ ] **Step 2: Run tests**

```
cargo test --lib cmd::audit
cargo clippy --all-targets -- -D warnings
```

- [ ] **Step 3: Smoke test on the actual host**

```
cargo build --release
nix build .#cheni
./result/bin/cheni audit
./result/bin/cheni audit --brief
./result/bin/cheni audit --json | jq .
```

Expected:
- Default output has TL;DR line + sections + next-action tip
- `--brief` prints a single line + signals (or just verdict if clear)
- `--json` is parseable JSON with verdict, health, updates, state, next_action keys

If the hostname/flake setup makes any of these fail, take note in the commit message.

- [ ] **Step 4: Commit tests**

```
git add src/cmd/tests/audit.rs
git commit -m "test(audit): coverage for brief_signals helper"
```

---

### Task 9: Vérification finale

**Files:** N/A (gates de qualité)

- [ ] **Step 1: Pre-merge gate**

```
cargo build
cargo build --release
cargo clippy --all-targets -- -D warnings
cargo test
```

Expected: tout passe.

- [ ] **Step 2: Sandbox Nix gate**

```
nix build .#cheni
./result/bin/cheni audit
./result/bin/cheni audit --brief
./result/bin/cheni audit --json | jq .verdict
```

Expected: build sandbox passes, all three modes produce valid output.

- [ ] **Step 3: Verify no regression on doctor / check / status**

```
./result/bin/cheni doctor
./result/bin/cheni check --brief
./result/bin/cheni status --brief
```

Expected: identical to v0.6.x behaviour (existing run() functions unchanged in surface).

- [ ] **Step 4: Diff stat**

```
git diff main..HEAD --stat
```

Expected: positive net diff (~500-800 LOC added across `src/cmd/audit.rs`, sibling tests, and small additions to doctor/check/status/main.rs).

- [ ] **Step 5: Merge to main + push**

(Controller decides — don't auto-merge. Surface the branch state to the user for review.)

---

## Auto-review du plan

**Spec coverage** :
- ✅ Refactor extracts collect_* — Tasks 2/3/4
- ✅ Audit module + types — Task 1
- ✅ verdict + next-action heuristics — Task 5
- ✅ Orchestrator — Task 6
- ✅ CLI dispatch — Task 7
- ✅ Tests — Tasks 1, 5, 8
- ✅ --brief, --json modes — Tasks 6, 7
- ✅ Edge case "uninit flake" — Task 6 short-circuit
- ✅ Verification gates — Task 9

**Placeholders** : aucun "TBD"/"TODO". Une note dans Task 4 Step 2 dit "engineer should grep nix/flake.rs to verify field names" — c'est honnête (le plan a été écrit sans ouvrir flake.rs juste-pour-cette-vérification), pas un placeholder.

**Type consistency** :
- `AuditReport`, `HealthReport`, `UpdatesReport`, `StateReport`, `HealthIssue`, `FlakeInputUpdate`, `AuditVerdict` — définis Task 1, utilisés cohéremment Tasks 2-8
- `AuditOptions { brief: bool, json: bool }` — Task 6, consommé Task 7
- Function signatures :
  - `collect_health(flake_dir: &Path) -> Result<HealthReport>` — Task 2
  - `collect_updates(nix_config: &NixConfig) -> Result<UpdatesReport>` async — Task 4
  - `collect_state(nix_config: &NixConfig) -> Result<StateReport>` — Task 3
  - `compute_verdict(&HealthReport, &UpdatesReport) -> AuditVerdict` — Task 5
  - `compute_next_action(&AuditReport) -> Option<String>` — Task 5
  - `run(AuditOptions) -> Result<()>` async — Task 6

**Out-of-scope respecté** :
- Pas de tracking persistant (Phase 3e)
- Pas de filtre `--category`
- Pas de cache
- Pas de modification de la surface publique de `doctor::run` / `check::run` / `status::run`
