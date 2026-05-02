# nh-cheni

Personal fork of [nh](https://github.com/viperML/nh) (Yet Another Nix
Helper) by harrael. Adds a layer of NixOS-management subcommands for
package pins, freezes, version checks, audit timeline, and so on,
while preserving the user-facing `nh` command from upstream.

> **Personal use only.** This fork is mono-user. Issues and pull
> requests are not accepted; please use [upstream nh](https://github.com/viperML/nh)
> if you want to contribute or report bugs there.

The fork lives at `gitlab.com/harrael/nh-cheni`. The previous
**wrapper-era cheni** (a thin Rust CLI that shelled out to nh) is
preserved at the tag `wrapper-archive-v0.8.5` and remains buildable
via `nix build gitlab:harrael/nh-cheni/wrapper-archive-v0.8.5`.

---

## Table of contents

1. [Install](#install)
2. [Usage — the cheni subcommands](#usage--the-cheni-subcommands)
3. [Architecture overview](#architecture-overview)
4. [Navigating the code](#navigating-the-code)
5. [Adding a new cheni subcommand](#adding-a-new-cheni-subcommand)
6. [Workflows](#workflows)
7. [Rust mini-glossary for non-Rust devs](#rust-mini-glossary-for-non-rust-devs)
8. [Reading walkthrough — `pins.rs` line by line](#reading-walkthrough--pinsrs-line-by-line)
9. [Conventions](#conventions)
10. [License](#license)

---

## Install

In your NixOS flake:

```nix
{
  inputs.cheni.url = "gitlab:harrael/nh-cheni";
  inputs.cheni.inputs.nixpkgs.follows = "nixpkgs";

  outputs = { cheni, ... }: {
    nixosConfigurations.<host> = nixpkgs.lib.nixosSystem {
      modules = [
        {
          environment.systemPackages = [
            cheni.packages.x86_64-linux.default
          ];
        }
      ];
    };
  };
}
```

The installed binary is `nh` (so `nh os switch ...` keeps working
identically to upstream nh, with the cheni-extension subcommands
available alongside). The Nix-store path identifies the fork as
`nh-cheni-<version>`.

If you previously had `pkgs.nh` (upstream nh) in your
`environment.systemPackages`, **remove it** — the fork's binary is
the same name and they would conflict in your `$PATH`.

---

## Usage — the cheni subcommands

These are added on top of every `nh os` subcommand from upstream
nh. Run `nh os <subcommand> --help` for full options.

| Subcommand | What it does |
|---|---|
| `nh os pin <pkg>` | Routes `<pkg>` through the `nixpkgs-latest` overlay (your `flake.nix` must declare a `nixpkgs-latest` input). |
| `nh os pin` (no args) | Lists currently pinned packages. |
| `nh os unpin <pkg>` / `--all` | Removes pin(s). |
| `nh os freeze <pkg>` | Locks `<pkg>` at the *current* `nixpkgs` rev (auto-detected from `flake.lock`). Records `rev`, `narHash`, optional `--version` string. |
| `nh os freeze` (no args) | Lists currently frozen packages. |
| `nh os unfreeze <pkg>` / `--all` | Removes freeze(s). |
| `nh os timeline` | Shows recent cheni events (pin / unpin / freeze / unfreeze / self-update) in reverse-chronological order. |
| `nh os events` | Lists NixOS generations with timeline events grouped under each. Useful for "what changed around generation 142?" |
| `nh os check` | For each pin and freeze, queries upstream version via `nix eval` and flags obsolete ones (pin where `nixpkgs` caught up, freeze where `nixpkgs` is at-or-below the frozen version). |
| `nh os doctor` | Sanity checks: `nix` / `git` in PATH, `flake.nix` present, pins / freezes / timeline files readable, `flake.lock` age, version-cache state, active rebuild lock. |
| `nh os bug-report` | Self-contained markdown dump suitable for pasting into a GitLab issue. Includes version, OS, kernel, active pins / freezes, timeline tail. |
| `nh os self-update [--switch]` | Runs `nix flake update <input>` (default `cheni`) in your flake-dir, reports the diff, optionally chains into `nh os switch`. |

State files written by these subcommands:

| File | Owner | Format |
|---|---|---|
| `<flake-dir>/package-pins.json` | `pin` / `unpin` | JSON array of strings |
| `<flake-dir>/package-freezes.json` | `freeze` / `unfreeze` | JSON object `{ name → { rev, narHash, version, frozen_at, majorConstraint? } }` |
| `$XDG_CACHE_HOME/cheni/timeline.jsonl` | every modifying subcommand | JSONL, one event per line |
| `$XDG_CACHE_HOME/cheni/version-cache.json` | (infra, not yet wired to a consumer) | nested JSON `{ input → rev → attr → version }` |

All files are mode `0o600`; the cache directory is mode `0o700`.

---

## Architecture overview

This is a Cargo workspace inherited unchanged from upstream nh:

```
crates/
├── nh/         binary entry + clap top-level dispatch
├── nh-core/    exec layer, args, installable, update
├── nh-nixos/   NixOS rebuild, generations, rollback     ← cheni-spec modules live here
├── nh-clean/   GC
├── nh-darwin/  nix-darwin
├── nh-home/    home-manager
├── nh-remote/  remote rebuilds
└── nh-search/  search.nixos.org

xtask/          man-page + completions generation
```

The cheni-specific code is **all inside `crates/nh-nixos/`** — we
deliberately don't touch upstream nh elsewhere, so future
`git fetch upstream && git merge` operations have a small conflict
surface. The cheni-spec modules in `crates/nh-nixos/src/` are:

```
pins.rs           the `nh os pin` / `unpin` state file + commands
freezes.rs        same for `freeze` / `unfreeze`
timeline.rs       JSONL event log
events.rs         NixOS generation listing annotated with timeline
check.rs          obsolete pin / freeze detection via nix eval
doctor.rs         system sanity checks
bug_report.rs     markdown diagnostic dump
self_update.rs    nix flake update <cheni> [+ chained switch]
versioning.rs     parse / compare / is_prerelease (version helpers)
version_cache.rs  on-disk cache for nix eval results (infra)
cheni_meta.rs     read the option-B version components at runtime
cheni_util/       shared utilities (atomic, time, validation, flake)
   ├── atomic.rs       atomic file write (tmp + fsync + rename, 0o600, O_NOFOLLOW)
   ├── time.rs         RFC 3339 / ISO date helpers (no chrono dep)
   ├── validation.rs   package-name + git-rev + narHash validation
   └── flake.rs        read locked rev/narHash from flake.lock
```

And the cheni-spec **additions to upstream nh files** (kept tiny so
upstream merges stay friction-free):

| File | What we add |
|---|---|
| `crates/nh-nixos/src/args.rs` | `OsXxxArgs` structs and `OsSubcommand::Xxx` variants for every cheni subcommand, plus their `FeatureRequirements` arms |
| `crates/nh-nixos/src/nixos.rs` | One dispatch arm per subcommand: `OsSubcommand::Xxx(args) => args.run()` |
| `crates/nh-nixos/src/lib.rs` | `pub mod` declarations for the cheni-spec modules |
| `crates/nh/build.rs` | Decomposes the option-B workspace version into `CHENI_FULL_VERSION` for clap to display |
| `Cargo.toml` (workspace) | The option-B version string + serde / regex deps for cheni-spec modules |
| `package.nix`, `flake.nix` | `pname = "nh-cheni"`, `mainProgram = "nh"`, points to gitlab.com/harrael/nh-cheni |

Everything else under `crates/` is untouched upstream nh code.

---

## Navigating the code

A grep cheat-sheet for the most common questions:

**"Where is `<subcommand>` implemented?"**

```
crates/nh-nixos/src/<subcommand>.rs
```

(They're flat-named: `pin` is `pins.rs`, `freeze` is `freezes.rs`,
`bug-report` is `bug_report.rs`, `self-update` is `self_update.rs`.)

**"Where is `OsXxxArgs` defined?"**

```bash
grep -n "pub struct OsXxxArgs" crates/nh-nixos/src/args.rs
```

All cheni-extension args structs live there, after the `OsSubcommand`
enum. Same for the variants — search `OsSubcommand::Xxx` in
`crates/nh-nixos/src/args.rs` (definition) or `nixos.rs` (dispatch).

**"Where is the atomic write / date helper / validator?"**

`crates/nh-nixos/src/cheni_util/{atomic,time,validation,flake}.rs`.
Each submodule has its own short header comment explaining what it
covers and why it was extracted from the original duplicate copies.

**"Where does `nh --version` come from?"**

The chain is:

1. `Cargo.toml` workspace `version = "<nh-base>+cheni.<cheni-layer>"`
2. `crates/nh/build.rs` reads it and exports
   `CHENI_FULL_VERSION` env var at build time
3. `crates/nh-nixos/src/args.rs` (the `Main` struct) uses
   `version = env!("CHENI_FULL_VERSION")` in the `#[command(...)]`
   attribute
4. clap renders that string when you pass `--version`

**"What does `<some clippy warning>` mean?"**

Most pedantic warnings come from upstream nh's strict clippy config.
For cheni-spec files, the convention is "zero new warnings"; for
upstream nh files, leave them alone (touching them creates merge
friction).

---

## Adding a new cheni subcommand

The 5-step checklist. Replace `foo` with your subcommand name (e.g.
`trace`, `audit`, etc).

### Step 1 — Create the module

```
crates/nh-nixos/src/foo.rs
```

Skeleton:

```rust
//! `nh os foo` — <one-line description>.

use color_eyre::eyre::Result;
use crate::args::OsFooArgs;

impl OsFooArgs {
    /// Run `nh os foo`.
    ///
    /// # Errors
    ///
    /// Returns an error if ...
    pub fn run(self) -> Result<()> {
        // Orchestrator: stay short, delegate to named helpers.
        Ok(())
    }
}

#[cfg(test)]
#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
mod tests {
    use super::*;
    // ...
}
```

### Step 2 — Add the args struct + the subcommand variant

In `crates/nh-nixos/src/args.rs`, append:

```rust
#[derive(Debug, Args)]
pub struct OsFooArgs {
    // your CLI flags / args
}
```

…and add the variant to the `OsSubcommand` enum (also in args.rs):

```rust
/// <one-line description visible in `nh os --help`>. (cheni extension)
Foo(OsFooArgs),
```

…and add the feature-requirements arm in `OsArgs::get_feature_requirements`:

```rust
OsSubcommand::Foo(_) => Box::new(NoFeatures),
//                              ^^^^^^^^^^ or FlakeFeatures if you call nix
```

### Step 3 — Wire the dispatch

In `crates/nh-nixos/src/nixos.rs`, add one arm to the match in
`OsArgs::run`:

```rust
OsSubcommand::Foo(args) => args.run(),
```

### Step 4 — Declare the module

In `crates/nh-nixos/src/lib.rs`, add:

```rust
pub mod foo;
```

(keep the list alphabetical for friendly diffs).

### Step 5 — Tests

Inline `#[cfg(test)] mod tests { ... }` at the bottom of the module
file. **Not** sibling files — the fork uses inline tests for
consistency with upstream nh files in the same crate.

Write tests that:
- Are parallel-safe: no `set_var`, no `set_current_dir`, no shared
  paths. Use `tempfile::TempDir` for any per-test fixture.
- Cover happy path + missing/empty input + validation failure.
- Check on-disk file mode (Unix) when the module writes anything:
  `fs::metadata(&path).unwrap().permissions().mode() & 0o777 == 0o600`.

### Verification

```bash
cargo build --release
cargo test -p nh-nixos --lib foo
cargo clippy --all-targets
nix build .#nh-cheni
./result/bin/nh os foo --help    # must show your new subcommand
```

---

## Workflows

### Cutting a cheni-layer release

You added a new feature/fix on the cheni side. Bump the cheni-layer
version (NEVER both halves at once):

1. Edit `Cargo.toml`: `version = "<unchanged-base>+cheni.<new-layer>"`
   - Patch (e.g. `0.1.0 → 0.1.1`): polish, refactor, fix
   - Minor (e.g. `0.1.0 → 0.2.0`): new subcommand
   - Major (e.g. `0.1.0 → 1.0.0`): structural break, rare
2. `cargo build` (validates Cargo.lock)
3. Quality gate:
   ```
   cargo clippy --all-targets
   cargo test --workspace -- \
       --skip test_get_build_image_variants_expression \
       --skip test_get_build_image_variants_file \
       --skip test_get_build_image_variants_flake
   nix build .#nh-cheni
   ```
4. `git commit -am "release: v<full-version>"`
5. `git tag -a "v<full-version>" -m "release v<full-version>"`
6. `git push origin main && git push origin "v<full-version>"`
7. ```bash
   glab release create "v<full-version>" \
       -R harrael/nh-cheni \
       --name "v<full-version>" \
       --notes-file <changelog-section>
   ```

### Merging a new upstream nh release

```bash
git fetch upstream --tags
git diff --stat main...upstream/master    # preview the scope
git merge upstream/master --no-ff -m "Merge upstream nh <tag>"
```

Expected conflict surface:
- `crates/nh-nixos/src/args.rs` — our cheni-extension variants
  interleave with upstream's additions (additive resolution: keep
  both).
- `crates/nh-nixos/src/nixos.rs` — same for the dispatch arms.
- `Cargo.toml` workspace version — keep upstream's nh-base, our
  `+cheni.<layer>` suffix.

After resolving:

1. Bump the nh-base half to whatever upstream tag we merged
   (`git describe --tags upstream/master` strips its `v` prefix
   if present):
   ```toml
   version = "<new-nh-base>+cheni.<unchanged-cheni-layer>"
   ```
2. Quality gate (same commands as the release workflow).
3. Commit `release: bump nh-base to <new-nh-base> (post-upstream-merge)`.
4. `git push origin main`. **Don't tag a release here** — that's a
   separate decision (you might want to ship a fix on top before
   tagging).

### Rollback to wrapper-era cheni (emergency)

```bash
cd ~/cheni
git checkout wrapper-archive-v0.8.5
nix build
sudo cp -r result/* /run/current-system/sw/   # NOT recommended, see below
```

Better: in your `nixos-config/flake.nix`, swap the input URL to
pin the wrapper tag:

```nix
inputs.cheni.url = "gitlab:harrael/nh-cheni/wrapper-archive-v0.8.5";
```

Then `nix flake update cheni && nh os switch`. The wrapper-era
binary is `cheni` (not `nh`), so any system running the wrapper
needs `pkgs.nh` reinstated alongside.

---

## Rust mini-glossary for non-Rust devs

The minimum to read the cheni-spec modules without being lost. Not a
Rust tutorial — just anchors for things you'll see on every page.

### `Result<T, E>` and the `?` operator

```rust
fn read_pin() -> Result<Vec<String>> {
    let content = std::fs::read_to_string("path")?;  //  ?  unwraps Ok or returns Err
    Ok(parse(&content)?)
}
```

`Result<T, E>` is "either an `Ok(T)` value or an `Err(E)` error".
The `?` operator says "if this is an Err, return it from my function;
if it's Ok, give me the value inside". It's how Rust does
"propagate the error up" without exceptions.

In nh-cheni the error type is mostly `color_eyre::eyre::Result` (the
type alias drops the `, E` part). Errors carry context strings via
`.context("doing X")` / `.with_context(|| format!("..."))`.

### `Option<T>`

`Option<T>` is "either `Some(T)` or `None`". Used everywhere a value
might legitimately be absent — `freeze.major_constraint` is
`Option<u32>` because freezes without that constraint just don't
have it.

```rust
match freeze.major_constraint {
    Some(n) => println!("major-locked at {n}"),
    None => println!("strict freeze"),
}
```

`.unwrap_or(default)`, `.map(|x| ...)`, `.and_then(|x| ...)` are the
common combinators you'll see chained.

### `&str` vs `String`

- `String` owns the bytes (heap-allocated, can be mutated).
- `&str` is a borrowed view (a pointer + length, can't be modified
  through it).

Functions that don't need to mutate take `&str`; functions that need
to keep the value around take `String`. Most of the cheni-spec API
takes `&str` for inputs and returns `String` for outputs.

### `match`

Like a `switch` but exhaustive (the compiler refuses to compile
unless every variant is covered) and returns a value:

```rust
let label = match status {
    PinStatus::StillUseful => "useful",
    PinStatus::Obsolete => "obsolete",
    PinStatus::Unresolvable => "unresolvable",
};
```

### `mod` / `pub mod`

`mod foo;` says "look for the module `foo` in `foo.rs` or `foo/mod.rs`".
`pub mod` makes it visible outside the crate. The `lib.rs` file at
the top of `crates/nh-nixos/src/` is just a list of `pub mod` lines
— it's the table of contents.

### `#[cfg(test)]` and `#[derive(...)]`

Attributes (the `#[...]` syntax) modify what comes after. Common ones
in cheni-spec code:

- `#[derive(Debug, Clone, Serialize, Deserialize)]` — auto-implement
  these traits for the struct
- `#[cfg(test)]` — only compile this when running `cargo test`
- `#[serde(rename = "narHash")]` — serialize this field as `"narHash"`
  (camelCase) instead of `"nar_hash"` (Rust's snake_case)
- `#[allow(dead_code)]` — suppress the "unused" warning
- `#[expect(...)]` — like `allow` but errors if the warning ISN'T
  triggered (catches "this lint is no longer needed")

### clap derive macros

We use clap to parse the CLI. The pattern is:

```rust
#[derive(Args, Debug)]
pub struct OsPinArgs {
    /// Path to your NixOS flake. Resolved via $NH_FLAKE if absent.
    #[arg(long, value_name = "PATH")]
    pub flake_dir: Option<PathBuf>,

    /// Package names to pin. Run with no names to list current pins.
    pub names: Vec<String>,
}
```

- `#[derive(Args)]` makes the struct usable as a clap subcommand-args.
- The doc-comment above each field is what shows in `--help`.
- `#[arg(long, ...)]` declares a `--flag` style argument; positional
  args (no `#[arg]`) come from leftover argv.

### `Vec<T>` / `HashMap<K, V>` / `BTreeMap<K, V>`

The standard collection types:
- `Vec<T>` — a growable array
- `HashMap<K, V>` — a hash table (no guaranteed iteration order)
- `BTreeMap<K, V>` — a sorted map (predictable iteration order;
  used for the freezes file so the JSON has stable key order)

### Lifetimes (`'static`, `&'a T`)

You'll see `&'static str` in a few places (e.g. `&'static str`
returned by `cheni_meta::nh_base_version()`). It means "a borrowed
string slice that lives for the whole program" — typically because
it points into the binary's data section (string literals,
`env!()` results).

Usually you can read past `'a` in function signatures without
worrying — it's the compiler tracking lifetimes; you only have to
think about it when you're writing the function.

---

## Reading walkthrough — `pins.rs` line by line

Goal of this section: let you read a cheni-spec module top-to-bottom
without dropping into "what is this Rust thing" mode every two lines.
We use `crates/nh-nixos/src/pins.rs` because it's the simplest
cheni-spec module — every other one follows the same shape with
small additions.

The file has 4 chunks: **module header → state-file functions → the
flake-dir resolver → the subcommand impl**. We walk each chunk.

### Chunk 1 — module header (lines 1-30)

```rust
//! Per-package pins to a `nixpkgs-latest` overlay.
//!
//! cheni-specific feature carried over from the wrapper-era. The pin
//! state lives in `<flake-dir>/package-pins.json` ...
```

Lines starting with `//!` are **module-level doc comments** —
they're the file's README. The first line is what shows up in
`cargo doc` and what an IDE shows when you hover the module name.

Look for the **`# Helpers used`** subsection — it tells you which
`cheni_util` items this file relies on. If you're going to read
the body of `add()` and you see `validation::package_name(name)?`,
the header tells you the implementation lives in
`cheni_util/validation.rs`. No mystery.

### Chunk 2 — the state-file functions (lines 30-130)

```rust
const PINS_FILE: &str = "package-pins.json";

pub fn read(flake_dir: &Path) -> Result<Vec<String>> {
    let path = flake_dir.join(PINS_FILE);
    if !path.exists() {
        debug!("no {} found", PINS_FILE);
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    // ... more reading + parsing
}
```

What's happening:

- `pub fn read(...) -> Result<Vec<String>>` — public function, takes
  a path, returns "either a list of strings or an error". `Result`
  is Rust's "two-outcome" return type (see Rust glossary above).
- `flake_dir.join(PINS_FILE)` — append `"package-pins.json"` to the
  directory path. Returns a new `PathBuf`.
- `if !path.exists() { return Ok(Vec::new()); }` — if the file
  doesn't exist, that's a **valid** state (no pins yet); return an
  empty Vec wrapped in `Ok`.
- `fs::read_to_string(...)?` — read the whole file into a String.
  The trailing `?` says "if this errors, propagate the error up
  out of `read()`".
- `.with_context(|| format!("..."))` — if the error happens, add
  this human-readable context to it. The `||` syntax is a
  zero-arg closure (a function with no parameters) — used here
  so the format! only runs when there IS an error.

The same pattern repeats for `write`, `add`, `remove`, `clear`.
Each is a small function doing one thing.

When you see `atomic::write(&path, ...)?`, that's a call into
`cheni_util/atomic.rs` — the helper that does tmp-file + fsync +
rename. The header at the top of `pins.rs` told you that.

### Chunk 3 — `resolve_flake_dir` (~lines 130-180)

```rust
pub fn resolve_flake_dir(cli: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = cli {
        if has_flake(p) {
            return Ok(p.to_path_buf());
        }
        bail!("--flake-dir '{}' does not contain a flake.nix", p.display());
    }
    for var in ["NH_FLAKE", "CHENI_CONFIG"] {
        // ...
    }
    // fallback to ~/nixos-config or /etc/nixos
}
```

`Option<&Path>` means "either Some(path) or None" — the `--flake-dir`
flag is optional. The function tries each source in order and bails
out with `bail!("...")` (which constructs an error and returns it)
when none work. `bail!` is `color_eyre`'s "shortcut" for `return
Err(eyre!("..."))`.

This is the standard pattern: explicit precedence list, each step
verified, clear error if all fail.

### Chunk 4 — the subcommand impl (~lines 180-260)

```rust
impl OsPinArgs {
    pub fn run(self) -> Result<()> {
        let flake_dir = resolve_flake_dir(self.flake_dir.as_deref())?;
        if self.names.is_empty() {
            // ... list pins
            return Ok(());
        }
        let added = add(&flake_dir, &self.names)?;
        for name in &added {
            crate::timeline::record(...);
        }
        // ... print summary
    }
}
```

`impl OsPinArgs { fn run(self) -> Result<()> { ... } }` —
"implement the `run` method on the `OsPinArgs` struct". `OsPinArgs`
is defined in `args.rs` (the args struct that clap fills). When
the user runs `nh os pin firefox`, the dispatch in `nixos.rs`
ends up calling this `run` method.

The orchestrator is intentionally short — just sequencing the steps:
resolve, branch on "list vs add", call the state-file functions,
record events, print. Each step is a named function (or a single
expression). If any step needs more logic, it goes into a helper,
not inline here.

### Chunk 5 — inline tests (bottom of file)

```rust
#[cfg(test)]
#[expect(clippy::expect_used, clippy::unwrap_used, reason = "Fine in tests")]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn read_returns_empty_when_file_absent() {
        let dir = fake_flake_dir();
        assert_eq!(read(dir.path()).unwrap(), Vec::<String>::new());
    }
    // ...
}
```

`#[cfg(test)]` says "only compile this when running `cargo test`".
The `mod tests { ... }` block is the test suite for this module
(see the [Conventions](#conventions) section — inline tests are
the fork standard).

`#[expect(clippy::unwrap_used, ...)]` relaxes the project's "no
unwraps" rule for the test code only. Tests are allowed to
`.unwrap()` because a panic in a test IS the failure signal.

`fake_flake_dir()` is a small fixture defined right above the
tests — it creates a temporary directory with an empty `flake.nix`
inside. Each test calls it to get a fresh, isolated dir, so the
tests don't race in parallel.

### What about the cheni_util submodules?

Apply the same chunk-walking to `cheni_util/atomic.rs`,
`time.rs`, `validation.rs`, `flake.rs`. Each one is short
(100-200 lines) and self-contained. The "Helpers used" headers in
each cheni-spec module tell you when to drop in.

When in doubt, just read the test names — they're written like
sentences (`add_returns_only_new_names`, `prune_keeps_most_recent_n`),
so they describe exactly what the function does.

---

## Conventions

Documented in `CLAUDE.md` (the AI agent instruction file at the repo
root). Highlights:

- **Run() short** — `OsXxxArgs::run` should be a few lines that
  delegate to named helpers (`gather_*`, `print_*_section`,
  `classify_*`, etc).
- **Inline tests** in cheni-spec modules: `#[cfg(test)] mod tests
  { ... }` at the bottom. Sibling-file tests via `#[path]` were the
  wrapper-era convention; the fork uses inline for consistency with
  upstream nh files in the same crate.
- **Atomic writes** for any file the CLI mutates: use
  `cheni_util::atomic::write` (handles tmp + fsync + rename + 0o600
  + O_NOFOLLOW). Don't write a 4th private copy.
- **No `.unwrap()` in prod**, only in `#[cfg(test)]` blocks.
  Use `?` on `color_eyre::eyre::Result` everywhere else.
- **Parallel-safe tests**: no `std::env::set_var`, no
  `set_current_dir`, no shared paths. Use `tempfile::TempDir` and
  the `_in()` pattern (`pins::resolve_flake_dir` etc).
- **Validation BEFORE format**: any value flowing from disk into a
  Nix expression goes through `cheni_util::validation::*` first.
  The wrapper-era pattern of "validate at write, trust at splice"
  is rejected; we validate at every splice site too (defence-in-
  depth).
- **English in artifacts**: code comments, README, CLAUDE.md, agent
  files in `.claude/agents/` — all in English. Conversation with
  Claude can stay in French.
- **Don't touch nh-upstream files** beyond the additive points
  listed in [Architecture overview](#architecture-overview). Each
  modification multiplies merge cost.

---

## License

[EUPL-1.2](./LICENSE) — inherited from upstream nh when we forked.
