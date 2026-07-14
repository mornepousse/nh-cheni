//! Clarify nh activation/switch failures into an actionable, readable block.
//!
//! v1 scope: `nh os switch/boot/test` activation failures. The entry point
//! [`try_clarify`] is called from `crates/nh/src/main.rs`'s error arm; when it
//! recognizes an activation failure it returns a rendered block (and the caller
//! prints it instead of the default `color_eyre` report, dropping the misleading
//! `Location:`), otherwise it returns `None` and the default report is used.

/// Meaning of a `switch-to-configuration` exit code, pinned from the
/// switch-to-configuration-ng source.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ActivationOutcome {
  /// Exit 4: activation ran, but one or more units failed to (re)start.
  UnitsFailed,
  /// Exit 1 / other non-zero: activation failed hard (system not switched).
  HardFail(i32),
}

/// Map a `switch-to-configuration` exit code to its operator meaning.
pub(crate) fn classify_exit_code(code: i32) -> ActivationOutcome {
  match code {
    4 => ActivationOutcome::UnitsFailed,
    other => ActivationOutcome::HardFail(other),
  }
}

/// Extract N from a formatted report containing `… Exited(N) …`.
pub(crate) fn parse_exit_code(report: &str) -> Option<i32> {
  let start = report.find("Exited(")? + "Exited(".len();
  let rest = &report[start..];
  let end = rest.find(')')?;
  rest[..end].trim().parse().ok()
}

/// True if this formatted report is an nh activation failure we can clarify.
///
/// Markers pinned from `nixos.rs`: the activation `Command` carries
/// `.message(ACTIVATION_MSG)`, so a failure renders as
/// `"<ACTIVATION_MSG> (exit status ExitStatus(Exited(N)))"`. Merge-safe by
/// construction: this reads the same `crate::nixos::ACTIVATION_MSG` constant
/// that `nixos.rs` tags the activation `Command` with, so an upstream rename
/// of that wording forces a merge conflict at the `nixos.rs` call site rather
/// than silently desyncing the two literals.
pub(crate) fn recognize(report: &str) -> bool {
  report.contains(crate::nixos::ACTIVATION_MSG)
    && report.contains("exit status")
    && parse_exit_code(report).is_some()
}

/// A unit that failed to start, with an optional cause line from the journal.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct FailedUnit {
  pub name:  String,
  pub cause: Option<String>,
}

/// Render the clarified block. Pure: given `outcome` and `units` it fully
/// determines the output. No I/O here.
pub(crate) fn render_block(outcome: &ActivationOutcome, units: &[FailedUnit]) -> String {
  use std::fmt::Write;

  let mut out = String::new();
  match outcome {
    ActivationOutcome::UnitsFailed => {
      out.push_str("⚠ Switch appliqué — la génération est active.\n");
      let n = units.len();
      if n == 0 {
        out.push_str(
          "  Un ou plusieurs services ont raté leur démarrage (voir la sortie ci-dessus).\n",
        );
      } else {
        let noun = if n > 1 {
          "services ont raté leur démarrage"
        } else {
          "service a raté son démarrage"
        };
        let _ = writeln!(out, "  Mais {n} {noun} :");
        for u in units {
          let _ = writeln!(out, "    {}", u.name);
          if let Some(cause) = &u.cause {
            let _ = writeln!(out, "      cause : {cause}");
          }
          let _ = writeln!(out, "      → journalctl -u {}", u.name);
        }
      }
      out.push_str("  (exit 4 de switch-to-configuration = activé, mais des units ont raté)");
    },
    ActivationOutcome::HardFail(code) => {
      let _ = writeln!(
        out,
        "✗ L'activation a échoué (code {code}) — le système n'a PAS basculé."
      );
      out.push_str("  Voir la sortie de switch-to-configuration ci-dessus.");
      for u in units {
        let _ = write!(out, "\n    {} (actuellement en échec)", u.name);
      }
    },
  }
  out
}

/// Systemd queries, behind a trait so the rendering/glue stays pure and the
/// tests never touch real `systemctl`/`journalctl`.
pub(crate) trait SystemdProbe {
  /// Units currently in the `failed` state.
  fn failed_units(&self) -> Vec<String>;
  /// Last significant error line from a unit's journal, if readable.
  fn unit_last_error(&self, unit: &str) -> Option<String>;
}

