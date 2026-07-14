# Nix build/eval error readability (v2) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `nh os build/switch` show the real nix build/eval error (root cause, `nix log` hint) cleanly, instead of a content-free `exit status` + misleading `Location:`.

**Architecture:** In `command.rs` (nh-core), tee nix's `internal-json` stream (forward verbatim to `nom`, collect error `raw_msg`s) and `bail!` an error carrying a shared marker + the real text. In `error_clarify.rs` (nh-nixos, cheni), extend the existing v1 clarifier with pure functions that recognize that marker, collapse `N dependencies failed` propagation blocks, strip ANSI, and render a clean block. `main.rs` is unchanged (its v1 hook already routes through `error_clarify::try_clarify`).

**Tech Stack:** Rust 2024, `subprocess` 1.2.0 (`Exec`, `Popen` with `.stdout`/`.stdin: Option<File>`, `Redirection`), `serde_json` (workspace dep, already in nh-core), `color_eyre::eyre`, inline `#[cfg(test)] mod tests`.

## Global Constraints

- Cheni conventions: no `.unwrap()`/`.expect()` in production (`?` / `let-else` / `.ok()`); inline `#[cfg(test)] mod tests` with `#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]`; short entry fns delegating to named helpers; clean clippy on new cheni code.
- Parallel-safe tests: no env/CWD mutation, no shared paths, no network, **no real nix/systemctl/journalctl** — fixtures only.
- Communication nh-core → cheni is **via the error message text only** (no cross-crate types; nh-nixos depends on nh-core, never the reverse).
- Recognizer keyed on a **shared constant** `nh_core::NIX_BUILD_ERROR_MARKER` (merge-safe like v1's `ACTIVATION_MSG`), frozen by a test.
- internal-json shape (verified live, nix 2.34.8): lines prefixed `@nix `; the full error is the event `{"action":"msg","level":0,"raw_msg":"…"}`. `raw_msg` contains ANSI escapes and `\n`.
- Scope MVP: class-agnostic clean surfacing + `Reason: N dependencies failed.` collapse. **Do NOT** implement hash/assertions/conflicts renderers or the non-nom path (v2.1).
- Gate after every task: `./scripts/check.sh` (`--fast` mid-task); must be GREEN.
- Version bump on ship: cheni-layer `4.4.1+cheni.0.2.0` → `4.4.1+cheni.0.3.0`.

## File structure
- `crates/nh-core/src/command.rs` (upstream, modify): `NIX_BUILD_ERROR_MARKER` const, a pure `extract_nix_error_raw_msgs(chunk) -> Vec<String>`, the tee helper, and the restructured `Build::run` nom branch.
- `crates/nh-core/src/lib.rs` (upstream, modify): `pub use command::NIX_BUILD_ERROR_MARKER;` (clean path for cheni).
- `crates/nh-nixos/src/error_clarify.rs` (cheni, modify): ANSI strip, `NixFailure`, `parse_nix_failures`, `recognize_nix_build`, `render_nix_block`, wired into `try_clarify_with`.
- `Cargo.toml` (modify): version bump.

---

### Task 1: ANSI strip + nix-failure parsing (pure, cheni)

**Files:**
- Modify: `crates/nh-nixos/src/error_clarify.rs`

**Interfaces:**
- Produces: `fn strip_ansi(s: &str) -> String`; `struct NixFailure { drv: Option<String>, summary: String, log_lines: Vec<String>, log_cmd: Option<String> }`; `fn parse_nix_failures(text: &str) -> Vec<NixFailure>`.

- [ ] **Step 1: Write the failing test** — add to `error_clarify.rs`'s `mod tests`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nh-nixos --lib error_clarify::tests::parse_collapses_dependency_blocks_keeps_leaves`
Expected: FAIL — `strip_ansi`/`NixFailure`/`parse_nix_failures` not found (does not compile).

- [ ] **Step 3: Write minimal implementation** — add above `mod tests`:

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p nh-nixos --lib error_clarify::`
Expected: PASS (existing 20 + 3 new).

- [ ] **Step 5: Commit**

```bash
git add crates/nh-nixos/src/error_clarify.rs
git commit -m "error_clarify: parse nix failures (collapse deps, strip ANSI)"
```

---

### Task 2: Recognizer + block rendering (pure, cheni)

**Files:**
- Modify: `crates/nh-nixos/src/error_clarify.rs`
- Modify: `crates/nh-core/src/command.rs` (add the marker const — the recognizer references it)
- Modify: `crates/nh-core/src/lib.rs` (re-export the const)

**Interfaces:**
- Consumes: `parse_nix_failures`, `NixFailure` (Task 1); `nh_core::NIX_BUILD_ERROR_MARKER`.
- Produces: `fn recognize_nix_build(report: &str) -> bool`; `fn render_nix_block(failures: &[NixFailure]) -> String`.

- [ ] **Step 1: Add the shared marker in nh-core** — in `crates/nh-core/src/command.rs`, near the top (module scope):

```rust
/// Marker cheni prepends to a captured nix build/eval error so the clarifier
/// (in nh-nixos) can recognize it from the error report. Merge-safe: a rename
/// forces a compile break at the one call site.
pub const NIX_BUILD_ERROR_MARKER: &str = "nh-cheni:nix-build-error";
```
and in `crates/nh-core/src/lib.rs`, add:
```rust
pub use command::NIX_BUILD_ERROR_MARKER;
```
Run `cargo build -p nh-core` to confirm it compiles and the path `nh_core::NIX_BUILD_ERROR_MARKER` resolves.

- [ ] **Step 2: Write the failing test** — add to `error_clarify.rs`'s `mod tests`:

```rust
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
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p nh-nixos --lib error_clarify::tests::recognize_nix_build_via_marker`
Expected: FAIL — `recognize_nix_build`/`render_nix_block` not found.

- [ ] **Step 4: Write minimal implementation** — add above `mod tests` in `error_clarify.rs`:

```rust
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
```

- [ ] **Step 5: Run tests + gate**

Run: `cargo test -p nh-nixos --lib error_clarify::` then `./scripts/check.sh --fast`
Expected: PASS / GREEN.

- [ ] **Step 6: Commit**

```bash
git add crates/nh-core/src/command.rs crates/nh-core/src/lib.rs crates/nh-nixos/src/error_clarify.rs
git commit -m "error_clarify: recognize + render nix build failures (shared marker)"
```

---

### Task 3: Wire nix-build clarification into `try_clarify`

**Files:**
- Modify: `crates/nh-nixos/src/error_clarify.rs`

**Interfaces:**
- Consumes: `recognize` (v1 activation), `recognize_nix_build`, `parse_nix_failures`, `render_nix_block` (Tasks 1-2), `try_clarify_with`.

- [ ] **Step 1: Write the failing test** — add to `mod tests`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nh-nixos --lib error_clarify::tests::try_clarify_handles_nix_build_error`
Expected: FAIL — `try_clarify_with` returns `None` for the nix-build report (activation recognizer doesn't match).

- [ ] **Step 3: Write minimal implementation** — in `try_clarify_with`, after the activation branch and before the final `None`, add the nix-build branch. The function currently formats `let report = format!("{err:#}");` and returns activation clarification when `recognize(&report)`. Add:

```rust
  if recognize_nix_build(&report) {
    // Strip the marker line before parsing the nix text.
    let text = report.replacen(nh_core::NIX_BUILD_ERROR_MARKER, "", 1);
    let failures = parse_nix_failures(&text);
    if !failures.is_empty() {
      return Some(render_nix_block(&failures));
    }
  }
```
Place it so activation is tried first, then nix-build, then `None` (default report).

- [ ] **Step 4: Run tests + gate**

Run: `cargo test -p nh-nixos --lib error_clarify::` then `./scripts/check.sh --fast`
Expected: PASS / GREEN.

- [ ] **Step 5: Commit**

```bash
git add crates/nh-nixos/src/error_clarify.rs
git commit -m "error_clarify: route nix build errors through try_clarify"
```

---

### Task 4: Pure JSON extraction in nh-core (capture-side, testable)

**Files:**
- Modify: `crates/nh-core/src/command.rs`

**Interfaces:**
- Produces: `fn extract_nix_error_raw_msgs(chunk: &str) -> Vec<String>` — parse `@nix ` NDJSON lines, return the `raw_msg` of every `{"action":"msg","level":0,…}` event.

- [ ] **Step 1: Write the failing test** — add a `#[cfg(test)] mod build_error_tests` (or extend the existing test module) in `command.rs`:

```rust
#[cfg(test)]
#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
mod build_error_tests {
  use super::*;

  #[test]
  fn extracts_level0_raw_msgs_only() {
    let chunk = concat!(
      "@nix {\"action\":\"stop\",\"id\":1}\n",
      "@nix {\"action\":\"msg\",\"level\":3,\"raw_msg\":\"note: not an error\"}\n",
      "@nix {\"action\":\"msg\",\"level\":0,\"raw_msg\":\"Cannot build '/nix/store/x.drv'.\"}\n",
      "not-a-nix-line\n",
    );
    let msgs = extract_nix_error_raw_msgs(chunk);
    assert_eq!(msgs, vec!["Cannot build '/nix/store/x.drv'.".to_string()]);
  }

  #[test]
  fn ignores_malformed_json_lines() {
    let chunk = "@nix {not json}\n@nix {\"action\":\"msg\",\"level\":0,\"raw_msg\":\"boom\"}\n";
    assert_eq!(extract_nix_error_raw_msgs(chunk), vec!["boom".to_string()]);
  }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nh-core --lib build_error_tests::extracts_level0_raw_msgs_only`
Expected: FAIL — `extract_nix_error_raw_msgs` not found.

- [ ] **Step 3: Write minimal implementation** — add near `NIX_BUILD_ERROR_MARKER` in `command.rs`:

```rust
/// Parse `@nix `-prefixed internal-json lines and return the `raw_msg` of each
/// error event (`action == "msg"`, `level == 0`). Malformed lines are skipped.
pub(crate) fn extract_nix_error_raw_msgs(chunk: &str) -> Vec<String> {
  chunk
    .lines()
    .filter_map(|l| l.strip_prefix("@nix "))
    .filter_map(|j| serde_json::from_str::<serde_json::Value>(j).ok())
    .filter(|v| v.get("action").and_then(|a| a.as_str()) == Some("msg")
             && v.get("level").and_then(serde_json::Value::as_u64) == Some(0))
    .filter_map(|v| v.get("raw_msg").and_then(|m| m.as_str()).map(str::to_string))
    .collect()
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p nh-core --lib build_error_tests::`
Expected: PASS (2).

- [ ] **Step 5: Commit**

```bash
git add crates/nh-core/src/command.rs
git commit -m "command: pure extraction of nix error raw_msgs from internal-json"
```

---

### Task 5: Tee `Build::run` nom branch + bail with marker+text

**Files:**
- Modify: `crates/nh-core/src/command.rs` (`Build::run`, nom branch, ~lines 1017-1047) and its `use std::{…}`.

**Interfaces:**
- Consumes: `extract_nix_error_raw_msgs` (Task 4), `NIX_BUILD_ERROR_MARKER` (Task 2).

This task's core is subprocess threading — not unit-testable; validate by build + manual smoke. Keep the helper thin.

- [ ] **Step 1: Ensure imports** — the nom branch needs `std::io::{BufRead, BufReader, Write}` and `std::thread`. Add them to the top-of-file `use std::{…}` block (check what's already imported first with `grep -n 'use std' crates/nh-core/src/command.rs`).

- [ ] **Step 2: Replace the nom branch body** — the current nom branch (inside `if self.nom {` … `}`) builds a `nix | nom` pipeline via the `|` operator and `.start()`. Replace it with a manual tee. Target code:

```rust
    if self.nom {
      // Spawn nix (stdout piped to us) and nom (stdin piped from us) separately
      // so we can tee: forward every byte to nom verbatim (display unchanged)
      // AND collect nix's internal-json error messages.
      let mut nix = base_command
        .args(["--log-format", "internal-json", "--verbose"])
        .stderr(Redirection::Merge)
        .stdout(Redirection::Pipe)
        .popen()?;
      let mut nom = Exec::cmd("nom")
        .args(["--json"])
        .stdin(Redirection::Pipe)
        .popen()?;

      let nix_out = nix.stdout.take().ok_or_else(|| eyre::eyre!("nix stdout pipe missing"))?;
      let nom_in = nom.stdin.take().ok_or_else(|| eyre::eyre!("nom stdin pipe missing"))?;

      // Copy thread: drain nix stdout continuously (no deadlock), forward to
      // nom verbatim, collect error raw_msgs.
      let collector = std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(nix_out);
        let mut sink = nom_in;
        let mut errors: Vec<String> = Vec::new();
        let mut line = String::new();
        loop {
          line.clear();
          match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
          }
          // Forward verbatim first (byte-fidelity for nom).
          let _ = sink.write_all(line.as_bytes());
          errors.extend(extract_nix_error_raw_msgs(&line));
        }
        let _ = sink.flush();
        drop(sink); // close nom's stdin so it can finish
        errors
      });

      let nix_status = nix.wait()?;
      let errors = collector.join().unwrap_or_default();
      let _ = nom.wait();

      if !nix_status.success() {
        if errors.is_empty() {
          bail!(ExitError(nix_status));
        }
        bail!("{}\n{}", NIX_BUILD_ERROR_MARKER, errors.join("\n"));
      }
    } else {
```
Keep the existing `else { … }` (non-nom) branch and the trailing `Ok(())` unchanged.

Notes for the implementer:
- `nix.wait()` returns `subprocess::Result<ExitStatus>`; propagate with `?`.
- `collector.join()` returns `thread::Result<Vec<String>>`; `.unwrap_or_default()` avoids a prod panic (a poisoned thread → treat as no captured errors → falls back to `ExitError`).
- Do NOT introduce a prod `.unwrap()`/`.expect()`. The `ok_or_else` + `unwrap_or_default` + `let _ =` patterns above are the sanctioned forms.

- [ ] **Step 3: Build**

Run: `cargo build -p nh-core -p nh`
Expected: compiles. Fix any `subprocess` API mismatch (in 1.2.0, `Popen.stdout`/`.stdin` are `Option<std::fs::File>`; `.take()` yields the `File`).

- [ ] **Step 4: Gate**

Run: `./scripts/check.sh` (full — this touches upstream nh-core, run clippy+build)
Expected: GREEN.

- [ ] **Step 5: Manual smoke (real nix, deliberate failure)**

```bash
mkdir -p /tmp/nh-v2-smoke && printf '%s' '{ outputs = _: { packages.x86_64-linux.boom = derivation { name="boom"; system="x86_64-linux"; builder="/bin/sh"; args=["-c" "echo oops >&2; exit 1"]; }; }; }' > /tmp/nh-v2-smoke/flake.nix
cargo build --release -p nh
./target/release/nh os build --help >/dev/null   # sanity
# Drive a real failing build through nh's Build path (nom on):
NH_NO_CHECKS=1 ./target/release/nh build 'path:/tmp/nh-v2-smoke#boom' 2>&1 | tail -12 || true
```
Expected: the clarified block appears — `✗ Build nix échoué …`, the `boom.drv`, `> oops` (or the reason), `→ nix log …`, and NO `Location: crates/nh-core/src/command.rs`. Also confirm nom's live progress rendering during the build looks unchanged. If nh has no bare `build` subcommand, drive via `nh os build` against a config whose closure includes the broken drv, or note the smoke was done via the next real failing switch. Record the actual output.

- [ ] **Step 6: Commit**

```bash
git add crates/nh-core/src/command.rs
git commit -m "command: tee nix internal-json to nom + capture real error text"
```

---

### Task 6: Version bump, full gate, ratchet

**Files:**
- Modify: `Cargo.toml` (`[workspace.package].version`)
- Modify: `.tripwire-testcount`

- [ ] **Step 1: Bump the cheni layer** — edit `Cargo.toml`:

```toml
version = "4.4.1+cheni.0.3.0"
```

- [ ] **Step 2: Full gate**

Run: `./scripts/check.sh`
Expected: GREEN (fast + clippy + build); Cargo.lock updates cleanly.

- [ ] **Step 3: Update the ratchet** — `check.sh` bumps `.tripwire-testcount` (new tests added). Confirm and stage it.

Run: `git status --porcelain .tripwire-testcount`
Expected: modified (higher count).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock .tripwire-testcount
git commit -m "release: cheni-layer 0.3.0 — nix build/eval error readability"
```

---

## Self-Review

**Spec coverage:**
- Tee internal-json capture (nh-core, capture-only) → Task 4 (pure extract) + Task 5 (tee + bail). ✓
- Class-agnostic clean surfacing → Task 2 `render_nix_block` (eval + builder) + Task 3 wiring. ✓
- §2.2 dependency-failure collapse → Task 1 `parse_nix_failures` (drops `N dependencies failed`). ✓
- Drop color_eyre Location → v1 main.rs hook already prints the block instead of the default report; Task 3 makes nix-build errors recognized. ✓
- Communication via error text, shared marker const, merge-safe → Task 2 (`NIX_BUILD_ERROR_MARKER` in nh-core, recognizer test freezes it). ✓
- Parallel-safe fixture tests, no real nix → Tasks 1-4 (pure); Task 5 tee validated by manual smoke only. ✓
- main.rs unchanged → confirmed (no task edits it). ✓
- Version 0.3.0 → Task 6. ✓
- v2.1 deferrals (hash/assertions/conflicts/non-nom) → not present. ✓

**Placeholder scan:** No TBD/TODO; every code step shows full code. Task 5's smoke has a conditional ("if nh has no bare `build`…") — that's a real fallback instruction, not a placeholder.

**Type consistency:** `NixFailure { drv, summary, log_lines, log_cmd }`, `strip_ansi`, `parse_nix_failures`, `recognize_nix_build`, `render_nix_block`, `extract_nix_error_raw_msgs`, `NIX_BUILD_ERROR_MARKER` — names/signatures identical across Tasks 1→5. ✓
