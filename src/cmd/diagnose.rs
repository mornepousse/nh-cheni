//! `cheni diagnose` command.
//!
//! Scans a build log (from a file or stdin) for known-failure patterns
//! and prints an actionable hint for each one it recognises.
//!
//! The matcher is **phase-aware and derivation-scoped**. Rather than
//! flatten the whole log to one big haystack and substring-grep, we
//! parse it line-by-line into a [`LogContext`]:
//!
//! - each line carries the **derivation** that emitted it (extracted
//!   from `nh`'s `name>` prefix when present);
//! - each line carries a **phase** (eval / fetch / build / check /
//!   install / activate) when we can guess one — phase markers are
//!   sticky per derivation until the next marker in that derivation;
//! - the log as a whole carries an optional `failing_derivation`,
//!   pulled from the trailing `error: Cannot build '...drv'` /
//!   `error: builder for '...' failed` anchors.
//!
//! Each [`Finding`] then declares a [`Scope`] that says where it can
//! legitimately fire (anywhere, only in a given phase, only inside the
//! failing derivation, etc.). Findings only consider lines that match
//! their scope. This eliminates a real-world class of false positives
//! we hit in v0.5.8 — a `module 'aes_generic' not found` matcher
//! firing on a log whose actual failure was a Rust test panic in some
//! other derivation's `checkPhase`.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use colored::Colorize;

/// Options for `cheni diagnose`.
pub struct DiagnoseOptions {
    /// Path to a log file. `None` means read from stdin.
    pub path: Option<PathBuf>,
}

/// Run `cheni diagnose`.
pub fn run(opts: DiagnoseOptions) -> Result<()> {
    let log = load_input(opts.path.as_deref())?;
    let findings = find_issues(&log);
    print_findings(&findings);
    Ok(())
}

// ── Phase / Scope model ─────────────────────────────────────────────────────

/// A coarse classification of what a Nix build is doing on a given
/// line. We only track the phases that have meaningfully different
/// failure surfaces — the goal is to scope findings, not to mirror
/// the full nixpkgs phase taxonomy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Phase {
    /// Nix expression evaluation — `nix eval`, attribute lookup,
    /// type checks, infinite recursion, undefined variables, etc.
    Eval,
    /// A fixed-output derivation pulling source bytes (`fetchurl`,
    /// `fetchFromGitHub`, …). Where hash-mismatch lives.
    Fetch,
    /// `buildPhase` — the actual compile / link of the package.
    Build,
    /// `checkPhase` — running the package's own test suite inside
    /// the sandbox. This is where "Rust test panicked" lives.
    Check,
    /// `installPhase` — copying outputs into `$out`.
    Install,
    /// nixos-rebuild's activation / switch step. Bootloader install,
    /// systemd unit reload, `Pre-switch check`.
    Activate,
}

/// Where in a parsed [`LogContext`] a [`Finding`] is allowed to fire.
///
/// All variants are part of the API surface even when no current
/// [`Finding`] uses one — adding a new pattern is supposed to be a
/// one-line edit, and we want every legitimate scope already
/// reachable.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub(crate) enum Scope {
    /// Any line, anywhere. Legacy behaviour (and the safe default
    /// when phase attribution is unreliable, e.g. disk-full).
    Global,
    /// Only lines emitted by the derivation that ultimately failed.
    /// If the log has no failing-derivation anchor, the finding does
    /// not fire.
    FailingDerivation,
    /// Only lines we attributed to a specific phase, regardless of
    /// which derivation emitted them.
    Phase(Phase),
    /// Intersection: only lines in `phase` AND emitted by the
    /// failing derivation. The strictest, lowest-false-positive
    /// scope. If there's no failing-derivation anchor, the finding
    /// does not fire.
    FailingDerivationPhase(Phase),
}

/// A single line of the build log, with the context we managed to
/// attribute to it.
#[derive(Debug, Clone)]
pub(crate) struct LogLine<'a> {
    /// Name of the derivation that emitted the line, extracted from
    /// the `name>` prefix that `nh` injects in front of every build
    /// stream. `None` for global lines (Nix's own `error:` anchors,
    /// dependency-graph chatter, the `Updating flake inputs` banner).
    pub(crate) derivation: Option<&'a str>,
    /// Phase guessed for this line — propagated from the most recent
    /// phase marker we saw inside the same derivation. `None` until
    /// we have any signal at all (and stays `None` for global lines
    /// that weren't preceded by a phase marker).
    pub(crate) phase: Option<Phase>,
    /// The textual content of the line, with the `derivation>` prefix
    /// stripped if there was one.
    pub(crate) text: &'a str,
}

/// The whole log, parsed into context-bearing lines plus the global
/// "who failed" datum.
pub(crate) struct LogContext<'a> {
    pub(crate) lines: Vec<LogLine<'a>>,
    /// Name of the derivation that ultimately failed, extracted from
    /// the trailing `error: Cannot build '/nix/store/HASH-NAME.drv'`
    /// or `error: builder for '/nix/store/HASH-NAME.drv' failed`
    /// anchor. Stored without the `.drv` suffix and without the
    /// store-hash prefix (so `cheni-0.5.8`, not the full path).
    pub(crate) failing_derivation: Option<&'a str>,
}

// ── Finding catalogue ───────────────────────────────────────────────────────

/// How severe a [`Finding`] is, used to pick the title colour.
///
/// - `Critical` — blocks the rebuild or risks data loss (red)
/// - `Warning`  — non-blocking or fixable without a reboot (yellow)
/// - `Hint`     — informational, a "look here first" pointer (cyan)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Severity {
    Critical,
    Warning,
    Hint,
}

/// A single known-failure pattern, with human-readable context and
/// the scope where it is allowed to match.
pub(crate) struct Finding {
    /// Case-insensitive substring we look for in scoped log text.
    pub(crate) matcher: &'static str,
    /// Short headline for the issue.
    pub(crate) title: &'static str,
    /// Why the failure happens, in one or two sentences.
    pub(crate) explanation: &'static str,
    /// What the user should do about it.
    pub(crate) action: &'static str,
    /// Where in the log this pattern can legitimately fire.
    pub(crate) scope: Scope,
    /// How severe the issue is — controls title colour in output.
    pub(crate) severity: Severity,
}