/// Glue: recognize → classify → gather units → render. `probe` is injected so
/// this is fully unit-testable without touching systemd.
pub(crate) fn try_clarify_with(
  err: &color_eyre::eyre::Report,
  probe: &dyn SystemdProbe,
) -> Option<String> {
  let report = format!("{err:#}");
  if recognize(&report) {
    let outcome = classify_exit_code(parse_exit_code(&report)?);
    let units = probe
      .failed_units()
      .into_iter()
      .map(|name| {
        let cause = probe.unit_last_error(&name);
        FailedUnit { name, cause }
      })
      .collect::<Vec<_>>();
    return Some(render_block(&outcome, &units));
  }
  if recognize_nix_build(&report) {
    // Strip the marker line before parsing the nix text.
    let text = report.replacen(nh_core::NIX_BUILD_ERROR_MARKER, "", 1);
    let failures = parse_nix_failures(&text);
    if !failures.is_empty() {
      return Some(render_nix_block(&failures));
    }
  }
  None
}

use std::process::Command;

/// Parse `systemctl --failed --no-legend --plain` stdout into unit names.
/// Pure so it's testable without a real systemd. Keeps only the first
/// whitespace-separated token per line, and only tokens that look like a
/// unit name (contain a `.`), to skip legend/blank lines.
fn parse_failed_units(stdout: &str) -> Vec<String> {
  stdout
    .lines()
    .filter_map(|l| l.split_whitespace().next())
    .filter(|u| u.contains('.'))
    .map(str::to_string)
    .collect()
}

/// Pick the last significant error line out of `journalctl` output. Pure so
/// it's testable without a real systemd. Skips systemd's own boilerplate
/// ("failed with result", "failed to start") to surface the application's
/// own error line instead.
fn extract_last_error(journal_text: &str) -> Option<String> {
  journal_text
    .lines()
    .filter(|l| {
      let low = l.to_lowercase();
      (low.contains("error") || low.contains("failed"))
        && !low.contains("failed with result")
        && !low.contains("failed to start")
    })
    .last()
    .map(|l| l.trim().to_string())
}

/// Real probe: shells `systemctl` / `journalctl`. All failures degrade to an
/// empty result / `None` so clarification never itself errors.
pub(crate) struct RealProbe;

impl SystemdProbe for RealProbe {
  fn failed_units(&self) -> Vec<String> {
    let Ok(out) = Command::new("systemctl")
      .args(["--failed", "--no-legend", "--plain", "--no-pager"])
      .output()
    else {
      return Vec::new();
    };
    parse_failed_units(&String::from_utf8_lossy(&out.stdout))
  }

  fn unit_last_error(&self, unit: &str) -> Option<String> {
    let out = Command::new("journalctl")
      .args(["-u", unit, "-b", "--no-pager", "-n", "50", "-o", "cat"])
      .output()
      .ok()?;
    extract_last_error(&String::from_utf8_lossy(&out.stdout))
  }
}

/// Entry point used from `main.rs`. Returns a clarified block for recognized
/// activation failures, else `None`.
#[must_use]
pub fn try_clarify(err: &color_eyre::eyre::Report) -> Option<String> {
  try_clarify_with(err, &RealProbe)
}

/// Remove ANSI CSI escape sequences (`ESC [ … m` etc.) for clean display.
pub(crate) fn strip_ansi(s: &str) -> String {
  let mut out = String::with_capacity(s.len());
  let mut chars = s.chars().peekable();
  while let Some(c) = chars.next() {
    if c == '\u{1b}' {
      // Skip until the final byte of the escape (0x40..=0x7e), e.g. 'm'.
      if chars.peek() == Some(&'[') {
        chars.next();
        while let Some(&n) = chars.peek() {
          chars.next();
          if ('\u{40}'..='\u{7e}').contains(&n) {
            break;
          }
        }
      }
    } else {
      out.push(c);
    }
  }
  out
}

/// One failed derivation (or, for eval errors, one summary block).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct NixFailure {
  pub drv:       Option<String>,
  pub summary:   String,
  pub log_lines: Vec<String>,
  pub log_cmd:   Option<String>,
}

