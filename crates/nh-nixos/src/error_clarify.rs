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
}