/// Curated list of known patterns. Order is the print order. Scope
/// choice per entry is documented in the inline comments — when in
/// doubt we pick `Global` to preserve legacy behaviour.
pub(crate) const KNOWN_FINDINGS: &[Finding] = &[
    Finding {
        matcher: "Pre-switch check",
        title: "live switch refused (critical component change)",
        explanation: "The build succeeded but nixos-rebuild's activation pre-flight \
                      refused to switch the running system because a critical \
                      component is moving (dbus → dbus-broker, sysvinit → systemd, \
                      pulseaudio → pipewire, …). Live-switching such a change can \
                      crash dbus and take half the desktop with it. The new \
                      generation is on disk and bootable; only the live activation \
                      was blocked.",
        action: "Stage the new generation for next boot, then reboot: \
                 `sudo nh os boot ~/nixos-config && sudo reboot`. \
                 The previous generation stays bootable as a rollback target. \
                 Setting NIXOS_NO_CHECK=1 forces the switch but is strongly \
                 discouraged for these specific changes.",
        // Pre-switch is an activation concern.
        scope: Scope::Phase(Phase::Activate),
        severity: Severity::Critical,
    },
    Finding {
        matcher: "Switching into this system is not recommended",
        title: "live switch not recommended",
        explanation: "Same root cause as `Pre-switch check ... failed` — \
                      nixos-rebuild prints this human-readable line alongside \
                      the structured pre-flight failure when a critical \
                      component is changing.",
        action: "Run `sudo nh os boot ~/nixos-config` then `sudo reboot` to \
                 pick up the new generation through a clean boot rather than \
                 a runtime swap.",
        scope: Scope::Phase(Phase::Activate),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "module 'aes_generic' not found",
        title: "kernel module `aes_generic` not found",
        explanation: "Linux 7.0 folded `aes_generic` into the main `aes` module. \
                      Configs that still list it in `boot.initrd.availableKernelModules` \
                      fail at the modules-shrunk build step.",
        action: "Remove `aes_generic` from `boot.initrd.availableKernelModules` in your \
                 NixOS config (check `hardware-configuration.nix` as well).",
        // Only meaningful inside a buildPhase — eliminates the v0.5.8
        // false positive where the literal string appeared in an
        // unrelated test/eval context.
        scope: Scope::Phase(Phase::Build),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "test result: FAILED.",
        title: "Rust test panicked during Nix sandbox build",
        explanation: "A test in the package's `checkPhase` failed inside the Nix sandbox. \
                      The sandbox has a restricted environment (no `hostname` binary by \
                      default, no `/nix/var/nix/profiles`, no network, no `$HOSTNAME`) \
                      so tests that work locally can panic here.",
        action: "Inspect the failing test names above. If they're cheni's own tests, \
                 the package needs a fix. Run `nix build .#cheni` locally to reproduce \
                 — `cargo test` alone won't catch sandbox-specific failures.",
        // Strictest scope: only fire if we can prove this came from
        // the failing derivation's checkPhase.
        scope: Scope::FailingDerivationPhase(Phase::Check),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "hash mismatch in fixed-output derivation",
        title: "fixed-output hash mismatch",
        explanation: "A `fetchurl`/`fetchFromGitHub`/... expected one sha256 but the \
                      remote served different bytes. Either the upstream changed the \
                      artifact in place, or you're resolving a different mirror.",
        action: "If you own the derivation, update the hash with the value reported \
                 in the error. For nixpkgs, refresh the channel (`nix flake update`) — \
                 upstream typically gets a fix within hours.",
        scope: Scope::Phase(Phase::Fetch),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "No space left on device",
        title: "disk full during build",
        explanation: "The Nix store or /tmp ran out of space mid-build. Nix doesn't \
                      roll back the partial result — subsequent rebuilds can keep \
                      failing until you free space.",
        action: "Free space with `cheni history --gc` (trims old generations and \
                 runs `nix-collect-garbage`). If /tmp is the culprit, \
                 `TMPDIR=/var/tmp sudo nixos-rebuild switch`.",
        // Disk-full can hit during eval (write cache), fetch (NAR
        // unpack), build (object files), install (copy out). Keeping
        // it Global is the right call.
        scope: Scope::Global,
        severity: Severity::Critical,
    },
    Finding {
        matcher: "does not provide attribute",
        title: "flake attribute missing",
        explanation: "A `nix build`/`nix flake check` asked for an output attribute \
                      that the flake doesn't expose. Usually a typo, a renamed \
                      attribute after a flake update, or a system mismatch \
                      (e.g. `aarch64-linux` on an `x86_64-linux` host).",
        action: "List what the flake actually provides with \
                 `nix flake show <flake-url>` and adjust the reference.",
        scope: Scope::Phase(Phase::Eval),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "infinite recursion encountered",
        title: "infinite recursion in the Nix expression",
        explanation: "Some attribute depends on itself through a chain of `rec`/let/with. \
                      Often triggered by an override that refers back to the \
                      overridden set. Nix can't evaluate it.",
        action: "Bisect the change: comment out recent `override`/`overrideAttrs` \
                 calls until evaluation succeeds, then reintroduce one at a time.",
        scope: Scope::Phase(Phase::Eval),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "has an unfree license",
        title: "unfree package refused",
        explanation: "A package in your config ships under an unfree license \
                      (e.g. proprietary drivers, Steam, VS Code). NixOS refuses \
                      by default — the user must opt in explicitly.",
        action: "Add `nixpkgs.config.allowUnfree = true;` to your NixOS config. \
                 For a one-shot on the CLI, `NIXPKGS_ALLOW_UNFREE=1` plus `--impure` \
                 on `nix build`/`nix shell` lets a single invocation through without \
                 touching the config.",
        scope: Scope::Phase(Phase::Eval),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "is marked as broken",
        title: "broken package",
        explanation: "Someone in nixpkgs flagged this package as not-currently-building \
                      or known-failing. The marker is usually recent and documented \
                      in the GitHub issue tracker.",
        action: "First: remove the package from your config and try without it. \
                 If you genuinely need it, `nixpkgs.config.allowBroken = true;` will \
                 force-build (often fails), or `override { meta.broken = false; }` \
                 opts out at the overlay level. Check the nixpkgs issue tracker for \
                 the WHY — fixes tend to land fast on popular packages.",
        scope: Scope::Phase(Phase::Eval),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "is marked as insecure",
        title: "insecure package (CVEs or unmaintained upstream)",
        explanation: "nixpkgs refuses to evaluate a package with known unpatched CVEs \
                      or an unmaintained upstream. Typical offenders: qtwebengine-5.x, \
                      older electron/chromium, python2.7, outdated openssl. The error \
                      lists the exact derivation name (e.g. `qtwebengine-5.15.19`).",
        action: "Preferred: drop the dependency — if a module pulls it transitively, \
                 disable the offending option (e.g. Qt5 webview features). Escape \
                 hatch: `nixpkgs.config.permittedInsecurePackages = [ \"NAME-VERSION\" ];` \
                 in your config, using the exact name from the error. The pin is \
                 version-specific so a later nixpkgs bump won't auto-allow a future \
                 vulnerable release.",
        scope: Scope::Phase(Phase::Eval),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "collision between",
        title: "package collision (two packages provide the same file)",
        explanation: "`environment.systemPackages` has two packages that both install \
                      the same file (typically a `bin/` executable or a man page). \
                      Nix refuses to pick one for you — activation would be ambiguous.",
        action: "Pick one. If you need both, set a priority: \
                 `(lib.hiPrio pkgs.X)` in the preferred entry, or `(lib.lowPrio pkgs.Y)` \
                 on the other. Runs cleanly once one path is unambiguously winning.",
        // Profile build-time conflict, surfaced during system-toplevel
        // assembly. Keep Global: the message can land before activation
        // markers are visible.
        scope: Scope::Global,
        severity: Severity::Warning,
    },
    Finding {
        matcher: "is forbidden in pure eval mode",
        title: "absolute path access in pure eval mode",
        explanation: "Flakes evaluate in pure mode: absolute paths like `/home/user/foo` \
                      are refused because they're not reproducible across machines. \
                      Usually a `path:/...` flake input, an `import /abs/path`, or \
                      a secret-loading trick meant for impure eval.",
        action: "Replace the absolute path with a relative one (`./foo`) or add the \
                 file as a proper flake input. For a deliberate one-shot, re-run \
                 the command with `--impure` — but avoid making that the default, \
                 you lose reproducibility guarantees.",
        scope: Scope::Phase(Phase::Eval),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "does not exist in the flake",
        title: "file referenced but not tracked by git",
        explanation: "Flakes only see files that git knows about. A new `.nix` file \
                      that hasn't been `git add`-ed is invisible to the flake source \
                      copied into the Nix store, so `imports = [ ./foo.nix ];` fails \
                      with `does not exist in the flake`.",
        action: "`git add <file>` (you can `git commit` later — staging is enough for \
                 the flake to see it). A trailing `warning: Git tree '...' is dirty` \
                 in the same output is the usual smoking gun.",
        scope: Scope::Phase(Phase::Eval),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "cached failure of attribute",
        title: "stale flake eval-cache masking the real error",
        explanation: "Nix's flake evaluation cache remembered a previous failure and \
                      is replaying it instead of re-evaluating. Even `--show-trace` \
                      returns this uninformative line. The underlying cause may have \
                      been fixed already, but the cache still says no.",
        action: "Re-run with `--option eval-cache false` (or `--no-eval-cache`) to \
                 force a fresh evaluation. Once the real error surfaces, fix that; \
                 the cache will update on the next successful eval. See \
                 https://github.com/NixOS/nix/issues/3872 for the root cause.",
        scope: Scope::Phase(Phase::Eval),
        severity: Severity::Hint,
    },
    Finding {
        matcher: "SSL peer certificate",
        title: "TLS failure fetching from a substituter",
        explanation: "Nix couldn't validate the TLS certificate of a download target \
                      (typically cache.nixos.org or a private binary cache). Common \
                      causes: corporate VPN/proxy (ZScaler, mitmproxy) intercepting \
                      TLS, an out-of-date CA bundle, or `NIX_SSL_CERT_FILE` pointing \
                      at the wrong file.",
        action: "Check `NIX_SSL_CERT_FILE` — on NixOS it should point at \
                 `/etc/ssl/certs/ca-bundle.crt`. If behind a corporate proxy, add \
                 the proxy's CA cert to the bundle (or `--option ssl-cert-file <path>`). \
                 `curl -v https://cache.nixos.org/` reproduces without nix in the loop.",
        // TLS errors land during fetch (substituter download) but
        // can also surface during eval (flake metadata fetch). Keep
        // Global to avoid silencing the eval-time variant.
        scope: Scope::Global,
        severity: Severity::Warning,
    },
    Finding {
        matcher: "undefined variable",
        title: "undefined variable in the Nix expression",
        explanation: "A name used in the config isn't in scope — a typo, a missing \
                      `let`-binding, a module attribute accessed before its module \
                      is imported, or a `with pkgs;` section that doesn't contain \
                      what you expected. The error line usually points at the exact \
                      offending reference.",
        action: "Read the `at ...` file:line:col in the error — that's where the \
                 undefined name sits. Check for typos, a missing `import`, or a \
                 missing `inputs.<foo>.follows = \"nixpkgs\";` wiring for a flake \
                 input that injects packages.",
        scope: Scope::Phase(Phase::Eval),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "cannot coerce",
        title: "type mismatch — usually function passed where string expected",
        explanation: "Nix found a value of one type where another was expected. \
                      The common shape is `cannot coerce a function to a string`, \
                      which happens when you write `pkgs.writeScript \"foo\" pkgs.bash` \
                      instead of `pkgs.writeScript \"foo\" \"${pkgs.bash}/bin/bash\"`, \
                      or pass an attribute set where a path was expected.",
        action: "Inspect the `at ...` location in the error. If the value is a \
                 function, you probably forgot to call it (`f arg` instead of `f`). \
                 If it's an attribute set, you likely want a specific field \
                 (`pkg.out`, `\"${pkg}\"`, `pkg.meta.mainProgram`).",
        scope: Scope::Phase(Phase::Eval),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "experimental Nix feature",
        title: "experimental feature (flakes / nix-command) not enabled",
        explanation: "The command you ran relies on `nix-command` or `flakes`, \
                      which are still marked experimental in Nix and must be \
                      enabled explicitly. On NixOS this is typically set once \
                      in the system config; on a stock multi-user Nix install \
                      it's usually missing.",
        action: "Persistent fix, NixOS: \
                 `nix.settings.experimental-features = [ \"nix-command\" \"flakes\" ];` \
                 in configuration.nix. \
                 Persistent fix, non-NixOS: add \
                 `experimental-features = nix-command flakes` to \
                 `~/.config/nix/nix.conf` (or `/etc/nix/nix.conf`). \
                 One-shot: prefix any command with \
                 `--extra-experimental-features 'nix-command flakes'`.",
        // This always trips before any phase signal lands.
        scope: Scope::Global,
        severity: Severity::Warning,
    },
    Finding {
        matcher: "is in the way of",
        title: "home-manager refuses to overwrite an existing file",
        explanation: "home-manager protects files it doesn't own: if your home \
                      directory already has (say) `~/.config/git/config` from \
                      before you declared `programs.git.enable = true;`, activation \
                      stops rather than clobber your manual copy.",
        action: "Two clean options. Move the existing file aside manually \
                 (`mv ~/.config/git/config{,.pre-hm}` and re-run), or let \
                 home-manager back it up automatically with \
                 `home-manager.backupFileExtension = \"backup\";` in your \
                 NixOS-level home-manager block. The second option stays \
                 idempotent across rebuilds.",
        scope: Scope::Phase(Phase::Activate),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "API rate limit exceeded",
        title: "GitHub API rate limit hit during flake fetch",
        explanation: "Anonymous GitHub API calls are capped at 60/hour per IP. \
                      `github:owner/repo` flake refs hit the API — a CGNAT'd \
                      connection or a shared office IP exhausts the quota \
                      quickly, and every `nix flake update` adds to the count.",
        action: "Configure an access token: create a fine-grained personal \
                 access token on GitHub (read-only is enough), then add \
                 `nix.settings.access-tokens = [ \"github.com=<token>\" ];` \
                 to your NixOS config. The per-token quota is 5000/hour. \
                 Alternatively, switch specific flakes from `github:` to \
                 `git+https://github.com/.../repo.git?ref=main` — bypasses \
                 api.github.com entirely.",
        // Lands at flake metadata resolution — pre-build, eval-ish.
        scope: Scope::Global,
        severity: Severity::Hint,
    },
    Finding {
        matcher: "exit code 137",
        title: "build killed by the OOM killer (exit 137 = SIGKILL)",
        explanation: "Exit code 137 is `128 + 9` (SIGKILL). On Linux, a build \
                      that hits this without a deliberate `kill -9` was almost \
                      certainly reaped by the kernel's out-of-memory killer — \
                      some packages (CUDA, large C++ projects, LLVM-based \
                      toolchains) peak well past 8 GB during link.",
        action: "Rebuild on a machine with more RAM, or raise your swap: \
                 `swapon --show` then `swapoff` + resize + `swapon`. \
                 On a VM, bump `memorySize`. If you're on a laptop and can't \
                 add RAM, pinning the package to a pre-built binary cache \
                 version via `cheni pin` sidesteps the local build entirely.",
        scope: Scope::Phase(Phase::Build),
        severity: Severity::Critical,
    },
    Finding {
        matcher: "Temporary failure in name resolution",
        title: "DNS resolution failure inside a build or fetch",
        explanation: "A derivation tried to reach a hostname and couldn't \
                      resolve it. Typical causes: the Nix sandbox strips DNS \
                      (fetches must use `fetchurl` / `fetchFromGitHub` with \
                      fixed-output hashes, which go through the daemon's \
                      resolver), `/etc/resolv.conf` is stale, or a \
                      corporate VPN blocks the DNS the nix daemon is using.",
        action: "For fixed-output derivations: confirm your DNS works with \
                 `getent hosts github.com`. If the derivation itself tries \
                 to resolve names at build time (not fixed-output), it's a \
                 package bug — file an issue. For flake updates failing: \
                 same check, then verify `nix.settings.substituters` aren't \
                 pointing at an unreachable host.",
        // DNS can fail in fetch (substituter), build (network-allowed
        // fixed-output), or eval (flake metadata). Stay Global.
        scope: Scope::Global,
        severity: Severity::Warning,
    },
    Finding {
        matcher: "syntax error, unexpected",
        title: "Nix syntax error",
        explanation: "The Nix parser hit a token it didn't expect — typically \
                      a missing closing brace/bracket, a stray `;`, an \
                      unterminated string, or a `with`/`let` that wasn't \
                      followed by `in`. The error line:column in the output \
                      points at where the parser gave up, not necessarily \
                      where the real mistake is.",
        action: "Look a few lines ABOVE the reported position — the missing \
                 delimiter usually lives upstream. Running `nix-instantiate \
                 --parse <file.nix> > /dev/null` isolates the parse step \
                 without eval. An editor with Nix syntax support (VS Code \
                 + Nix plugin, Emacs nix-mode, etc.) catches most of these \
                 live.",
        scope: Scope::Phase(Phase::Eval),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "is not of type",
        title: "NixOS option value has the wrong type",
        explanation: "A config entry was assigned a value that doesn't match \
                      the option's declared type — `environment.systemPackages \
                      = [ \"firefox\" ];` (string) where a list of `package` \
                      was expected, or `services.picom.activeOpacity = \"0.8\";` \
                      (string) where the module now wants a float. The full \
                      error points at the exact `[definition N-entry M]` so \
                      you can trace back to the offending assignment.",
        action: "Re-run with `--show-trace` for the full path to the bad \
                 definition. Usual fix: swap the string for the real package \
                 (`pkgs.firefox`), or the float for a number literal (`0.8` \
                 not `\"0.8\"`). Type changes after a nixpkgs bump are a \
                 common trigger — check the release notes for renamed/retyped \
                 options.",
        scope: Scope::Phase(Phase::Eval),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "Failed to start",
        title: "systemd service failed to start after activation",
        explanation: "The rebuild succeeded at eval/build time but one or more \
                      systemd units failed when the new configuration was \
                      activated. The previous generation is still active; \
                      boot should still work, but the feature that needed the \
                      failing service won't.",
        action: "Inspect the unit: `journalctl -u <service>.service -b` \
                 (recent boot) or `systemctl status <service>.service`. \
                 Common culprits on desktops: \
                 `systemd-networkd-wait-online.service` timing out on \
                 unused NICs (disable with `systemd.network.wait-online.enable \
                 = false;`), `systemd.update-utmp.service` on btrfs (known \
                 upstream issue), ad-hoc service definitions with a bad \
                 `ExecStart` path.",
        scope: Scope::Phase(Phase::Activate),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "cannot parse flake reference",
        title: "malformed flake URL",
        explanation: "A flake ref in your config (or on the command line) \
                      doesn't match any of the accepted shapes: `github:...`, \
                      `gitlab:...`, `git+https://...`, `git+ssh://...`, \
                      `path:...`. Typical mistakes: a trailing slash, a \
                      `.git` suffix on a github short-ref, or a missing \
                      protocol prefix.",
        action: "Test the ref in isolation: `nix flake metadata <ref>`. \
                 Common valid shapes: `github:owner/repo`, \
                 `github:owner/repo/branch`, \
                 `git+https://gitlab.com/owner/repo.git?ref=main`, \
                 `path:./subflake`. Drop trailing slashes and quote the ref \
                 if it contains special characters.",
        scope: Scope::Phase(Phase::Eval),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "Authentication failed for",
        title: "private repository fetch without credentials",
        explanation: "A `git+https://` or `git+ssh://` flake input points at \
                      a repository that requires authentication and nix \
                      doesn't have credentials for it. Public repos served \
                      over SSH also fail this way when your SSH key isn't \
                      loaded in the agent.",
        action: "For `git+https://github.com/...` private refs: configure \
                 `access-tokens` in `nix.settings` with a read-scoped PAT. \
                 For `git+ssh://`: make sure `ssh-agent` is running and the \
                 key is added (`ssh-add ~/.ssh/id_ed25519`); `ssh -T \
                 git@github.com` should say \"Hi <user>!\" before nix \
                 will have any luck.",
        scope: Scope::Phase(Phase::Fetch),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "failed to install the bootloader",
        title: "bootloader install failed — system may not boot",
        explanation: "Activation reached the bootloader step and it refused \
                      to install. Typical triggers: the disk UUID in \
                      `boot.loader.grub.device` doesn't match the current \
                      device, EFI/BIOS mode mismatch (systemd-boot needs \
                      EFI, grub in legacy mode needs a BIOS boot partition), \
                      or `/boot` is read-only. Critically: the *previous* \
                      generation's bootloader is untouched — rebooting still \
                      brings you back to the old working system.",
        action: "Do not reboot until this is fixed. Check \
                 `lsblk -f` / `efibootmgr -v` to confirm the device layout, \
                 then either correct `boot.loader.grub.device` / \
                 `boot.loader.systemd-boot.enable` in your config or \
                 remount `/boot` writable. `cheni rollback` is always \
                 available as an escape hatch.",
        scope: Scope::Phase(Phase::Activate),
        severity: Severity::Critical,
    },
    Finding {
        matcher: "cannot allocate memory",
        title: "memory exhausted during evaluation (not a build)",
        explanation: "Different from exit code 137: this fires at eval time \
                      (before any build starts), when Nix itself or `nix \
                      flake` runs out of memory while walking the config. \
                      Large configs with many `rec`/`with` blocks blow up \
                      memory here. Unlike 137, swap doesn't help much — \
                      Nix's working set needs RAM.",
        action: "Close other memory-heavy apps and retry. For persistent \
                 cases on low-RAM machines, evaluate on a bigger box and \
                 copy the result: `nix copy --to ssh://laptop \
                 /nix/store/<drv>` after building remotely. Longer-term: \
                 look for large `attrNames` iterations or transitive \
                 `rec` webs in your config.",
        scope: Scope::Phase(Phase::Eval),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "untrusted substituter",
        title: "binary cache rejected because it's not trusted",
        explanation: "Nix refused to pull a build from a substituter \
                      (binary cache) because either its public key isn't \
                      in `trusted-public-keys` or the substituter URL \
                      isn't listed in `trusted-substituters`. The build \
                      would have succeeded, but without cache trust Nix \
                      won't use the pre-built artifact.",
        action: "Add the substituter's public key to \
                 `nix.settings.trusted-public-keys` AND list its URL in \
                 `nix.settings.substituters` (or `trusted-substituters`). \
                 For well-known caches like cachix, their installation \
                 docs publish both the URL and the key. Without trust \
                 configured, Nix falls back to building locally.",
        scope: Scope::Global,
        severity: Severity::Hint,
    },
    Finding {
        matcher: "refusing to overwrite",
        title: "activation refused to overwrite an existing file",
        explanation: "A NixOS module (not home-manager this time) tried \
                      to place a file where something non-Nix-managed \
                      already lives. Common with `environment.etc.\"…\".text` \
                      colliding with a file you wrote by hand, or a \
                      service putting a config in `/etc` that was \
                      previously managed outside NixOS.",
        action: "Move the existing file aside (`mv /etc/foo /etc/foo.pre-nixos`) \
                 and re-run the rebuild. If the file is meant to stay \
                 hand-managed, reconsider whether the module should own \
                 it — either remove the module declaration or mark the \
                 option with `lib.mkForce` to override the default.",
        scope: Scope::Phase(Phase::Activate),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "is used but not defined",
        title: "option referenced without the declaring module loaded",
        explanation: "Your config sets a NixOS option (e.g. \
                      `hardware.nvidia.open = true;`) but the module \
                      that *declares* it isn't in scope — typical causes: \
                      a missing `imports = [ ... ];`, a typo in the \
                      attribute path, or the option was renamed/removed \
                      in a recent nixpkgs bump (a classic is \
                      `hardware.opengl` → `hardware.graphics`).",
        action: "Grep nixpkgs for the option name — if it's truly gone, \
                 check the release notes of the nixpkgs channel you \
                 upgraded from/to for the rename. If it's still there, \
                 confirm the module that declares it is in your \
                 `imports = [ ... ];`. `nixos-option <attribute-path>` \
                 (or the generic `nix eval .#nixosConfigurations.<host>.options.<path>`) \
                 is the fast way to check whether the option exists at all.",
        scope: Scope::Phase(Phase::Eval),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "NAR hash mismatch",
        title: "flake input narHash in flake.lock is stale",
        explanation: "The narHash recorded in your `flake.lock` for an \
                      input doesn't match what Nix just computed from \
                      the fetched source. Distinct from a fixed-output \
                      hash mismatch (that's upstream changing bytes); \
                      this one is usually a Nix-version change in how \
                      it hashes an input — git submodules handling \
                      changed between 2.18 → 2.22, zip input hashing \
                      changed at 2.21, `export-subst` in .gitattributes \
                      produces unstable hashes by design.",
        action: "`nix flake update <input>` regenerates the lock entry \
                 with the new hash. If you can't afford to bump the \
                 input, pin Nix to a version before the hash-mode change \
                 (last-ditch). For submodule inputs, `flake = false;` on \
                 the input sidesteps the recursive-hash recomputation.",
        scope: Scope::Phase(Phase::Eval),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "dependencies couldn't be built",
        title: "upstream dependency failed — real error is earlier in the log",
        explanation: "When a package deep in your closure fails, every \
                      derivation that depends on it fails too — and nix \
                      reports the SHALLOW failure last, so scrolling to \
                      the bottom gives you \"1 dependencies couldn't be \
                      built\" with no clue about which one. The real \
                      error is somewhere earlier, attached to the \
                      actual broken build.",
        action: "Scroll up to the FIRST `error: builder for ...failed` \
                 entry — that's the root cause. `nix log <drv-path>` \
                 replays the full build output for a specific \
                 derivation. For a rebuilder loop, pass \
                 `--keep-going` so every failure surfaces in one pass \
                 instead of stopping at the first.",
        scope: Scope::Global,
        severity: Severity::Hint,
    },
    Finding {
        matcher: "access to network is forbidden",
        title: "build tried to reach the network inside the sandbox",
        explanation: "By default Nix builds run in a network-isolated \
                      namespace — a pure build cannot fetch anything. \
                      If the derivation legitimately needs the network \
                      (a source download), it must be a fixed-output \
                      derivation (declare `outputHash`) so Nix can \
                      verify what came back. The error means either the \
                      package is mis-packaged upstream or your build \
                      step is trying to download something it shouldn't.",
        action: "If the derivation is yours, declare the fetch as a \
                 `fetchurl` / `fetchFromGitHub` / similar fixed-output \
                 derivation with `sha256`. If you're building someone \
                 else's package and hit this, file an issue — don't \
                 reach for `--option sandbox false`, that's a global \
                 security knob and should stay on.",
        scope: Scope::Phase(Phase::Build),
        severity: Severity::Warning,
    },
    Finding {
        matcher: "Too many open files",
        title: "process hit the file-descriptor limit",
        explanation: "Systemd services, including `nix-daemon`, inherit \
                      the per-service `LimitNOFILE` ceiling (default \
                      1024 on many distros). Large builds and \
                      `nix develop` shells can open tens of thousands \
                      of files while walking a closure — blow through \
                      1024 and fd-heavy operations start returning \
                      errors that look like random I/O failures.",
        action: "On NixOS: `systemd.services.nix-daemon.serviceConfig.\
LimitNOFILE = 1048576;` in your config (then `systemctl daemon-reload \
&& systemctl restart nix-daemon`). `/etc/security/limits.conf` and \
                 `ulimit -n` don't help for systemd services — they \
                 need the `LimitNOFILE=` directive.",
        scope: Scope::Global,
        severity: Severity::Warning,
    },
];