/// Parse the collected nix error text into root-cause failures.
/// Splits on `Cannot build '…'.` blocks; drops blocks whose Reason is
/// `N dependencies failed.` (pure propagation). Text with no `Cannot build`
/// block (eval errors) becomes a single summary failure.
pub(crate) fn parse_nix_failures(text: &str) -> Vec<NixFailure> {
  let text = strip_ansi(text);
  if !text.contains("Cannot build '") {
    let summary = text.trim().to_string();
    if summary.is_empty() {
      return Vec::new();
    }
    return vec![NixFailure { drv: None, summary, log_lines: Vec::new(), log_cmd: None }];
  }
  let mut out = Vec::new();
  // Each block starts at a "Cannot build '" occurrence.
  let mut rest = text.as_str();
  while let Some(start) = rest.find("Cannot build '") {
    let after = &rest[start..];
    let end = after[1..].find("Cannot build '").map_or(after.len(), |i| i + 1);
    let block = &after[..end];
    rest = &after[end..];

    let reason = block
      .lines()
      .find_map(|l| l.trim().strip_prefix("Reason:"))
      .map(str::trim)
      .unwrap_or("");
    // Drop pure propagation blocks.
    if reason.contains("dependency failed") || reason.contains("dependencies failed") {
      continue;
    }
    let drv = block
      .split_once("Cannot build '")
      .and_then(|(_, r)| r.split_once('\'').map(|(d, _)| d.to_string()));
    let log_lines = block
      .lines()
      .filter_map(|l| l.trim().strip_prefix("> "))
      .map(str::to_string)
      .collect();
    let log_cmd = block
      .lines()
      .map(str::trim)
      .find(|l| l.starts_with("nix log "))
      .map(str::to_string);
    let summary = format!("builder failed{}", if reason.is_empty() { String::new() } else { format!(" ({reason})") });
    out.push(NixFailure { drv, summary, log_lines, log_cmd });
  }
  out
}

/// True if the report is a captured nix build/eval failure (marker present).
pub(crate) fn recognize_nix_build(report: &str) -> bool {
  report.contains(nh_core::NIX_BUILD_ERROR_MARKER)
}

/// Render the clarified nix-failure block. Pure given `failures`.
pub(crate) fn render_nix_block(failures: &[NixFailure]) -> String {
  let mut out = String::new();
  let n = failures.len();
  if failures.iter().all(|f| f.drv.is_none()) {
    // Eval error(s): just the summaries, cleanly.
    out.push_str("✗ Évaluation nix échouée :\n");
    for f in failures {
      out.push_str(&format!("  {}\n", f.summary.replace('\n', "\n  ")));
    }
    return out.trim_end().to_string();
  }
  let noun = if n > 1 { "dérivations en échec (causes racines)" } else { "dérivation en échec (cause racine)" };
  out.push_str(&format!("✗ Build nix échoué — {n} {noun} :\n"));
  for f in failures {
    match &f.drv {
      Some(drv) => out.push_str(&format!("    {drv}\n")),
      None => out.push_str(&format!("    {}\n", f.summary)),
    }
    for line in &f.log_lines {
      out.push_str(&format!("      {line}\n"));
    }
    if let Some(cmd) = &f.log_cmd {
      out.push_str(&format!("      → {cmd}\n"));
    }
  }
  out.push_str("  (blocs intermédiaires « N dependencies failed » masqués)");
  out
}

#[cfg(test)]
#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
mod tests {
  use super::*;

  #[test]
  fn classify_four_is_units_failed() {
    assert_eq!(classify_exit_code(4), ActivationOutcome::UnitsFailed);
  }

  #[test]
  fn classify_one_is_hard_fail() {
    assert_eq!(classify_exit_code(1), ActivationOutcome::HardFail(1));
  }

  #[test]
  fn classify_other_is_hard_fail_with_code() {
    assert_eq!(classify_exit_code(7), ActivationOutcome::HardFail(7));
  }

  #[test]
  fn parse_exit_code_from_real_report() {
    let report = "Activating configuration (exit status ExitStatus(Exited(4)))";
    assert_eq!(parse_exit_code(report), Some(4));
  }

  #[test]
  fn parse_exit_code_absent_is_none() {
    assert_eq!(parse_exit_code("some unrelated error"), None);
  }

  #[test]
  fn recognize_real_activation_failure() {
    // Built FROM the shared constant, not a decoupled literal: this test
    // rides `crate::nixos::ACTIVATION_MSG` the same way `recognize` does, so
    // an upstream rename of the message shows up as a merge conflict at the
    // `nixos.rs` call site, not as a silently-desynced test string.
    let report = format!(
      "Activation (test) failed: {} (exit status ExitStatus(Exited(4)))",
      crate::nixos::ACTIVATION_MSG
    );
    assert!(recognize(&report));
  }

  #[test]
  fn recognize_rejects_unrelated_error() {
    assert!(!recognize("error: build of derivation failed"));
  }

  #[test]
  fn recognize_requires_parseable_code() {
    // activation-ish text but no Exited(N) → not our clarifiable case
    let report = format!("{} (exit status Signal(9))", crate::nixos::ACTIVATION_MSG);
    assert!(!recognize(&report));
  }

