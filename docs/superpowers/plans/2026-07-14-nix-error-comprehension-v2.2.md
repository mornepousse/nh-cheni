# Nix error comprehension (v2.2) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two more comprehension clarifiers — unknown NixOS option, and impure-path-forbidden — each explaining the error and steering to the right fix (never `--impure`).

**Architecture:** Same as v2.1: two pure `clarify_*(text) -> Option<String>` added to `crates/nh-nixos/src/error_clarify.rs`, wired into `try_clarify_with` after the existing specific clarifiers, before the class-agnostic fallback. Zero upstream churn (reuses the v2 tee/capture + `strip_ansi`).

**Tech Stack:** Rust 2024, inline `#[cfg(test)] mod tests`. Reuses `strip_ansi`.

## Global Constraints

- No `.unwrap()`/`.expect()` in production (`?`/`let-else`/`.ok()`/`.map_or`).
- **Clean clippy on new code**: `use std::fmt::Write;` + `let _ = write!/writeln!(out, …)` — NEVER `push_str(&format!(…))`; `.map_or` not `.map().unwrap_or`.
- Parallel-safe tests: fixtures only, no real nix/env/CWD/shared-path/network.
- Each `clarify_*` strips ANSI internally, returns `None` if not its class.
- Recognizers key on stable nix substrings, frozen by positive tests (merge-watch).
- **Never suggest `--impure`** in the impure-path block (project policy).
- Scope: exactly these two classes. NOT: option renamed (not an error — succeeds), option removed, untracked-git (nix already gives the `git add` command). Nothing else.
- Version bump on ship: `4.4.1+cheni.0.3.1` → `4.4.1+cheni.0.3.2`.
- Gate after every task: `./scripts/check.sh` (`--fast` mid-task) GREEN.

## File structure
- `crates/nh-nixos/src/error_clarify.rs` (modify): two `clarify_*` fns + tests; two lines added to `try_clarify_with`'s clarifier chain.
- `Cargo.toml` (modify): version bump.

---

### Task 1: `clarify_option_does_not_exist` (pure)

**Files:** Modify `crates/nh-nixos/src/error_clarify.rs`
**Interfaces:** Produces `fn clarify_option_does_not_exist(text: &str) -> Option<String>`. Consumes `strip_ansi`.

- [ ] **Step 1: Write the failing test** — add to `mod tests`:

```rust
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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nh-nixos --lib error_clarify::tests::clarify_option_does_not_exist_names_option_and_checklist`
Expected: FAIL — function not found.

- [ ] **Step 3: Write minimal implementation** — add above `mod tests`:

```rust
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
```

- [ ] **Step 4: Run tests + gate**

Run: `cargo test -p nh-nixos --lib error_clarify::` then `./scripts/check.sh --fast`
Expected: PASS / GREEN.

- [ ] **Step 5: Commit**

```bash
git add crates/nh-nixos/src/error_clarify.rs
git commit -m "error_clarify: clarify unknown NixOS option"
```

---

### Task 2: `clarify_impure_path` (pure)

**Files:** Modify `crates/nh-nixos/src/error_clarify.rs`
**Interfaces:** Produces `fn clarify_impure_path(text: &str) -> Option<String>`. Consumes `strip_ansi`.

- [ ] **Step 1: Write the failing test** — add to `mod tests`:

```rust
  #[test]
  fn clarify_impure_path_names_path_and_steers_off_impure() {
    let text = "access to absolute path '/etc/hostname' is forbidden in pure \
                evaluation mode (use '--impure' to override)";
    let block = clarify_impure_path(text).expect("should recognize impure path");
    assert!(block.contains("/etc/hostname"));
    assert!(block.contains("git add"), "steer to git add");
    assert!(block.contains("--impure"), "must mention --impure (to say: don't use it)");
    assert!(block.to_lowercase().contains("pas") || block.to_lowercase().contains("n'utilise"),
      "must steer AWAY from --impure:\n{block}");
    assert!(!block.contains("command.rs"));
  }

  #[test]
  fn clarify_impure_path_none_on_unrelated() {
    assert!(clarify_impure_path("error: something else").is_none());
  }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nh-nixos --lib error_clarify::tests::clarify_impure_path_names_path_and_steers_off_impure`
Expected: FAIL — function not found.

- [ ] **Step 3: Write minimal implementation** — add above `mod tests`:

```rust
/// Explain a pure-eval impure-path access and steer to the right fix
/// (move the file into the repo + git add) — never `--impure`.
pub(crate) fn clarify_impure_path(text: &str) -> Option<String> {
  use std::fmt::Write;
  let text = strip_ansi(text);
  if !text.contains("forbidden in pure evaluation mode") {
    return None;
  }
  let path = text
    .split_once("access to absolute path '")
    .and_then(|(_, r)| r.split_once('\'').map(|(p, _)| p))?;
  let mut out = String::new();
  let _ = writeln!(
    out,
    "✗ Accès à un fichier hors du flake : « {path} » (évaluation pure)."
  );
  let _ = writeln!(
    out,
    "  En flake, seuls les fichiers trackés par git dans le repo sont visibles."
  );
  let _ = writeln!(
    out,
    "  → si ce fichier fait partie de ta config, déplace-le dans le repo + git add."
  );
  let _ = write!(
    out,
    "  (N'utilise PAS --impure : ça masque le problème au lieu de le régler.)"
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
git commit -m "error_clarify: clarify impure-path-in-pure-eval (steer off --impure)"
```

