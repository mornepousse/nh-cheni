# cheni diagnose — pattern catalogue

`cheni diagnose` scans a rebuild log (from a file or stdin) and prints
an actionable `what / why / fix` block for every pattern it recognises.
Two invocation shapes:

```
$ sudo nixos-rebuild switch --upgrade 2>&1 | tee /tmp/build.log
$ cheni diagnose /tmp/build.log          # after the fact
$ <cmd> 2>&1 | cheni diagnose            # via pipe
```

The same pattern library runs automatically as a postscript on a
failed `cheni upgrade` / `cheni self-update` — no extra invocation.

## How matching works

Simple case-insensitive substring match against the whole log. No
regex, no context windows. A pattern fires as soon as its matcher
string appears anywhere in the text. False positives are possible
by design — the library errs on the side of surfacing a hint that
might help rather than staying silent.

## Current catalogue — 33 patterns

### Build runtime

| Matcher | Title |
|---|---|
| `aes_generic` | kernel module `aes_generic` not found |
| `hash mismatch in fixed-output derivation` | fixed-output hash mismatch |
| `No space left on device` | disk full during build |
| `exit code 137` | build killed by the OOM killer (exit 137 = SIGKILL) |
| `failed to install the bootloader` | bootloader install failed — system may not boot |
| `refusing to overwrite` | activation refused to overwrite an existing file |
| `dependencies couldn't be built` | upstream dependency failed — real error is earlier in the log |
| `access to network is forbidden` | build tried to reach the network inside the sandbox |
| `Too many open files` | process hit the file-descriptor limit |

### Network / fetch

| Matcher | Title |
|---|---|
| `SSL peer certificate` | TLS failure fetching from a substituter |
| `API rate limit exceeded` | GitHub API rate limit hit during flake fetch |
| `Temporary failure in name resolution` | DNS resolution failure inside a build or fetch |
| `Authentication failed for` | private repository fetch without credentials |

### Flake structure

| Matcher | Title |
|---|---|
| `does not provide attribute` | flake attribute missing |
| `does not exist in the flake` | file referenced but not tracked by git |
| `is forbidden in pure eval mode` | absolute path access in pure eval mode |
| `cannot parse flake reference` | malformed flake URL |
| `NAR hash mismatch` | flake input narHash in flake.lock is stale |

### Eval-time

| Matcher | Title |
|---|---|
| `infinite recursion encountered` | infinite recursion in the Nix expression |
| `undefined variable` | undefined variable in the Nix expression |
| `cannot coerce` | type mismatch — usually function passed where string expected |
| `cached failure of attribute` | stale flake eval-cache masking the real error |
| `syntax error, unexpected` | Nix syntax error |
| `is not of type` | NixOS option value has the wrong type |
| `cannot allocate memory` | memory exhausted during evaluation (not a build) |
| `is used but not defined` | option referenced without the declaring module loaded |

### Package policy

| Matcher | Title |
|---|---|
| `has an unfree license` | unfree package refused |
| `is marked as broken` | broken package |
| `collision between` | package collision (two packages provide the same file) |

### Activation / configuration

| Matcher | Title |
|---|---|
| `is in the way of` | home-manager refuses to overwrite an existing file |
| `Failed to start` | systemd service failed to start after activation |
| `experimental Nix feature` | experimental feature (flakes / nix-command) not enabled |
| `untrusted substituter` | binary cache rejected because it's not trusted |

## Adding a pattern

The library is a single `const KNOWN_FINDINGS: &[Finding]` array in
[`src/cmd/diagnose.rs`](src/cmd/diagnose.rs). Each entry has four
fields:

```rust
Finding {
    matcher: "…",      // case-insensitive substring to look for
    title: "…",        // one-line headline
    explanation: "…",  // why this error happens (1–2 sentences)
    action: "…",       // what to do about it (concrete command(s))
}
```

Contributing flow:

1. Pick a matcher substring that's specific enough to avoid obvious
   false positives (e.g. `exit code 137` not `137`) but generic
   enough to survive minor wording drift across Nix releases.
2. Write an `explanation` that describes the root cause in plain
   language — two sentences max.
3. Write an `action` that gives concrete commands and configuration
   snippets. Paths, flags, option names.
4. Add one sibling test in
   [`src/cmd/tests/diagnose.rs`](src/cmd/tests/diagnose.rs) with a
   realistic snippet of the error output. Assert the hit count and a
   substring of the title.
5. Commit: `diagnose(add): <short-pattern-name>`.

## Known limitations

- **No regex, no structured parsing.** A pattern that needs to
  match on surrounding context or extract a specific value can't be
  expressed in the current model.
- **Wording drift.** Nix / nixpkgs can rephrase an error at any
  release. A pattern that worked last month may silently stop
  firing. No automated way to detect this today.
- **Some errors are too generic to match well.** `error: attribute
  'foo' missing` has the word `missing` in too many contexts to
  safely match without false positives, for instance.
- **English-only.** If Nix ever localises error messages the
  library breaks — unlikely in practice, but worth noting.

## Strategic position

This library is cheni's answer to "desktop-user NixOS error messages
are cryptic". We grow the catalogue one pattern at a time as real
logs cross our path, rather than investing in a standalone tool or
upstreaming to `nh`. Each pattern added is a measurable improvement —
no roadmap, no promises, just commits stacking up.

If you hit a cryptic NixOS error and cheni didn't catch it, please
open an issue with the log snippet — that's exactly how this
catalogue grows.
