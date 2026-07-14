# Nix error taxonomy — design input for error_clarify v2 (build/eval)

Produced 2026-07-14 (nix-rebuild-debugger agent), every example **verified live
on `nix 2.34.8`** (flakes, NixOS + home-manager modules), not from memory —
except §2.7 (disk full), flagged as needing a real repro before implementing.

v1 (shipped, cheni-layer 0.2.0) clarifies `switch-to-configuration` **activation**
failures. v2 extends to **nix eval + build** failures — the class the user
actually hits and can't read.

## Key architectural finding (the crux of v2)

nh runs `nix build --log-format internal-json --verbose | nom`. That
`internal-json` stream is NDJSON lines prefixed `@nix `, and it carries:
- a final `{"action":"msg","level":0,"raw_msg":"…"}` event = the **full,
  structured nix error message**;
- per-derivation `{"action":…,"type":101,…}` events = the builder's **last log
  lines**.

**nh already pipes this stream but never parses it** — on nix failure it just
`bail!(ExitError(exit_status))` (`crates/nh-core/src/command.rs` ~1004-1062),
so nh's own final message is content-free ("Command exited with status …") and
color_eyre stamps a misleading `Location:`.

⇒ **v2 anchor: tee the internal-json stream.** Forward one copy to nom (display
unchanged), parse the other for the structured root cause, print a clarified
block last (after nom’s scroll), drop the color_eyre `Location:`. This is
strictly better than regex-on-ANSI for anything via `nix build`. Eval errors
happen *before* the build (nom not involved) → plain nix stderr, text-parsed.

## MVP v2 — highest readability/effort payoff (in order)

1. **§2.2 Dependency build failure** — most frequent on real `nh os switch`
   (a deep pkg in the closure breaks). Nix prints the real leaf failure, then a
   `Reason: N dependencies failed.` block per ancestor; if the leaf scrolls off,
   the user only sees the useless top-level block. Fix = keep the leaf blocks,
   collapse the `N dependencies failed` noise. Mechanical.
2. **§2.1 Builder failed / non-zero exit** — prerequisite parsing for #1, and
   the case in the brief (`command.rs:1032` hides everything). Extract drv +
   `Last N log lines` + `nix log <drv>` (all in the JSON `raw_msg`).
3. **§1.7 Conflicting option definitions** — nix already gives option + both
   files + both values; extraction is a trivial line split. High gain, tiny effort.
4. **§1.6 Failed assertions** — isolate the `Failed assertions:` block from the
   ~10 frames of internal module plumbing; put it first. Very stable signature,
   biggest perceived win (scariest-looking trace).
5. **§2.4 Hash mismatch** — trivial `specified`/`got` extraction; generate the
   concrete "replace with this hash" action nix never states.

## Full class catalog

### Eval-time (plain nix stderr; nom not involved)
| # | Class | Detect signature (stable) | Extract | Render |
|---|---|---|---|---|
| 1.1 | Infinite recursion | `error: infinite recursion encountered` | first `at file:line` (cycle site), dedup repeated frames | cycle site + "check `with self;`/`rec` self-ref"; collapse "repeated ×N" |
| 1.2 | Missing attribute | `error: attribute '(.+)' missing` | attr, `file:line`, native `Did you mean X?` | attr + nix's own suggestion (best-effort) |
| 1.3 | Undefined variable | `error: undefined variable '(.+)'` | var, `file:line` | as-is; optional static nixpkgs-rename table |
| 1.4 | Type mismatch (expr) | `error: (cannot .+|value is .+ while a .+ was expected)` | message verbatim + enclosing `while evaluating the option 'X'` | keep nix msg, add "option: X" context |
| 1.5 | Syntax error | `error: syntax error, .+` | `file:line` + Bison msg verbatim | keep; map common Bison phrasings (unclosed `{`) |
| 1.6 | **Failed assertions** | `error:\nFailed assertions:\n(- .+)+` | the `- …` lines (no file loc from nix) | show block first, drop plumbing stack; static "assertion text → likely option" table for common cases |
| 1.7 | **Conflicting option defs** | `error: The option \`(.+)' has conflicting definition values:` + `- In \`…': …` | option, list of (file, value) | "option X conflicts: fileA=valA, fileB=valB; add mkForce/mkDefault" |
| 1.8 | Option type mismatch | `error: A definition for option \`(.+)' is not of type \`(.+)'` | option, type (verbatim), file, value | keep type verbatim, point to `nixos-option X` |
| 1.9 | Option does not exist / renamed / removed | 3 sigs: `does not exist\.` / `has been renamed to \`(.+)'` / `has been removed` | option + file; or old→new / removal msg | nu case → checklist (typo/import/renamed after bump); renamed/removed → show nixpkgs msg verbatim (already actionable) |
| 1.10 | Restricted eval / impure path | `access to absolute path '(.+)' is forbidden in pure evaluation mode` / `Path '(.+)' … is not tracked by Git\.` | path (+ builtin) / path + ready `git add` cmd | **never suggest `--impure`**; "move file into repo + git add"; untracked variant already perfect |
| 1.11 | IFD build failure | co-occurrence of `while evaluating` + `error: builder for '.+' failed` | the nested `.drv` | "this is a build failure during eval (IFD) — real problem in `nix log <nested-drv>`, not your flake" |

### Build-time (via internal-json stream — tee & parse `raw_msg`)
| # | Class | Detect signature | Extract | Render |
|---|---|---|---|---|
| 2.1 | **Builder failed** | `Cannot build '(.+)'\.\s*Reason: (.+)\.` | drv, reason, `Last N log lines`, `nix log` cmd | print clarified last (unscrolled), drop Location |
| 2.2 | **Dep build failure** | all `Cannot build` blocks; filter `Reason: N dependenc(y/ies) failed\.` | the leaf blocks only (non-"N failed") | "N leaves failed. root cause #1: name — last log lines …"; collapse propagation |
| 2.3 | Output not produced | `builder for '(.+)' failed to produce output path for output '(.+)'` | drv, output | "builder exited 0 but wrote nothing to $out — check `set -eu` / writes to $out" |
| 2.4 | **Hash mismatch** | `hash mismatch in file downloaded from '(.+)':\n specified: (\S+)\n got: (\S+)` (+ NAR variant) | url, specified, got, `file:line` | "replace specified with sha256-<got>"; give the line |
| 2.5 | Flake attr missing | `flake '.+' does not provide attribute '(.+)'` | attr, tried paths | keep; if nixosConfigurations, list real hosts (extra query) |
| 2.6 | Flake output not a derivation | `while evaluating the flake output attribute '(.+)'` + `has 0 entries in its context` | the attr | "attr X is not a valid derivation (nix wants mkDerivation, got a string/value)" |
| 2.7 | Disk full | `No space left on device` (needs real repro to confirm) | presence anywhere / in last log lines | short-circuit 2.1: "disk full — `nh clean` / gc, not a config bug" |

## Out of scope for v2
- Activation failures (`switch-to-configuration` exit 4) — **v1** already.
- home-manager "file already exists, aborting activation" (backup collision) —
  runtime activation, extend v1 not v2.
- C stack overflow / evaluator segfault — rare; v3.

## Notes for implementation
- Prefer parsing the internal-json `raw_msg` (structured) over ANSI text for
  build classes; text-parse only eval classes (pre-build stderr).
- Every regex above is anchored to a stable nix message; the recognizer tests
  must freeze the expected forms so an upstream nix bump turns them red
  (same merge-watch discipline as v1's `ACTIVATION_MSG`).
- Signatures target nix ~2.34; re-verify on nix major bumps.