---

### Task 3: Wire the two clarifiers into `try_clarify_with`

**Files:** Modify `crates/nh-nixos/src/error_clarify.rs`
**Interfaces:** Consumes `clarify_option_does_not_exist`, `clarify_impure_path` (Tasks 1-2).

- [ ] **Step 1: Write the failing test** — add to `mod tests`:

```rust
  #[test]
  fn try_clarify_routes_to_v2_2_classes() {
    use color_eyre::eyre::eyre;
    let probe = FakeProbe { failed: vec![], cause: None };

    let unknown = eyre!(
      "{}\nThe option `services.foo' does not exist. Definition values:\n- In `<unknown-file>': {{ enable = true; }}",
      nh_core::NIX_BUILD_ERROR_MARKER
    );
    let out = try_clarify_with(&unknown, &probe).expect("unknown option clarified");
    assert!(out.contains("Option inconnue"), "specific block:\n{out}");

    let impure = eyre!(
      "{}\naccess to absolute path '/etc/hostname' is forbidden in pure evaluation mode (use '--impure' to override)",
      nh_core::NIX_BUILD_ERROR_MARKER
    );
    let out = try_clarify_with(&impure, &probe).expect("impure path clarified");
    assert!(out.contains("hors du flake") && out.contains("--impure"), "specific block:\n{out}");
  }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p nh-nixos --lib error_clarify::tests::try_clarify_routes_to_v2_2_classes`
Expected: FAIL — the generic fallback (or `None`) is returned, not the new blocks.

- [ ] **Step 3: Write minimal implementation** — in `try_clarify_with`'s nix-build branch, the clarifier chain currently is:

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
Append the two new clarifiers to the chain (after `clarify_failed_assertions`, before `parse_nix_failures`):

```rust
    if let Some(b) = clarify_option_does_not_exist(text) {
      return Some(b);
    }
    if let Some(b) = clarify_impure_path(text) {
      return Some(b);
    }
```

- [ ] **Step 4: Run tests + gate**

Run: `cargo test -p nh-nixos --lib error_clarify::` then `./scripts/check.sh --fast`
Expected: PASS / GREEN (all prior tests + the new routing test).

- [ ] **Step 5: Commit**

```bash
git add crates/nh-nixos/src/error_clarify.rs
git commit -m "error_clarify: route unknown-option + impure-path before generic fallback"
```

---

### Task 4: Version bump, full gate, ratchet

**Files:** Modify `Cargo.toml`, `.tripwire-testcount`

- [ ] **Step 1: Bump the cheni layer** — edit `Cargo.toml`:

```toml
version = "4.4.1+cheni.0.3.2"
```

- [ ] **Step 2: Full gate**

Run: `./scripts/check.sh`
Expected: GREEN (fast + clippy + build); Cargo.lock updates cleanly; clippy clean on new code.

- [ ] **Step 3: Update the ratchet** — `check.sh` bumps `.tripwire-testcount` (6 new tests). Confirm and stage it.

Run: `git status --porcelain .tripwire-testcount`
Expected: modified (higher count).

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml Cargo.lock .tripwire-testcount
git commit -m "release: cheni-layer 0.3.2 — nix error comprehension (unknown option, impure path)"
```

---

## Self-Review

**Spec coverage:**
- Unknown-option renderer (checklist) → Task 1. ✓
- Impure-path renderer (steer off --impure) → Task 2. ✓
- Wired after the existing clarifiers, before generic fallback → Task 3. ✓
- Reuse `strip_ansi`, no upstream churn → Tasks 1-2; only error_clarify + Cargo.toml. ✓
- Never suggest --impure → Task 2 block says "N'utilise PAS --impure". ✓
- Parallel-safe fixture tests, merge-safe recognizers frozen by positive tests → Tasks 1-3. ✓
- Version 0.3.2 → Task 4. ✓
- Dropped classes (renamed/removed/untracked-git) NOT implemented. ✓

**Placeholder scan:** No TBD/TODO; full code in every step.

**Type consistency:** `clarify_option_does_not_exist`, `clarify_impure_path` — both `(&str) -> Option<String>`, consistent Tasks 1-3; both consume `strip_ansi`. ✓

**Recognizer disjointness note:** `clarify_option_does_not_exist` requires BOTH `"does not exist"` AND `"The option `"` so it does not steal the conflicting-options case (which has `"has conflicting definition values:"`, not `"does not exist"`). Negative test covers it.