// ── Public API ──────────────────────────────────────────────────────────────

/// Pure core: scan `log` for every pattern and return the ones that
/// matched, in `KNOWN_FINDINGS` order, deduplicated.
///
/// Public signature is unchanged from the pre-refactor version — this
/// is what `cmd::self_update` and friends consume.
pub(crate) fn find_issues(log: &str) -> Vec<&'static Finding> {
    let ctx = parse_log_context(log);
    KNOWN_FINDINGS
        .iter()
        .filter(|f| finding_matches(f, &ctx))
        .collect()
}

/// Print a compact postscript of diagnose hints for `raw_output`, or
/// nothing at all when no pattern matches. Shared by `cheni upgrade`
/// and `cheni self-update` for the failure-mode hint injection.
pub(crate) fn print_hints_for(raw_output: &str) {
    let findings = find_issues(raw_output);
    if findings.is_empty() {
        return;
    }
    println!(
        "\n{} matched {} known {}:",
        "─── cheni diagnose ───".dimmed(),
        findings.len().to_string().bold(),
        crate::util::pluralize(findings.len(), "issue")
    );
    for (i, f) in findings.iter().enumerate() {
        println!(
            "  {} {}",
            format!("[{}/{}]", i + 1, findings.len()).dimmed(),
            f.title.bold()
        );
        println!("      {}: {}", "why".yellow(), f.explanation);
        println!("      {}: {}", "fix".green(), f.action);
    }
    println!();
}

