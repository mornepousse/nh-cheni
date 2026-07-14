//! Clarify nh activation/switch failures into an actionable, readable block.
//!
//! v1 scope: `nh os switch/boot/test` activation failures. The entry point
//! [`try_clarify`] is called from `crates/nh/src/main.rs`'s error arm; when it
//! recognizes an activation failure it returns a rendered block (and the caller
//! prints it instead of the default color_eyre report, dropping the misleading
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
/// Markers pinned from `nixos.rs`: the activation Command carries
/// `.message("Activating configuration")`, so a failure renders as
/// `"Activating configuration (exit status ExitStatus(Exited(N)))"`.
/// Merge-watch: if upstream changes that wording, the recognizer tests turn
/// red — that red is the signal to re-pin the markers.
pub(crate) fn recognize(report: &str) -> bool {
  report.contains("Activating configuration")
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
  let mut out = String::new();
  match outcome {
    ActivationOutcome::UnitsFailed => {
      out.push_str("⚠ Switch appliqué — la génération est active.\n");
      let n = units.len();
      let noun = if n > 1 { "services ont raté leur démarrage" } else { "service a raté son démarrage" };
      out.push_str(&format!("  Mais {n} {noun} :\n"));
      for u in units {
        out.push_str(&format!("    {}\n", u.name));
        if let Some(cause) = &u.cause {
          out.push_str(&format!("      cause : {cause}\n"));
        }
        out.push_str(&format!("      → journalctl -u {}\n", u.name));
      }
      out.push_str("  (exit 4 de switch-to-configuration = activé, mais des units ont raté)");
    },
    ActivationOutcome::HardFail(code) => {
      out.push_str(&format!(
        "✗ L'activation a échoué (code {code}) — le système n'a PAS basculé.\n"
      ));
      out.push_str("  Voir la sortie de switch-to-configuration ci-dessus.");
      for u in units {
        out.push_str(&format!("\n    {} (actuellement en échec)", u.name));
      }
    },
  }
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
    let report = "Activation (test) failed: Activating configuration \
                  (exit status ExitStatus(Exited(4)))";
    assert!(recognize(report));
  }

  #[test]
  fn recognize_rejects_unrelated_error() {
    assert!(!recognize("error: build of derivation failed"));
  }

  #[test]
  fn recognize_requires_parseable_code() {
    // activation-ish text but no Exited(N) → not our clarifiable case
    assert!(!recognize("Activating configuration (exit status Signal(9))"));
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
  fn render_hard_fail_says_not_switched() {
    let block = render_block(&ActivationOutcome::HardFail(1), &[]);
    assert!(block.contains("code 1"));
    assert!(block.contains("n'a PAS"), "hard fail must say system not switched:\n{block}");
  }
}
