# Switch/activation error readability — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn nh's cryptic `nh os switch/boot/test` activation-failure output into a clear, actionable block (which units failed, why, what the exit code means), without the misleading color_eyre `Location:`.

**Architecture:** One cheni-spec module `crates/nh-nixos/src/error_clarify.rs` holding pure logic (exit-code classification, message recognition, block rendering) plus a `SystemdProbe` trait whose real impl shells `systemctl`/`journalctl` and whose fixture impl serves tests. A ~5-line hook in `crates/nh/src/main.rs` calls `try_clarify` in the error arm and prints the clarified block instead of the default report when the error is recognized.

**Tech Stack:** Rust 2024, `color_eyre::eyre::Report`, `std::process::Command`, inline `#[cfg(test)] mod tests`.

## Global Constraints

- Cheni-spec conventions (verbatim from CLAUDE.md): short `run()`/entry delegating to named helpers; inline `mod tests` (NOT sibling files); no `.unwrap()`/`.expect()` in prod (`?` on `color_eyre::eyre::Result`); parallel-safe tests (no `std::env::set_var`, no `set_current_dir`, no shared paths, no network, no real `systemctl`/`journalctl` — use the fixture probe).
- Inline test module header: `#[cfg(test)]` then `#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]` then `mod tests { use super::*; … }`.
- Scope v1 = switch/activation failures only. No build-error handling.
- Gate after every task: `./scripts/check.sh` (or `--fast` mid-task); must be GREEN.
- Exit-code table pinned from `switch-to-configuration-ng` source: `0`=success, `4`=activation ran but ≥1 unit failed to (re)start, `1`/other non-zero=hard failure, `100`=dry (out of scope).
- Recognizer markers pinned from `crates/nh-nixos/src/nixos.rs`: `.message("Activating configuration")` (line 393) produces `"Activating configuration (exit status ExitStatus(Exited(N)))"`; outer wrap `"Activation (test) failed"` (line 399).
- Only two upstream files touched: `crates/nh-nixos/src/lib.rs` (one `pub mod` line) and `crates/nh/src/main.rs` (the hook). No other upstream edits.

---

### Task 1: Exit-code classification + parsing (pure)

**Files:**
- Create: `crates/nh-nixos/src/error_clarify.rs`
- Test: inline `mod tests` in the same file.

**Interfaces:**
- Produces: `enum ActivationOutcome { UnitsFailed, HardFail(i32) }`; `fn classify_exit_code(code: i32) -> ActivationOutcome`; `fn parse_exit_code(report: &str) -> Option<i32>`.