// ── Log parsing ─────────────────────────────────────────────────────────────

/// Strip the `derivation>` prefix `nh` puts in front of every line of
/// a build's stdout/stderr stream, returning `(derivation, rest)`.
///
/// Returns `None` when the line has no recognisable `name>` prefix.
/// Keeps the rule conservative: the prefix is `[A-Za-z0-9._+-]+` then
/// a literal `> ` (with optional space). This avoids matching lines
/// that just contain a `>` for some other reason (`error: ... -> ...`).
fn split_derivation_prefix(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let ok = b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'+' || b == b'-';
        if !ok {
            break;
        }
        i += 1;
    }
    if i == 0 || i >= bytes.len() || bytes[i] != b'>' {
        return None;
    }
    // Reject pure-numeric prefixes (line numbers, pids) that would
    // otherwise satisfy the alnum rule. A real derivation name has at
    // least one non-digit.
    let name = &line[..i];
    if name.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut rest_start = i + 1;
    if rest_start < bytes.len() && bytes[rest_start] == b' ' {
        rest_start += 1;
    }
    Some((name, &line[rest_start..]))
}

/// Inspect `text` (a single log line, possibly already de-prefixed)
/// and return the [`Phase`] it announces, if any.
///
/// Markers are intentionally narrow — we only honour the strings that
/// nixpkgs / nh actually emit, not anything that *contains* the word.
fn extract_phase(text: &str) -> Option<Phase> {
    let lower = text.to_ascii_lowercase();
    // Activation markers — nixos-rebuild / nh activate output.
    if lower.contains("activating the configuration")
        || lower.contains("setting up /etc")
        || lower.contains("reloading user units")
        || lower.contains("installing the boot loader")
        || lower.contains("pre-switch check")
        || lower.contains("switching into this system")
    {
        return Some(Phase::Activate);
    }
    // Phase banners that nixpkgs' generic builder prints.
    if lower.contains("checkphase") || lower.contains("running tests") {
        return Some(Phase::Check);
    }
    if lower.contains("installphase") {
        return Some(Phase::Install);
    }
    if lower.contains("buildphase") {
        return Some(Phase::Build);
    }
    if lower.contains("unpackphase") || lower.contains("patchphase") || lower.contains("configurephase") {
        // Pre-build setup steps still belong to Build for our purposes.
        return Some(Phase::Build);
    }
    // Eval-stage banners.
    if lower.starts_with("evaluating")
        || lower.contains("updating flake inputs")
        || lower.contains("warning: git tree")
    {
        return Some(Phase::Eval);
    }
    // Fetch-stage banners — fixed-output derivations and substituter
    // downloads.
    if lower.starts_with("trying https://")
        || lower.starts_with("downloading")
        || lower.contains("copying path") && lower.contains("from")
    {
        return Some(Phase::Fetch);
    }
    // `building '/nix/store/...drv'...` — a derivation entered build.
    if lower.starts_with("building '") || lower.contains(" building '") {
        return Some(Phase::Build);
    }
    None
}

