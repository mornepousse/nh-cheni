# Nix error comprehension (v2.1) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** For three common nix error classes (conflicting options, failed assertions, hash mismatch), render a dedicated block that *explains* the error and gives the concrete next action — beyond v2's clean-message surfacing.

**Architecture:** Pure additions to `crates/nh-nixos/src/error_clarify.rs` (cheni): three `clarify_*(text) -> Option<String>` functions, wired into `try_clarify_with`'s existing nix-build branch BEFORE the class-agnostic `parse_nix_failures`/`render_nix_block` fallback. Zero upstream churn (the v2 tee/capture are reused as-is).

**Tech Stack:** Rust 2024, `color_eyre::eyre`, inline `#[cfg(test)] mod tests`. Reuses `strip_ansi` (already in error_clarify).

## Global Constraints

- Cheni conventions: no `.unwrap()`/`.expect()` in production (`?`/`let-else`/`.ok()`/`.map_or`); inline `#[cfg(test)] mod tests` with `#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]`.
- **Clean clippy on new code**: use `use std::fmt::Write;` + `let _ = write!/writeln!(out, …)` — NOT `push_str(&format!(…))` (the v1/v2 review trap); use `.map_or(…)` not `.map(…).unwrap_or(…)`.
- Parallel-safe tests: fixtures only, no real nix, no env/CWD/shared-path/network.
- Recognizers key on stable nix substrings, frozen by the positive tests (merge-watch).
- Each `clarify_*` strips ANSI internally (`strip_ansi`) before matching, so it is self-contained and testable with raw fixtures.
- Reuse of the existing marker: eval errors reach `try_clarify_with` via `nh_core::NIX_BUILD_ERROR_MARKER` (v2). `try_clarify_with` already extracts the text after the marker.
- Version bump on ship: `4.4.1+cheni.0.3.0` → `4.4.1+cheni.0.3.1`.
- Gate after every task: `./scripts/check.sh` (`--fast` mid-task) GREEN.

## File structure
- `crates/nh-nixos/src/error_clarify.rs` (modify): add three `clarify_*` fns + tests; wire into `try_clarify_with`.
- `Cargo.toml` (modify): version bump.

---

### Task 1: `clarify_conflicting_options` (pure)

**Files:** Modify `crates/nh-nixos/src/error_clarify.rs`
**Interfaces:** Produces `fn clarify_conflicting_options(text: &str) -> Option<String>`. Consumes `strip_ansi`.

- [ ] **Step 1: Write the failing test** — add to `mod tests`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nh-nixos --lib error_clarify::tests::clarify_conflicting_options_extracts_option_files_values`
Expected: FAIL — function not found.

- [ ] **Step 3: Write minimal implementation** — add above `mod tests`:

```rust
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
```

- [ ] **Step 4: Run tests + gate**

Run: `cargo test -p nh-nixos --lib error_clarify::` then `./scripts/check.sh --fast`
Expected: PASS / GREEN.

- [ ] **Step 5: Commit**

```bash
git add crates/nh-nixos/src/error_clarify.rs
git commit -m "error_clarify: clarify conflicting option definitions"
```

---

### Task 2: `clarify_failed_assertions` (pure)

**Files:** Modify `crates/nh-nixos/src/error_clarify.rs`
**Interfaces:** Produces `fn clarify_failed_assertions(text: &str) -> Option<String>`. Consumes `strip_ansi`.

- [ ] **Step 1: Write the failing test** — add to `mod tests`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nh-nixos --lib error_clarify::tests::clarify_failed_assertions_lists_each`
Expected: FAIL — function not found.

- [ ] **Step 3: Write minimal implementation** — add above `mod tests`:

```rust
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
```

- [ ] **Step 4: Run tests + gate**

Run: `cargo test -p nh-nixos --lib error_clarify::` then `./scripts/check.sh --fast`
Expected: PASS / GREEN.

- [ ] **Step 5: Commit**

```bash
git add crates/nh-nixos/src/error_clarify.rs
git commit -m "error_clarify: clarify failed NixOS assertions"
```

---

### Task 3: `clarify_hash_mismatch` (pure)

**Files:** Modify `crates/nh-nixos/src/error_clarify.rs`
**Interfaces:** Produces `fn clarify_hash_mismatch(text: &str) -> Option<String>`. Consumes `strip_ansi`.

