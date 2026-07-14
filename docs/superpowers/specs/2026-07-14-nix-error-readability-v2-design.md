# Design — Nix build/eval error readability (v2)

**Date**: 2026-07-14
**Status**: approved (brainstorming), pending implementation plan
**Cheni layer**: new capability → bump cheni-layer 0.2.0 → 0.3.0 on ship
**Design input**: [`2026-07-14-nix-error-taxonomy-v2.md`](./2026-07-14-nix-error-taxonomy-v2.md)
**Builds on**: v1 activation clarifier ([`2026-07-14-switch-error-readability-design.md`](./2026-07-14-switch-error-readability-design.md))

## Problem

`nh os build/switch` runs `nix build --log-format internal-json --verbose | nom`
(nix-output-monitor). On nix failure, `crates/nh-core/src/command.rs`
`Build::run()` does `bail!(ExitError(exit_status))` — nh's own error is
**content-free** ("Command exited with status …"); the real nix error (eval
trace or build failure) was streamed to `nom` and then discarded. color_eyre
stamps a misleading `Location: crates/nh-core/src/command.rs:…`. The operator is
left with a scary but empty final error.

Verified live: `nh os build` on a bad flake printed
`Error: … 1: No such file or directory … Location: crates/nh-core/src/command.rs:1032`
— no nix message at all.

v1 clarifies `switch-to-configuration` **activation** failures. v2 clarifies the
**nix eval + build** failures — the class the user actually hits and can't read.

## Key finding (drives the architecture)

`nix build --log-format internal-json` emits NDJSON lines prefixed `@nix `. This
stream carries **both eval and build** diagnostics (eval errors during
`nix build` are in the stream too — they are NOT plain stderr). The final
`{"action":"msg","level":0,"raw_msg":"…"}` event holds the full structured nix
error message; per-derivation `type:101` events hold the builder's last log
lines. **nh already pipes this stream to nom but never reads it.**

⇒ The only way to recover the real message is to **tee the stream** inside
`Build::run`.

## Scope