/// Find the failing-derivation anchor in `log`. Returns the bare name
/// (no store-hash prefix, no `.drv` extension) of the derivation that
/// `error: Cannot build '...'` or `error: builder for '...' failed`
/// reports. `None` if neither anchor is present.
pub(crate) fn find_failing_derivation(log: &str) -> Option<&str> {
    // Anchors we look for, in order of specificity.
    const ANCHORS: &[&str] = &[
        "error: Cannot build '",
        "error: builder for '",
        "error: build of '",
    ];
    for anchor in ANCHORS {
        if let Some(start) = log.find(anchor) {
            let rest = &log[start + anchor.len()..];
            if let Some(end) = rest.find('\'') {
                let path = &rest[..end];
                if let Some(name) = derivation_name_from_path(path) {
                    return Some(name);
                }
            }
        }
    }
    None
}

/// Extract the bare derivation name from a `/nix/store/HASH-NAME.drv`
/// or plain `NAME.drv` / `NAME` string.
fn derivation_name_from_path(path: &str) -> Option<&str> {
    // Last path component.
    let basename = path.rsplit('/').next().unwrap_or(path);
    // Strip `.drv`.
    let no_drv = basename.strip_suffix(".drv").unwrap_or(basename);
    // Strip the store-hash prefix `HHHH...HHHH-` if present (32 chars
    // base32 then a dash).
    let stripped = if let Some(idx) = no_drv.find('-') {
        let (head, tail) = no_drv.split_at(idx);
        // Heuristic: the hash is exactly 32 chars and base32 (lower
        // alnum). If that doesn't match, keep the whole basename.
        if head.len() == 32 && head.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) {
            &tail[1..]
        } else {
            no_drv
        }
    } else {
        no_drv
    };
    if stripped.is_empty() { None } else { Some(stripped) }
}

