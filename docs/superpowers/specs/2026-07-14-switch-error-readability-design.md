# Design — Switch/activation error readability (v1)

**Date**: 2026-07-14
**Status**: approved (brainstorming), pending implementation plan
**Cheni layer**: new feature → bump cheni-layer version on ship

## Problem

`nh os switch/boot/test` presents activation failures in a way that buries the
actionable signal. Concrete captured specimen:

```
Error:
   0: Activation (test) failed
   1: Activating configuration (exit status ExitStatus(Exited(4)))
      stderr:
      Checking switch inhibitors... done
      stopping the following units: avahi-daemon.service, ... (dozens of lines)
      ... 40 lines of routine systemd churn ...
      warning: the following units failed: flatpak-setup.service
Location:
   crates/nh-core/src/command.rs:907
```

Three readability defects:
1. `Location: crates/nh-core/src/command.rs:907` surfaces nh's OWN source
   location (color_eyre capture site) as if it were where the problem is.
   Useless to the operator.
2. `exit status Exited(4)` is shown raw with no meaning. Exit 4 from
   `switch-to-configuration` means "activation succeeded, but some units failed
   to (re)start" — semantically NOT "activation failed".
3. The one actionable line (`warning: the following units failed:
   flatpak-setup.service`) is buried in routine unit churn, and its real cause
   (`Could not resolve hostname` → network) is only visible via a manual
   `journalctl -u flatpak-setup.service`.

In the specimen the switch actually **succeeded** (system activated, generation
set as boot default); only one network-dependent unit failed.

This is a re-visit of the wrapper-era `error_clarification` idea, now feasible
because the fork owns the nh code.

## Scope

**v1 = switch/activation failures only** (`nh os switch/boot/test`).
Build/eval errors (cryptic Nix traces) are explicitly **out of scope for v1**
(v2), but the architecture is designed to extend to them.

## Behavior decisions

- **Classification**: keep a **non-zero exit** (a failed unit is worth
  signalling, and scripts expecting non-zero still work), BUT the message must
  make clear the switch was *applied* (generation active) and merely list the
  units that failed + how to investigate. Never read as "nothing happened /
  system broken".
- **Journal enrichment**: for each failed unit, auto-run `journalctl -u <unit>`
  and extract the last significant error line, shown inline. Graceful fallback
  to just the `→ journalctl -u <unit>` hint if the journal is unreadable
  (permissions).
- **color_eyre `Location:`**: suppressed for recognized activation errors (we
  render our own block instead of the default report).

## Architecture

Single cheni-spec module: `crates/nh-nixos/src/error_clarify.rs`, declared in
`crates/nh-nixos/src/lib.rs`.

- `pub fn try_clarify(err: &color_eyre::eyre::Report) -> Option<String>` — entry
  point. Recognizes an activation failure, returns the rendered block, else
  `None`.
- Pure helpers: `classify_exit_code`, `render_block`, `recognize(msg) -> bool`.
- `trait SystemdProbe { fn failed_units(&self) -> Vec<String>;
  fn unit_last_error(&self, unit: &str) -> Option<String>; }`
  with a real impl (shells `systemctl --failed`, `journalctl -u <unit>`) and a
  fixture impl for tests. **All rendering/classification logic is pure and
  tested; every shell-out sits behind the trait.**

### The single upstream touch

`crates/nh/src/main.rs` error arm, ~5 lines:

```rust
if let Some(block) = nh_nixos::error_clarify::try_clarify(&report) {
    eprintln!("{block}");
    std::process::exit(1);
}
// else: existing color_eyre default report, unchanged
```

No other upstream file is modified.

## Data flow

1. `nh os switch` fails deep in the upstream rebuild path → `eyre::Report`
   bubbles up to `main.rs`.
2. `main.rs` hook calls `try_clarify(&report)`.
3. `try_clarify`:
   a. `recognize()` the report message as an activation failure (string match
      on the activation `msg` + `exit status` shape).
   b. `SystemdProbe::failed_units()` → currently-failed units.
   c. per unit: `SystemdProbe::unit_last_error()` → cause line (or None).
   d. `classify_exit_code()` → human meaning of the switch-to-configuration
      code.
   e. `render_block()` → final string.
4. Recognized → print block, exit non-zero, suppress default report.
   Not recognized → `None` → default color_eyre report unchanged.

## Rendered output (target)

```
⚠ Switch appliqué — la génération est active.
  Mais 1 service a raté son démarrage :
    flatpak-setup.service
      cause : Can't load uri … [6] Could not resolve hostname (réseau ?)
      → journalctl -u flatpak-setup.service
  (exit 4 de switch-to-configuration = activé, mais des units ont raté)
```

## Key realization — the churn is already streamed

In `show_output = true` mode the 40 lines of systemd churn are streamed **live**
during the switch, so they are already on screen before the error. We do NOT
buffer/suppress them in v1 (that would be a large change). We fix the **final
message** — the last thing printed, the part that misleads — by replacing the
color_eyre report with the clarified block. Honest and sufficient.

## Failed-unit sourcing (v1 limitation, accepted)

Sourced via `systemctl --failed` after the failure (works without sudo,
verified). Limitation: may include a pre-existing failure unrelated to THIS
switch. Mitigation: the block says "units actuellement en échec" rather than
asserting exact causality. Surgical precision (parsing the authoritative
`the following units failed: X` line) is approach B — deferred to a later
iteration.

## Testing & parallel-safety

- Inline `mod tests` (fork convention), fully parallel-safe: no env/CWD
  mutation, no shared paths, no real `systemctl`/`journalctl` (fixture
  `SystemdProbe`), no network.
- Cases: `classify_exit_code` table; `render_block` from fixture unit lists
  (0 units, 1 unit with cause, N units, unit with no journal → fallback arrow);
  `recognize()` matches the real `"Activation … (exit status Exited(4))"`
  format and rejects unrelated errors.

## Risks / watch points

- **Recognizer brittleness (merge)**: `recognize()` matches a string produced by
  upstream code (the activation `msg`). A `recognize()` test freezes the
  expected format → turns red at the next nh merge if upstream changes the
  wording. Documented; that red is the intended signal.
- **Exit-code table**: the exact `switch-to-configuration` exit-code → meaning
  mapping is pinned from the nixpkgs source during implementation (known anchor:
  4 = activated, some units failed). Not guessed.

## Out of scope (future)

- v2: build/Nix-eval error clarification (second parser).
- Approach B: capture-and-parse the switch output for exact per-switch units.
- Collapsing/suppressing the live systemd churn.
- Global color_eyre `Location:` suppression across all nh commands.
