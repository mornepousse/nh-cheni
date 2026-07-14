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
    // Take the text AFTER the marker — not just remove it — so any wrap_err
    // prefix nh added before the marker (e.g. "Failed to build configuration:")
    // is dropped too, keeping the eval summary clean.
    let text = report.find(nh_core::NIX_BUILD_ERROR_MARKER).map_or(
      report.as_str(),
      |i| &report[i + nh_core::NIX_BUILD_ERROR_MARKER.len()..],
    );
    if let Some(b) = clarify_hash_mismatch(text) {
      return Some(b);
    }
    if let Some(b) = clarify_conflicting_options(text) {
      return Some(b);
    }
    if let Some(b) = clarify_failed_assertions(text) {
      return Some(b);
    }
    let failures = parse_nix_failures(text);
    // Only render a clarified block when at least one real leaf failure
    // survived filtering; if `parse_nix_failures` dropped everything as
    // pure propagation blocks, fall through to the default report below.
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
      .map_or("", str::trim);
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
  use std::fmt::Write;

  let mut out = String::new();
  let n = failures.len();
  if failures.iter().all(|f| f.drv.is_none()) {
    // Eval error(s): just the summaries, cleanly.
    out.push_str("✗ Évaluation nix échouée :\n");
    for f in failures {
      let _ = writeln!(out, "  {}", f.summary.replace('\n', "\n  "));
    }
    return out.trim_end().to_string();
  }
  let noun = if n > 1 { "dérivations en échec (causes racines)" } else { "dérivation en échec (cause racine)" };
  let _ = writeln!(out, "✗ Build nix échoué — {n} {noun} :");
  for f in failures {
    match &f.drv {
      Some(drv) => {
        let _ = writeln!(out, "    {drv}");
      },
      None => {
        let _ = writeln!(out, "    {}", f.summary);
      },
    }
    for line in &f.log_lines {
      let _ = writeln!(out, "      {line}");
    }
    if let Some(cmd) = &f.log_cmd {
      let _ = writeln!(out, "      → {cmd}");
    }
  }
  out.push_str("  (blocs intermédiaires « N dependencies failed » masqués)");
  out
}

/// Explain a NixOS module option defined with conflicting values.
pub(crate) fn clarify_conflicting_options(text: &str) -> Option<String> {
  use std::fmt::Write;
  let text = strip_ansi(text);
  if !text.contains("has conflicting definition values:") {
    return None;
  }
  let option = text
    .split_once("The option `")
    .and_then(|(_, r)| r.split_once('\'').map(|(o, _)| o))?;
  let defs: Vec<(String, String)> = text
    .lines()
    .filter_map(|l| {
      let rest = l.trim().strip_prefix("- In `")?;
      let (file, after) = rest.split_once('\'')?;
      let value = after.trim_start().trim_start_matches(':').trim();
      Some((file.to_string(), value.to_string()))
    })
    .collect();
  if defs.is_empty() {
    return None;
  }
  let mut out = String::new();
  let _ = writeln!(
    out,
    "⚠ Conflit de configuration — l'option « {option} » est définie à"
  );
  let _ = writeln!(out, "  plusieurs endroits avec des valeurs différentes :");
  for (file, value) in &defs {
    let _ = writeln!(out, "    {file}  → {value}");
  }
  let _ = write!(
    out,
    "  Nix ne peut pas choisir. → garde une seule définition, ou impose la\n  \
     gagnante avec lib.mkForce (ou baisse la perdante avec lib.mkDefault)."
  );
  Some(out)
}

/// Explain failed NixOS module assertions (config guardrails).
pub(crate) fn clarify_failed_assertions(text: &str) -> Option<String> {
  use std::fmt::Write;
  let text = strip_ansi(text);
  let idx = text.find("Failed assertions:")?;
  let after = &text[idx + "Failed assertions:".len()..];
  let asserts: Vec<&str> = after
    .lines()
    .filter_map(|l| l.trim().strip_prefix("- "))
    .collect();
  if asserts.is_empty() {
    return None;
  }
  let mut out = String::new();
  let _ = writeln!(
    out,
    "✗ Ta config viole des garde-fous NixOS (assertions). Corrige :"
  );
  for a in &asserts {
    let _ = writeln!(out, "    • {a}");
  }
  let _ = write!(
    out,
    "  Chaque ligne est une règle de cohérence non respectée — cherche l'option\n  \
     correspondante dans tes modules récemment édités."
  );
  Some(out)
}