- [ ] **Step 1: Write the failing test** — create the file with the doc header, the types-under-test, and tests only (no impl yet won't compile; so include stub signatures that `todo!()`). Create `crates/nh-nixos/src/error_clarify.rs`:

```rust
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
  todo!()
}

/// Extract N from a formatted report containing `… Exited(N) …`.
pub(crate) fn parse_exit_code(report: &str) -> Option<i32> {
  todo!()
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
```

Also declare the module so it compiles — add to `crates/nh-nixos/src/lib.rs` (near the other `pub mod` lines):

```rust
pub mod error_clarify;
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nh-nixos --lib error_clarify::tests::parse_exit_code_from_real_report`
Expected: FAIL — panics at `todo!()`.

- [ ] **Step 3: Write minimal implementation** — replace the two `todo!()` bodies:

```rust
pub(crate) fn classify_exit_code(code: i32) -> ActivationOutcome {
  match code {
    4 => ActivationOutcome::UnitsFailed,
    other => ActivationOutcome::HardFail(other),
  }
}

pub(crate) fn parse_exit_code(report: &str) -> Option<i32> {
  let start = report.find("Exited(")? + "Exited(".len();
  let rest = &report[start..];
  let end = rest.find(')')?;
  rest[..end].trim().parse().ok()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p nh-nixos --lib error_clarify::`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/nh-nixos/src/error_clarify.rs crates/nh-nixos/src/lib.rs
git commit -m "error_clarify: exit-code classification + parsing"
```

---

### Task 2: Recognizer (pure)

**Files:**
- Modify: `crates/nh-nixos/src/error_clarify.rs`

**Interfaces:**
- Consumes: `parse_exit_code` (Task 1).
- Produces: `fn recognize(report: &str) -> bool`.

- [ ] **Step 1: Write the failing test** — add to `mod tests`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nh-nixos --lib error_clarify::tests::recognize_real_activation_failure`
Expected: FAIL — `recognize` not found (does not compile) or, once stubbed, assertion fails.

- [ ] **Step 3: Write minimal implementation** — add above `mod tests`:

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p nh-nixos --lib error_clarify::`
Expected: PASS (8 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/nh-nixos/src/error_clarify.rs
git commit -m "error_clarify: recognize activation failures"
```

---

### Task 3: Block rendering (pure)

**Files:**
- Modify: `crates/nh-nixos/src/error_clarify.rs`

**Interfaces:**
- Consumes: `ActivationOutcome` (Task 1).
- Produces: `struct FailedUnit { pub name: String, pub cause: Option<String> }`; `fn render_block(outcome: &ActivationOutcome, units: &[FailedUnit]) -> String`.

- [ ] **Step 1: Write the failing test** — add to `mod tests`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nh-nixos --lib error_clarify::tests::render_units_failed_with_cause`
Expected: FAIL — `FailedUnit` / `render_block` not found (does not compile).

- [ ] **Step 3: Write minimal implementation** — add above `mod tests`:

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p nh-nixos --lib error_clarify::`
Expected: PASS (11 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/nh-nixos/src/error_clarify.rs
git commit -m "error_clarify: render clarified block"
```

---

### Task 4: Probe trait + glue `try_clarify_with` (pure test via fixture)

**Files:**
- Modify: `crates/nh-nixos/src/error_clarify.rs`

**Interfaces:**
- Consumes: `recognize`, `parse_exit_code`, `classify_exit_code`, `render_block`, `FailedUnit` (Tasks 1–3).
- Produces: `trait SystemdProbe { fn failed_units(&self) -> Vec<String>; fn unit_last_error(&self, unit: &str) -> Option<String>; }`; `fn try_clarify_with(err: &color_eyre::eyre::Report, probe: &dyn SystemdProbe) -> Option<String>`.

- [ ] **Step 1: Write the failing test** — add to `mod tests`:

```rust
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
    // A report whose formatted form carries the activation markers + Exited(4).
    let err = eyre!(
      "Activation (test) failed: Activating configuration (exit status ExitStatus(Exited(4)))"
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nh-nixos --lib error_clarify::tests::try_clarify_with_recognized_activation`
Expected: FAIL — `SystemdProbe` / `try_clarify_with` not found (does not compile).

- [ ] **Step 3: Write minimal implementation** — add above `mod tests`:

```rust
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
  if !recognize(&report) {
    return None;
  }
  let outcome = classify_exit_code(parse_exit_code(&report)?);
  let units = probe
    .failed_units()
    .into_iter()
    .map(|name| {
      let cause = probe.unit_last_error(&name);
      FailedUnit { name, cause }
    })
    .collect::<Vec<_>>();
  Some(render_block(&outcome, &units))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p nh-nixos --lib error_clarify::`
Expected: PASS (13 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/nh-nixos/src/error_clarify.rs
git commit -m "error_clarify: SystemdProbe trait + try_clarify_with glue"
```

---

### Task 5: Real probe (shell-out) + public `try_clarify`

**Files:**
- Modify: `crates/nh-nixos/src/error_clarify.rs`

**Interfaces:**
- Consumes: `SystemdProbe`, `try_clarify_with` (Task 4).
- Produces: `struct RealProbe`; `impl SystemdProbe for RealProbe`; `pub fn try_clarify(err: &color_eyre::eyre::Report) -> Option<String>`.

No new unit test (shell-out is intentionally untested — it is behind the trait; the glue is already covered via the fake probe in Task 4). A `#[test]` that constructs `RealProbe` and calls `failed_units()` would hit real systemd → forbidden (not parallel-safe, environment-dependent). Instead we add a compile-only smoke assertion that `RealProbe` implements the trait.

- [ ] **Step 1: Write the failing test** — add to `mod tests`:

```rust
  #[test]
  fn real_probe_is_a_systemd_probe() {
    // Compile-time guarantee RealProbe implements the trait and try_clarify
    // wires it. We do NOT call systemd here (not parallel-safe).
    fn assert_impl(_p: &dyn SystemdProbe) {}
    assert_impl(&RealProbe);
  }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nh-nixos --lib error_clarify::tests::real_probe_is_a_systemd_probe`
Expected: FAIL — `RealProbe` not found (does not compile).

- [ ] **Step 3: Write minimal implementation** — add above `mod tests`:

```rust
use std::process::Command;

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
    String::from_utf8_lossy(&out.stdout)
      .lines()
      .filter_map(|l| l.split_whitespace().next())
      .filter(|u| u.contains('.'))
      .map(str::to_string)
      .collect()
  }

  fn unit_last_error(&self, unit: &str) -> Option<String> {
    let out = Command::new("journalctl")
      .args(["-u", unit, "-b", "--no-pager", "-n", "50", "-o", "cat"])
      .output()
      .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // Last line that looks like an application error, not systemd boilerplate.
    text
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
}

/// Entry point used from `main.rs`. Returns a clarified block for recognized
/// activation failures, else `None`.
pub fn try_clarify(err: &color_eyre::eyre::Report) -> Option<String> {
  try_clarify_with(err, &RealProbe)
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p nh-nixos --lib error_clarify::`
Expected: PASS (14 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/nh-nixos/src/error_clarify.rs
git commit -m "error_clarify: real systemd/journal probe + public try_clarify"
```

---

### Task 6: Wire the hook into `main.rs`

**Files:**
- Modify: `crates/nh/src/main.rs` (error arm)
- (lib.rs `pub mod error_clarify;` already added in Task 1.)

**Interfaces:**
- Consumes: `nh_nixos::error_clarify::try_clarify` (Task 5).

- [ ] **Step 1: Inspect the current error handling** — read the end of `main`:

Run: `grep -nE 'fn main|color_eyre|install|Err\(|return Err|\?;|Result' crates/nh/src/main.rs | head -30`
Identify where the top-level `Result` is returned/handled. Two shapes are possible:
- `fn main() -> Result<()> { … run() }` (color_eyre prints the report automatically), or
- an explicit `match run() { Err(e) => … }`.

- [ ] **Step 2: Write the hook** — insert the clarifier at the single top-level failure point. If `main` returns `Result`, convert to explicit handling; if it already matches on the error, add the branch. Target shape:

```rust
fn main() -> std::process::ExitCode {
  // ... existing color_eyre install + setup, unchanged ...
  if let Err(report) = real_main() {
    if let Some(block) = nh_nixos::error_clarify::try_clarify(&report) {
      eprintln!("{block}");
      return std::process::ExitCode::FAILURE;
    }
    // Unchanged default behaviour: let color_eyre render the full report.
    eprintln!("{report:?}");
    return std::process::ExitCode::FAILURE;
  }
  std::process::ExitCode::SUCCESS
}
```

Adapt `real_main`/`run` to the actual function name in the file. Keep the existing color_eyre setup untouched — only the *error rendering* branch changes. If `main` currently is `-> color_eyre::Result<()>`, rename the body to a helper (`fn run() -> Result<()>`) and make `main` the wrapper above. Do NOT change any success path.

- [ ] **Step 3: Build**

Run: `cargo build -p nh`
Expected: compiles. If `try_clarify` is unresolved, confirm `nh` depends on `nh-nixos` (it does — check `crates/nh/Cargo.toml`).

- [ ] **Step 4: Verify tests + clippy still pass**

Run: `./scripts/check.sh --fast`
Expected: GREEN.

- [ ] **Step 5: Commit**

```bash
git add crates/nh/src/main.rs
git commit -m "nh: clarify activation failures via error_clarify hook"
```

---

### Task 7: Version bump, live verification, full gate

**Files:**
- Modify: `Cargo.toml` (`[workspace.package].version`)
- Modify: `.tripwire-testcount` (ratchet — new tests added)

- [ ] **Step 1: Bump the cheni layer** — new feature ⇒ minor bump of the cheni layer, nh-base unchanged. Edit `Cargo.toml`:

```toml
version = "4.4.1+cheni.0.2.0"
```

- [ ] **Step 2: Full gate**

Run: `./scripts/check.sh`
Expected: GREEN (fast + clippy + build). Cargo.lock updates cleanly for the version bump.

- [ ] **Step 3: Live smoke test (manual, real system)** — the clarifier only triggers on a real activation failure, which is hard to force safely. Instead verify the two rendering paths against the real binary:

```bash
# Build the fork
cargo build --release -p nh
# Reproduce the specimen shape: a switch that leaves flatpak-setup failing
# (offline) — OR trust the unit tests for rendering and just confirm the hook
# path doesn't fire on a *successful* dry build:
./target/release/nh os build 2>&1 | tail -5   # must NOT print the clarified block
```
Expected: on a successful `nh os build`, no clarified block appears (try_clarify returns None on non-activation errors and on success). Record the result. A true activation-failure smoke test is opportunistic — the next real failed switch should show the block; note that in the release checklist rather than forcing a failure here.

- [ ] **Step 4: Update the ratchet** — the new tests raise the count; `check.sh` will have bumped `.tripwire-testcount`. Confirm and stage it.

Run: `git status --porcelain .tripwire-testcount`
Expected: shows the file modified (higher count).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock .tripwire-testcount
git commit -m "release: cheni-layer 0.2.0 — switch error readability"
```

---

## Self-Review

**Spec coverage:**
- Non-zero exit + clear "switch applied, units failed" message → Task 3 (`UnitsFailed` render) + Task 6 (exit FAILURE). ✓
- Auto-enrich via journalctl + fallback → Task 5 (`unit_last_error`) + Task 3 (cause line optional / hint always). ✓
- Suppress color_eyre `Location:` → Task 6 (print block, skip `{report:?}`). ✓
- Recognizer + exit-code table pinned → Tasks 1–2, Global Constraints. ✓
- Pure logic + `SystemdProbe` trait behind shell-out → Tasks 4–5. ✓
- Parallel-safe tests, no real systemd/network → fake probe (Task 4), no systemd call in tests (Task 5 compile-only). ✓
- Only two upstream files touched → lib.rs (Task 1), main.rs (Task 6). ✓
- v1 switch-only → no build-error task present. ✓

**Placeholder scan:** No TBD/TODO in steps; every code step shows full code.

**Type consistency:** `ActivationOutcome`, `FailedUnit { name, cause }`, `SystemdProbe::{failed_units, unit_last_error}`, `try_clarify_with(&Report, &dyn SystemdProbe)`, `try_clarify(&Report)` — names/signatures identical across Tasks 1→6. ✓
