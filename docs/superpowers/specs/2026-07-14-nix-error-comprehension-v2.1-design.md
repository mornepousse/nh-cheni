# Design — Nix error comprehension (v2.1): class-specific renderers

**Date**: 2026-07-14
**Status**: approved (brainstorming), pending implementation plan
**Cheni layer**: 0.3.0 → 0.3.1
**Builds on**: v2 ([`2026-07-14-nix-error-readability-v2-design.md`](./2026-07-14-nix-error-readability-v2-design.md)) — the tee + capture + `try_clarify` pipeline. Taxonomy: [`2026-07-14-nix-error-taxonomy-v2.md`](./2026-07-14-nix-error-taxonomy-v2.md).

## Problem — "readable" ≠ "comprehensible"

v2 surfaces the real nix error text cleanly (no `Location:`, no pile). But the
goal is that a user *understands* the error and knows what to do. The surfaced
nix text is still nix jargon:
- `The option 'X' has conflicting definition values:` — the user may not grasp
  that two of their modules set the same option, or which to change.
- `Failed assertions: - The fileSystems option does not specify your root file
  system.` — the user needs to know these are NixOS guardrails their config
  violated, and where to look.
- `hash mismatch … specified: … got: …` — nix never says the concrete fix
  ("put the got hash in your .nix").

**Key finding (verified live, nix 2.34.8):** the tee captures each of these as a
single clean `level:0` message event — the `while evaluating` pile is NOT in the
captured text (it's the separate trace). So the value of v2.1 is NOT
pile-stripping (v2 already gives the clean message); it is **adding the
explanation + concrete next action** nix omits.

## Scope

**v2.1 = the trio**, each rendered as an explanation-plus-action block:
1. **Conflicting option definitions** (§1.7)
2. **Failed NixOS assertions** (§1.6)
3. **Hash mismatch** (§2.4, fixed-output derivations)

**Out of scope**: option renamed/removed/does-not-exist (§1.9) — nix often
already gives the answer (`has been renamed to Y`) or a self-explanatory message,
so the comprehension-add is thin; deferred. Everything else in the taxonomy
stays backlog. No upstream changes (the tee/capture from v2 are reused as-is).

## Architecture

Pure additions to `crates/nh-nixos/src/error_clarify.rs` (cheni). **Zero upstream
churn.** In `try_clarify_with`'s existing nix-build branch, after stripping the
marker and BEFORE the class-agnostic `render_nix_block` fallback, try the three
specific recognizers on the captured text; the first that matches renders its
dedicated block:

```
try_clarify_with:
  … activation branch (v1) …
  if recognize_nix_build(report):
    text = <after marker>
    if let Some(b) = clarify_conflicting_options(text) { return Some(b) }
    if let Some(b) = clarify_failed_assertions(text)   { return Some(b) }
    if let Some(b) = clarify_hash_mismatch(text)       { return Some(b) }
    failures = parse_nix_failures(text)
    if !failures.is_empty() { return Some(render_nix_block(failures)) }  # v2 fallback
  None
```

Each `clarify_*(text) -> Option<String>` is a pure function: recognize (return
`None` if not this class), extract the structured bits, render the block.
Ordering: hash / conflict / assertions are mutually exclusive in practice
(distinct signatures), so order is not sensitive, but specific-before-generic is
the rule. Recognizers key on stable nix message substrings, frozen by tests
(merge-watch discipline).

## The three blocks (target rendering)

**1. Conflicting options** — recognize `has conflicting definition values:`; extract
the option name and the `- In '<file>': <value>` lines:
```
⚠ Conflit de configuration — l'option « <option> » est définie à plusieurs
  endroits avec des valeurs différentes :
    <fileA>  → <valA>
    <fileB>  → <valB>
  Nix ne peut pas choisir. → garde une seule définition, ou impose la gagnante
  avec lib.mkForce (ou baisse la perdante avec lib.mkDefault).
```

**2. Failed assertions** — recognize `Failed assertions:`; extract the `- ` lines:
```
✗ Ta config viole des garde-fous NixOS (assertions). Corrige :
    • <assertion 1>
    • <assertion 2>
  Chaque ligne est une règle de cohérence non respectée — cherche l'option
  correspondante dans tes modules récemment édités.
```

**3. Hash mismatch** — recognize `hash mismatch`; extract `specified:` and `got:`
(and the drv/url if present):
```
✗ Hash incorrect pour une source à contenu fixe. Nix a obtenu un contenu
  différent de l'attendu :
    attendu : <specified>
    obtenu  : <got>
  → remplace « attendu » par <got> dans le .nix qui déclare cette source
    (fetchurl/fetchFromGitHub/…). Si tu n'attendais PAS de changement,
    méfie-toi (source altérée).
```

## Fixtures (real, captured on nix 2.34.8 via internal-json — use verbatim in tests)
- Conflict: `The option \`foo' has conflicting definition values:\n- In \`<file-a>': "B"\n- In \`<file-b>': "A"\nUse \`lib.mkForce value' or \`lib.mkDefault value' to change the priority …`
- Assertions: `\nFailed assertions:\n- cheni assert fail exemple\n- The 'fileSystems' option does not specify your root file system.`
- Hash: `hash mismatch in fixed-output derivation '/nix/store/…-boom-hash.drv':\n  specified: sha256-AAAA…\n  got: sha256-<real>`
(ANSI is stripped by the existing `strip_ansi` before matching — the captured
`msg`/`raw_msg` may carry ANSI.)

## Testing & parallel-safety
- Inline `mod tests`, pure, parallel-safe (fixtures only, no real nix): for each
  `clarify_*` — a positive test (real fixture → block contains the option/files/
  values / the assertion lines / the got-hash-in-the-action) and a negative test
  (unrelated text → `None`). Plus a `try_clarify_with` integration test per class
  (report with marker + fixture → the specific block, not the generic one).
- Recognizer strings frozen by the positive tests → red on an upstream nix
  wording change.

## Risks / watch points
- **Recognizer brittleness (merge)**: keys on nix message substrings
  (`has conflicting definition values:`, `Failed assertions:`, `hash mismatch`).
  Frozen by tests. These are stable core-`lib/modules.nix` / nix-store messages.
- **ANSI in `msg`**: the fallback `msg` field carries ANSI; `strip_ansi` runs
  first (reuse v2's function) so extraction sees clean text.
- **Multiple `- In` / assertion lines**: parse all, render all.

## Out of scope (backlog)
Option renamed/removed/does-not-exist (§1.9), restricted-eval/untracked-git
(§1.10), the non-nom path, auxiliary text-bubbled nix errors. All in the taxonomy.