  fn unit(name: &str, cause: Option<&str>) -> FailedUnit {
    FailedUnit { name: name.to_string(), cause: cause.map(str::to_string) }
  }

  #[test]
  fn render_units_failed_with_cause() {
    let block = render_block(
      &ActivationOutcome::UnitsFailed,
      &[unit("flatpak-setup.service", Some("Could not resolve hostname"))],
    );
    assert!(block.contains("Switch appliqué"), "must reassure switch applied:\n{block}");
    assert!(block.contains("flatpak-setup.service"));
    assert!(block.contains("Could not resolve hostname"));
    assert!(block.contains("journalctl -u flatpak-setup.service"));
    assert!(block.contains("exit 4"));
    // never surfaces nh's own source location
    assert!(!block.contains("command.rs"), "must not leak nh source location");
  }

  #[test]
  fn render_units_failed_without_cause_falls_back_to_hint() {
    let block = render_block(
      &ActivationOutcome::UnitsFailed,
      &[unit("foo.service", None)],
    );
    assert!(block.contains("foo.service"));
    assert!(block.contains("journalctl -u foo.service"));
    assert!(!block.contains("cause :"), "no cause line when journal is unreadable");
  }

  #[test]
  fn render_units_failed_plural() {
    let block = render_block(
      &ActivationOutcome::UnitsFailed,
      &[unit("foo.service", None), unit("bar.service", None)],
    );
    assert!(block.contains("services ont raté"), "must use plural noun:\n{block}");
    assert!(block.contains("foo.service"));
    assert!(block.contains("bar.service"));
  }

  #[test]
  fn render_units_failed_zero_units() {
    let block = render_block(&ActivationOutcome::UnitsFailed, &[]);
    assert!(!block.contains("0 service"), "must not render broken grammar:\n{block}");
    assert!(
      block.contains("Un ou plusieurs services ont raté leur démarrage"),
      "must render a sensible fallback line:\n{block}"
    );
  }

  #[test]
  fn render_hard_fail_says_not_switched() {
    let block = render_block(&ActivationOutcome::HardFail(1), &[]);
    assert!(block.contains("code 1"));
    assert!(block.contains("n'a PAS"), "hard fail must say system not switched:\n{block}");
  }

  use color_eyre::eyre::eyre;

  struct FakeProbe {
    failed: Vec<String>,
    cause:  Option<String>,
  }
  impl SystemdProbe for FakeProbe {
    fn failed_units(&self) -> Vec<String> { self.failed.clone() }
    fn unit_last_error(&self, _unit: &str) -> Option<String> { self.cause.clone() }
  }

  #[test]
  fn try_clarify_with_recognized_activation() {
    // A report whose formatted form carries the activation markers + Exited(4),
    // built from the shared constant (see `recognize_real_activation_failure`).
    let err = eyre!(
      "Activation (test) failed: {} (exit status ExitStatus(Exited(4)))",
      crate::nixos::ACTIVATION_MSG
    );
    let probe = FakeProbe {
      failed: vec!["flatpak-setup.service".to_string()],
      cause:  Some("Could not resolve hostname".to_string()),
    };
    let out = try_clarify_with(&err, &probe).expect("should clarify");
    assert!(out.contains("flatpak-setup.service"));
    assert!(out.contains("Could not resolve hostname"));
  }

  #[test]
  fn try_clarify_with_unrecognized_returns_none() {
    let err = eyre!("error: build failed");
    let probe = FakeProbe { failed: vec![], cause: None };
    assert!(try_clarify_with(&err, &probe).is_none());
  }

  #[test]
  fn try_clarify_handles_nix_build_error() {
    use color_eyre::eyre::eyre;
    let err = eyre!(
      "{}\nCannot build '/nix/store/aaa-boom.drv'.\nReason: builder failed with exit code 1.\nLast 1 log lines:\n> oops\nFor full logs, run:\n  nix log /nix/store/aaa-boom.drv",
      nh_core::NIX_BUILD_ERROR_MARKER
    );
    // Activation probe is irrelevant here; reuse the fake from earlier tests.
    let probe = FakeProbe { failed: vec![], cause: None };
    let out = try_clarify_with(&err, &probe).expect("should clarify nix build error");
    assert!(out.contains("Build nix échoué"));
    assert!(out.contains("/nix/store/aaa-boom.drv"));
    assert!(out.contains("nix log /nix/store/aaa-boom.drv"));
  }

  #[test]
  fn real_probe_is_a_systemd_probe() {
    // Compile-time guarantee RealProbe implements the trait and try_clarify
    // wires it. We do NOT call systemd here (not parallel-safe).
    fn assert_impl(_p: &dyn SystemdProbe) {}
    assert_impl(&RealProbe);
  }