/// Parse `log` into a [`LogContext`] — split into lines, attribute
/// each line to a derivation (when prefixed) and a phase (sticky from
/// the most recent marker in the same derivation, or globally for
/// non-prefixed lines).
pub(crate) fn parse_log_context(log: &str) -> LogContext<'_> {
    use std::collections::HashMap;

    // Sticky phase, per derivation. The `None` key represents the
    // "global" stream (lines without a `name>` prefix).
    let mut sticky: HashMap<Option<&str>, Phase> = HashMap::new();
    let mut lines: Vec<LogLine<'_>> = Vec::new();

    for raw in log.lines() {
        let (deriv, text) = match split_derivation_prefix(raw) {
            Some((n, t)) => (Some(n), t),
            None => (None, raw),
        };
        // If this line announces a phase, update the sticky map for
        // its derivation BEFORE attribution — the announcing line
        // itself belongs to the new phase.
        if let Some(p) = extract_phase(text) {
            sticky.insert(deriv, p);
        }
        let phase = sticky.get(&deriv).copied();
        lines.push(LogLine {
            derivation: deriv,
            phase,
            text,
        });
    }
    let failing_derivation = find_failing_derivation(log);
    LogContext {
        lines,
        failing_derivation,
    }
}

// ── Scoped matching ─────────────────────────────────────────────────────────