/// Explain a fixed-output hash mismatch and offer the got hash as the fix.
pub(crate) fn clarify_hash_mismatch(text: &str) -> Option<String> {
  use std::fmt::Write;
  let text = strip_ansi(text);
  if !text.contains("hash mismatch") {
    return None;
  }
  let field = |name: &str| {
    text
      .lines()
      .find_map(|l| l.trim().strip_prefix(name))
      .map(|v| v.trim().to_string())
  };
  let specified = field("specified:")?;
  let got = field("got:")?;
  let mut out = String::new();
  let _ = writeln!(
    out,
    "✗ Hash incorrect pour une source à contenu fixe. Nix a obtenu un contenu"
  );
  let _ = writeln!(out, "  différent de l'attendu :");
  let _ = writeln!(out, "    attendu : {specified}");
  let _ = writeln!(out, "    obtenu  : {got}");
  let _ = writeln!(
    out,
    "  → remplace « attendu » par {got} dans le .nix qui déclare cette source"
  );
  let _ = write!(
    out,
    "    (fetchurl/fetchFromGitHub/…). Si tu n'attendais PAS de changement,\n    \
     méfie-toi (source altérée)."
  );
  Some(out)
}

/// Explain an unknown NixOS/home-manager option defined in the config.
pub(crate) fn clarify_option_does_not_exist(text: &str) -> Option<String> {
  use std::fmt::Write;
  let text = strip_ansi(text);
  if !text.contains("does not exist") || !text.contains("The option `") {
    return None;
  }
  let option = text
    .split_once("The option `")
    .and_then(|(_, r)| r.split_once('\'').map(|(o, _)| o))?;
  let mut out = String::new();
  let _ = writeln!(out, "⚠ Option inconnue « {option} » (définie dans ta config).");
  let _ = writeln!(out, "  Nix ne connaît pas cette option. Vérifie, dans l'ordre :");
  let _ = writeln!(out, "    1. une faute de frappe dans le nom ;");
  let _ = writeln!(out, "    2. le module qui la déclare n'est pas importé ;");
  let _ = write!(
    out,
    "    3. elle a été renommée/supprimée dans un bump nixpkgs récent\n       \
     (release notes NixOS / home-manager)."
  );
  Some(out)
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
  fn try_clarify_eval_error_drops_wrap_err_prefix() {
    use color_eyre::eyre::eyre;
    // An eval error (no "Cannot build") wrapped with nh's "Failed to build
    // configuration:" prefix before the marker. The prefix must NOT leak into
    // the rendered summary.
    let err = eyre!(
      "{}\nflake 'git+file:///x' does not provide attribute 'packages.x86_64-linux.foo'",
      nh_core::NIX_BUILD_ERROR_MARKER
    )
    .wrap_err("Failed to build configuration");
    let probe = FakeProbe { failed: vec![], cause: None };
    let out = try_clarify_with(&err, &probe).expect("should clarify eval error");
    assert!(out.contains("does not provide attribute"));
    assert!(!out.contains("Failed to build configuration"), "wrap_err prefix must not leak:\n{out}");
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
  fn render_nix_block_plural() {
    let fails = vec![
      NixFailure {
        drv:       Some("/nix/store/aaa-boom.drv".to_string()),
        summary:   "builder failed (builder failed with exit code 1)".to_string(),
        log_lines: vec![],
        log_cmd:   None,
      },
      NixFailure {
        drv:       Some("/nix/store/bbb-bang.drv".to_string()),
        summary:   "builder failed (builder failed with exit code 1)".to_string(),
        log_lines: vec![],
        log_cmd:   None,
      },
    ];
    let block = render_nix_block(&fails);
    assert!(block.contains("dérivations"), "must use plural noun:\n{block}");
    assert!(block.contains("/nix/store/aaa-boom.drv"));
    assert!(block.contains("/nix/store/bbb-bang.drv"));
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

  #[test]
  fn clarify_conflicting_options_extracts_option_files_values() {
    let text = "The option `foo' has conflicting definition values:\n\
                - In `/etc/nixos/module-a.nix': \"B\"\n\
                - In `/etc/nixos/module-b.nix': \"A\"\n\
                Use `lib.mkForce value' or `lib.mkDefault value' to change the priority.";
    let block = clarify_conflicting_options(text).expect("should recognize conflict");
    assert!(block.contains("foo"), "option name");
    assert!(block.contains("/etc/nixos/module-a.nix") && block.contains("\"B\""));
    assert!(block.contains("/etc/nixos/module-b.nix") && block.contains("\"A\""));
    assert!(block.contains("mkForce"));
    assert!(!block.contains("command.rs"));
  }

  #[test]
  fn clarify_conflicting_options_none_on_unrelated() {
    assert!(clarify_conflicting_options("error: something else").is_none());
  }

  #[test]
  fn clarify_failed_assertions_lists_each() {
    let text = "\nFailed assertions:\n\
                - cheni assert fail exemple\n\
                - The 'fileSystems' option does not specify your root file system.";
    let block = clarify_failed_assertions(text).expect("should recognize assertions");
    assert!(block.contains("cheni assert fail exemple"));
    assert!(block.contains("does not specify your root file system"));
    assert!(block.to_lowercase().contains("assertion"));
    assert!(!block.contains("command.rs"));
  }

  #[test]
  fn clarify_failed_assertions_none_on_unrelated() {
    assert!(clarify_failed_assertions("error: something else").is_none());
  }

  #[test]
  fn clarify_hash_mismatch_gives_got_as_action() {
    let text = "hash mismatch in fixed-output derivation '/nix/store/abc-boom-hash.drv':\n\
                \x20 specified: sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\n\
                \x20 got:       sha256-Zm9vYmFyYmF6cXV4Y29ycmVjdGhhc2h2YWx1ZTE=";
    let block = clarify_hash_mismatch(text).expect("should recognize hash mismatch");
    assert!(block.contains("sha256-AAAAAAAA"), "shows specified");
    assert!(block.contains("sha256-Zm9vYmFy"), "shows got");
    // the got hash is offered as the fix action
    assert!(block.contains("remplace"));
    assert!(!block.contains("command.rs"));
  }

  #[test]
  fn clarify_hash_mismatch_none_on_unrelated() {
    assert!(clarify_hash_mismatch("error: something else").is_none());
  }

  #[test]
  fn clarify_option_does_not_exist_names_option_and_checklist() {
    let text = "The option `services.thisOptionDoesNotExistCheni' does not exist. \
                Definition values:\n- In `<unknown-file>':\n    {\n      enable = true;\n    }";
    let block = clarify_option_does_not_exist(text).expect("should recognize unknown option");
    assert!(block.contains("services.thisOptionDoesNotExistCheni"));
    assert!(block.to_lowercase().contains("faute de frappe"));
    assert!(block.contains("nixpkgs"));
    assert!(!block.contains("command.rs"));
  }

  #[test]
  fn clarify_option_does_not_exist_none_on_unrelated() {
    assert!(clarify_option_does_not_exist("error: something else").is_none());
    // "conflicting definition values" is a DIFFERENT class — must not match here
    assert!(clarify_option_does_not_exist("The option `x' has conflicting definition values:").is_none());
  }

  #[test]
  fn try_clarify_routes_to_specific_class_blocks() {
    use color_eyre::eyre::eyre;
    let probe = FakeProbe { failed: vec![], cause: None };

    let conflict = eyre!(
      "{}\nThe option `foo' has conflicting definition values:\n- In `/a.nix': \"B\"\n- In `/b.nix': \"A\"\nUse `lib.mkForce value'.",
      nh_core::NIX_BUILD_ERROR_MARKER
    );
    let out = try_clarify_with(&conflict, &probe).expect("conflict clarified");
    assert!(out.contains("Conflit de configuration"), "specific block, not generic:\n{out}");

    let assertions = eyre!(
      "{}\nFailed assertions:\n- The 'fileSystems' option does not specify your root file system.",
      nh_core::NIX_BUILD_ERROR_MARKER
    );
    let out = try_clarify_with(&assertions, &probe).expect("assertions clarified");
    assert!(out.contains("garde-fous NixOS"), "specific block:\n{out}");

    let hash = eyre!(
      "{}\nhash mismatch in fixed-output derivation '/nix/store/x.drv':\n  specified: sha256-AAAA\n  got: sha256-BBBB",
      nh_core::NIX_BUILD_ERROR_MARKER
    );
    let out = try_clarify_with(&hash, &probe).expect("hash clarified");
    assert!(out.contains("Hash incorrect") && out.contains("sha256-BBBB"), "specific block:\n{out}");
  }
}