  #[test]
  fn parse_failed_units_from_fixture() {
    let stdout = "\
foo.service                   loaded failed failed Foo Service
bar.service                   loaded failed failed Bar Service
  not-a-unit-legend-line
";
    assert_eq!(
      parse_failed_units(stdout),
      vec!["foo.service".to_string(), "bar.service".to_string()]
    );
  }

  #[test]
  fn parse_failed_units_empty_stdout_is_empty() {
    assert!(parse_failed_units("").is_empty());
  }

  #[test]
  fn extract_last_error_finds_application_error_line() {
    let journal = "\
systemd[1]: Starting Foo Service...
foo[123]: error: could not resolve hostname
systemd[1]: foo.service: Failed with result 'exit-code'.
";
    assert_eq!(
      extract_last_error(journal),
      Some("foo[123]: error: could not resolve hostname".to_string())
    );
  }

  #[test]
  fn extract_last_error_ignores_systemd_boilerplate_only() {
    let journal = "\
systemd[1]: Starting Foo Service...
systemd[1]: foo.service: Failed with result 'exit-code'.
systemd[1]: Failed to start Foo Service.
";
    assert_eq!(extract_last_error(journal), None);
  }

  #[test]
  fn strip_ansi_removes_escapes() {
    assert_eq!(strip_ansi("\u{1b}[31;1merror:\u{1b}[0m x"), "error: x");
    assert_eq!(strip_ansi("plain"), "plain");
  }

  #[test]
  fn parse_collapses_dependency_blocks_keeps_leaves() {
    // Real shape: one leaf failure + one propagation block.
    let text = "\
Cannot build '/nix/store/aaa-boom.drv'.\n\
Reason: builder failed with exit code 1.\n\
Output paths:\n  /nix/store/xxx-boom\n\
Last 1 log lines:\n> oops\n\
For full logs, run:\n  nix log /nix/store/aaa-boom.drv\n\
Cannot build '/nix/store/bbb-top.drv'.\n\
Reason: 1 dependency failed.\n\
Output paths:\n  /nix/store/yyy-top";
    let fails = parse_nix_failures(text);
    assert_eq!(fails.len(), 1, "propagation block must be dropped");
    assert_eq!(fails[0].drv.as_deref(), Some("/nix/store/aaa-boom.drv"));
    assert!(fails[0].log_lines.iter().any(|l| l.contains("oops")));
    assert_eq!(fails[0].log_cmd.as_deref(), Some("nix log /nix/store/aaa-boom.drv"));
  }

  #[test]
  fn parse_eval_error_is_a_single_summary_failure() {
    // Eval errors have no "Cannot build" block — the whole text is the summary.
    let text = "flake 'git+file:///x' does not provide attribute 'packages.x86_64-linux.foo'";
    let fails = parse_nix_failures(text);
    assert_eq!(fails.len(), 1);
    assert!(fails[0].drv.is_none());
    assert!(fails[0].summary.contains("does not provide attribute"));
  }

  #[test]
  fn recognize_nix_build_via_marker() {
    let report = format!("{}\nCannot build '/nix/store/x.drv'.", nh_core::NIX_BUILD_ERROR_MARKER);
    assert!(recognize_nix_build(&report));
    assert!(!recognize_nix_build("some unrelated error"));
  }

  #[test]
  fn render_nix_block_shows_drv_cause_and_log_cmd() {
    let fails = vec![NixFailure {
      drv:       Some("/nix/store/aaa-boom.drv".to_string()),
      summary:   "builder failed (builder failed with exit code 1)".to_string(),
      log_lines: vec!["oops".to_string()],
      log_cmd:   Some("nix log /nix/store/aaa-boom.drv".to_string()),
    }];
    let block = render_nix_block(&fails);
    assert!(block.contains("Build nix échoué"));
    assert!(block.contains("/nix/store/aaa-boom.drv"));
    assert!(block.contains("oops"));
    assert!(block.contains("nix log /nix/store/aaa-boom.drv"));
    assert!(!block.contains("command.rs"), "must not leak nh source location");
  }

  #[test]
  fn render_nix_block_eval_summary_only() {
    let fails = vec![NixFailure {
      drv: None,
      summary: "flake '…' does not provide attribute 'x'".to_string(),
      log_lines: vec![], log_cmd: None,
    }];
    let block = render_nix_block(&fails);
    assert!(block.contains("does not provide attribute"));
  }
}