**v2 MVP (lean):**
- **Class-agnostic core**: capture the real nix error from the JSON stream and
  render it cleanly — drop the color_eyre `Location:`, print it last (after
  nom's scroll), always show the actual `raw_msg` instead of "exit status N".
  This alone covers eval errors and a single builder failure.
- **§2.2 dependency-build-failure collapse**: the one class-specific bit that's
  indispensable — filter out the propagation blocks (`Reason: N dependencies
  failed.`) and keep the real leaf failures, so the clean message isn't the
  useless top-level "1 dependency failed".

**Deferred to v2.1** (explicitly out of scope): class-specific renderers for
hash-mismatch (§2.4), failed-assertions isolation (§1.6), conflicting-option
extraction (§1.7), and the **non-nom path** (nom disabled). The taxonomy doc
lists them; v2 ships the 80% win small.

**Out of scope**: activation failures (v1), home-manager backup collisions,
evaluator stack overflow.

## Architecture — crate split (dependency constraint resolved)

`command.rs` is in **nh-core**; `error_clarify` is in **nh-nixos**; and
**nh-nixos depends on nh-core, not the reverse** — so `command.rs` cannot call
`error_clarify`. The split follows from that:

1. **`crates/nh-core/src/command.rs` (upstream) — the tee, CAPTURE ONLY.**
   Restructure the nom branch of `Build::run`: spawn `nix` with `stdout = Pipe`,
   spawn `nom` with `stdin = Pipe`, and a thread that copies nix's stdout to
   nom's stdin (display unchanged) while collecting the error-relevant JSON
   events (the `level:0` `raw_msg` and per-drv last-log-line events) into a
   buffer. On nix exit ≠ 0, build the real error **text** from the buffer and
   `bail!` an error that carries it (instead of the content-free `ExitError`).
   **No interpretation here** — just capture the right events and put their text
   into the error. This is the only meaningful upstream change; contain it in a
   small helper (e.g. `fn tee_and_collect(nix_out, nom_in) -> Vec<String>`).
2. **`crates/nh-nixos/src/error_clarify.rs` (cheni) — all the smarts.** Extend
   the existing module: recognize a nix-build failure (from the captured text in
   the report), collapse `N dependencies failed` blocks, keep leaves, render the
   clean block. Pure functions, fixture-tested.
3. **`crates/nh/src/main.rs` (nh) — unchanged.** The v1 hook already routes
   top-level errors through `error_clarify::try_clarify`; it now also matches
   nix-build failures.

Communication nh-core → cheni is **via the error message text** (no new
cross-crate types, no dependency inversion): `command.rs` puts the real nix
error text into the eyre error; `error_clarify` parses it from the formatted
report (`format!("{err:#}")`), exactly as v1 does for activation.

## Data flow

1. `nh os build/switch` → `Build::run()` (nom branch).
2. Tee: nix `stdout=Pipe` → thread: copy → `nom.stdin`; collect error events → buffer.
3. Wait for nix (exit status of the nix process, not nom — already handled).
4. nix exit ≠ 0 → assemble real error text from buffer → `bail!(<error carrying text>)`.
5. Bubbles to `main.rs` → `try_clarify(&report)`.
6. `error_clarify`: recognize → collapse `N dependencies failed` → keep leaves →
   render clean block; print, exit non-zero, drop default report.
   Not recognized → default color_eyre report unchanged.

## Rendered output (target)

Dependency/build failure:
```
✗ Build nix échoué — 1 dérivation en échec (cause racine) :
    <name-or-drv>
      <dernières lignes de log significatives>
      → nix log <drv>
  (blocs intermédiaires « N dependencies failed » masqués)
```
Eval error (also captured via the stream): show the real nix `raw_msg` cleanly,
no `Location:`, printed last. Class-agnostic core = "surface the real message".

## Testing & parallel-safety

- **Pure functions** in `error_clarify`, fed **fixtures** (real internal-json
  error-event samples / the extracted text): dependency-collapse (multi-block
  input with `Reason: N dependencies failed.` → only the real leaves survive),
  clean rendering, recognizer (matches the captured nix-build error shape,
  rejects unrelated). Inline `mod tests`, fully parallel-safe: no env/CWD, no
  shared paths, no network, **no real nix**.
- **The tee in command.rs** (subprocess threads) is hard to unit-test; keep it
  thin (capture only) and validate with a **manual smoke** — a deliberately
  failing build (e.g. a flake attr referencing a broken derivation), like the
  v1 demo. All interpretation is pure and covered in cheni.

## Risks / watch points

- **Tee rewrite risk**: the current code uses `subprocess`'s `|` pipeline
  operator + `popen`. Teeing needs manual spawn (nix `stdout=Pipe`, nom
  `stdin=Pipe`) + a copy thread. This is the riskiest, churniest part; it stays
  contained to the nom branch of `Build::run`. The copy thread must not
  deadlock (drain nix stdout continuously; don't block on a full nom stdin) and
  must forward bytes verbatim so nom's rendering is byte-identical to today.
- **Recognizer brittleness (merge)**: keys on the nix JSON `raw_msg` shapes
  (`Cannot build '…'. Reason: …`, `N dependencies failed`). A recognizer test
  freezes these forms → turns red on an upstream nix wording change
  (same discipline as v1's shared `ACTIVATION_MSG` constant).
- **nom byte-fidelity**: the tee must not alter what nom sees, or the live build
  UI regresses. Smoke must confirm nom output is unchanged on a successful build.

## Out of scope (future — v2.1+)
- Class-specific renderers: hash-mismatch, failed-assertions, conflicting
  options, option-does-not-exist/renamed, restricted-eval (see taxonomy doc).
- The non-nom path (nom disabled) capture.
- Auxiliary nix invocations that bubble as plain text (flake-attr-missing, path
  resolution) — could reuse the v1 text-parse hook.