/// Does `finding` match anywhere in `ctx`, given its scope?
///
/// Scope handling has two pragmatic fallbacks, layered on the strict
/// rules:
///
/// 1. If the entire log is "phase-less" (no line ever carried phase
///    attribution), a [`Scope::Phase`] finding degrades to
///    [`Scope::Global`] for that log. This keeps short error
///    fragments like `error: undefined variable 'pkgs'` matchable
///    when the user pipes a one-liner to `cheni diagnose`.
/// 2. If no line carries a `derivation>` prefix at all (i.e. the log
///    isn't an `nh` stream — typical of `nix build` raw output), the
///    derivation half of [`Scope::FailingDerivation`] /
///    [`Scope::FailingDerivationPhase`] degrades — we still require
///    a non-`None` `failing_derivation` anchor (the strict
///    "no anchor ⇒ no fire" guarantee), but per-line derivation
///    attribution is no longer required to match.
fn finding_matches(finding: &Finding, ctx: &LogContext<'_>) -> bool {
    let needle = finding.matcher.to_ascii_lowercase();
    let any_phase_seen = ctx.lines.iter().any(|l| l.phase.is_some());
    let any_deriv_prefix = ctx.lines.iter().any(|l| l.derivation.is_some());
    ctx.lines.iter().any(|l| {
        line_in_scope(
            l,
            &finding.scope,
            ctx.failing_derivation,
            any_phase_seen,
            any_deriv_prefix,
        ) && line_contains_ci(l.text, &needle)
    })
}