- [ ] **Step 1: Write the failing test** — add to `mod tests`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nh-nixos --lib error_clarify::tests::clarify_hash_mismatch_gives_got_as_action`
Expected: FAIL — function not found.

- [ ] **Step 3: Write minimal implementation** — add above `mod tests`:

```rust
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
```

- [ ] **Step 4: Run tests + gate**

Run: `cargo test -p nh-nixos --lib error_clarify::` then `./scripts/check.sh --fast`
Expected: PASS / GREEN.

- [ ] **Step 5: Commit**

```bash
git add crates/nh-nixos/src/error_clarify.rs
git commit -m "error_clarify: clarify fixed-output hash mismatch"
```

---

### Task 4: Wire the three clarifiers into `try_clarify_with`

**Files:** Modify `crates/nh-nixos/src/error_clarify.rs`
**Interfaces:** Consumes `clarify_hash_mismatch`, `clarify_conflicting_options`, `clarify_failed_assertions` (Tasks 1-3).

- [ ] **Step 1: Write the failing test** — add to `mod tests` (one integration test per class, proving the SPECIFIC block wins over the generic one):

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nh-nixos --lib error_clarify::tests::try_clarify_routes_to_specific_class_blocks`
Expected: FAIL — the generic `render_nix_block` (or `None`) is returned, not the specific blocks.

- [ ] **Step 3: Write minimal implementation** — in `try_clarify_with`, the nix-build branch currently is:

```rust
  if recognize_nix_build(&report) {
    // (comment about taking text after the marker)
    let text = report.find(nh_core::NIX_BUILD_ERROR_MARKER).map_or(
      report.as_str(),
      |i| &report[i + nh_core::NIX_BUILD_ERROR_MARKER.len()..],
    );
    let failures = parse_nix_failures(text);
    if !failures.is_empty() {
      return Some(render_nix_block(&failures));
    }
  }
```
Insert the three specific clarifiers between the `text` binding and `parse_nix_failures`:

```rust
    if let Some(b) = clarify_hash_mismatch(text) {
      return Some(b);
    }
    if let Some(b) = clarify_conflicting_options(text) {
      return Some(b);
    }
    if let Some(b) = clarify_failed_assertions(text) {
      return Some(b);
    }
```

- [ ] **Step 4: Run tests + gate**

Run: `cargo test -p nh-nixos --lib error_clarify::` then `./scripts/check.sh --fast`
Expected: PASS / GREEN (all prior tests + the new routing test).

- [ ] **Step 5: Commit**

```bash
git add crates/nh-nixos/src/error_clarify.rs
git commit -m "error_clarify: route specific nix error classes before generic fallback"
```

---

### Task 5: Version bump, full gate, ratchet

**Files:** Modify `Cargo.toml`, `.tripwire-testcount`

- [ ] **Step 1: Bump the cheni layer** — edit `Cargo.toml`:

```toml
version = "4.4.1+cheni.0.3.1"
```

- [ ] **Step 2: Full gate**

Run: `./scripts/check.sh`
Expected: GREEN (fast + clippy + build); Cargo.lock updates cleanly; clippy clean on the new code.

- [ ] **Step 3: Update the ratchet** — `check.sh` bumps `.tripwire-testcount` (8 new tests). Confirm and stage it.

Run: `git status --porcelain .tripwire-testcount`
Expected: modified (higher count).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock .tripwire-testcount
git commit -m "release: cheni-layer 0.3.1 — nix error comprehension (conflict/assert/hash)"
```

---

## Self-Review

**Spec coverage:**
- Conflicting options renderer → Task 1. ✓
- Failed assertions renderer → Task 2. ✓
- Hash mismatch renderer (got as action) → Task 3. ✓
- Wired specific-before-generic in `try_clarify_with` → Task 4 (order hash/conflict/assertions, all before `parse_nix_failures`). ✓
- Reuse `strip_ansi`, no upstream churn → Tasks 1-3 (each strips internally); nothing outside error_clarify + Cargo.toml. ✓
- Parallel-safe fixture tests, merge-safe recognizers frozen by positive tests → Tasks 1-4. ✓
- Version 0.3.1 → Task 5. ✓
- §1.9 and others NOT implemented (out of scope) → absent. ✓

**Placeholder scan:** No TBD/TODO; every code step has full code.

**Type consistency:** `clarify_conflicting_options`, `clarify_failed_assertions`, `clarify_hash_mismatch` — all `(&str) -> Option<String>`, consistent across Tasks 1-4; all consume `strip_ansi` (existing). ✓