/// Case-insensitive contains. We pre-lowered `needle`; do the same
/// for the haystack on the hot path.
fn line_contains_ci(haystack: &str, lower_needle: &str) -> bool {
    haystack.to_ascii_lowercase().contains(lower_needle)
}

/// Is `line` eligible under `scope`?
///
/// `failing` carries the parsed `error: Cannot build '...'` anchor
/// (if any). `any_phase_seen` and `any_deriv_prefix` are global
/// properties of the whole log used to drive the fallback policy
/// documented on [`finding_matches`].
fn line_in_scope(
    line: &LogLine<'_>,
    scope: &Scope,
    failing: Option<&str>,
    any_phase_seen: bool,
    any_deriv_prefix: bool,
) -> bool {
    // Helper: is this line "from the failing derivation"?
    // Strict mode (the log has `nh` prefixes): the line's derivation
    // name must align with the anchor's name. We accept either an
    // exact match OR a `pname`-style prefix match — `nh` truncates
    // the per-line label to the package's pname (`cheni`) while the
    // `.drv` anchor carries the full `pname-version`
    // (`cheni-v0.5.8`). Either side being a `<other>-...` extension
    // of the other is good enough.
    // Degraded mode (no prefixes anywhere in the log): accept any
    // line as long as a failing-derivation anchor exists at all.
    let from_failing = |fail: &str| -> bool {
        if !any_deriv_prefix {
            return true;
        }
        match line.derivation {
            None => false,
            Some(d) if d == fail => true,
            Some(d) => {
                // pname-prefix relation: one is the other followed
                // by `-version`. Stay strict on the `-` boundary so
                // that `cheni-v0.5.8` still matches `cheni`, but
                // `chenix` does NOT match `cheni`.
                fail.strip_prefix(d).is_some_and(|rest| rest.starts_with('-'))
                    || d.strip_prefix(fail).is_some_and(|rest| rest.starts_with('-'))
            }
        }
    };
    let phase_ok = |p: Phase| -> bool {
        if !any_phase_seen {
            return true;
        }
        line.phase == Some(p)
    };
    match scope {
        Scope::Global => true,
        Scope::Phase(p) => phase_ok(*p),
        Scope::FailingDerivation => match failing {
            Some(name) => from_failing(name),
            None => false,
        },
        Scope::FailingDerivationPhase(p) => match failing {
            Some(name) => from_failing(name) && phase_ok(*p),
            None => false,
        },
    }
}

// ── I/O & rendering ─────────────────────────────────────────────────────────

/// Read the log text — either from a user-supplied path or stdin.
fn load_input(path: Option<&Path>) -> Result<String> {
    match path {
        Some(p) => std::fs::read_to_string(p)
            .with_context(|| format!("reading {}", p.display())),
        None => {
            let mut s = String::new();
            std::io::stdin()
                .read_to_string(&mut s)
                .context("reading stdin")?;
            Ok(s)
        }
    }
}

fn print_findings(findings: &[&Finding]) {
    println!("{}\n", "=== cheni diagnose ===".bold());
    if findings.is_empty() {
        println!(
            "  {} No known issues found in the log.",
            "·".dimmed()
        );
        println!(
            "  {}",
            "(cheni only recognises a curated set of patterns — absence here \
             does not mean the log is clean.)".dimmed()
        );
        return;
    }
    println!(
        "  {} matched {} known {}:\n",
        "·".dimmed(),
        findings.len().to_string().bold(),
        crate::util::pluralize(findings.len(), "issue")
    );
    for (i, f) in findings.iter().enumerate() {
        let title = match f.severity {
            Severity::Critical => f.title.bold().red().to_string(),
            Severity::Warning  => f.title.bold().yellow().to_string(),
            Severity::Hint     => f.title.bold().cyan().to_string(),
        };
        println!(
            "{} {}",
            format!("[{}/{}]", i + 1, findings.len()).dimmed(),
            title
        );
        println!("  {}: {}", "why".yellow(), f.explanation);
        println!("  {}: {}", "fix".green(), f.action);
        println!();
    }
}

#[cfg(test)]
#[path = "tests/diagnose.rs"]
mod tests;
