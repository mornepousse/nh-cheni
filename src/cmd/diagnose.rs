//! `cheni diagnose` command.
//!
//! Scans a build log (from a file or stdin) for known-failure patterns
//! and prints an actionable hint for each one it recognises. This is
//! a readability layer, not a diagnostic engine — we match simple
//! substrings against a curated list, so the cost of adding or
//! removing a pattern is one entry.

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

/// A single known-failure pattern, with human-readable context.
///
/// Lean on purpose: adding a pattern means appending one entry to
/// `KNOWN_FINDINGS`. No regex, no URL lookups, no priority ordering.
pub struct Finding {
    /// Case-insensitive substring we look for in the log.
    pub matcher: &'static str,
    /// Short headline for the issue.
    pub title: &'static str,
    /// Why the failure happens, in one or two sentences.
    pub explanation: &'static str,
    /// What the user should do about it.
    pub action: &'static str,
}

/// Curated list of known patterns. Order doesn't matter — we print
/// every match found, in the order they appear here.
pub const KNOWN_FINDINGS: &[Finding] = &[
    Finding {
        matcher: "aes_generic",
        title: "kernel module `aes_generic` not found",
        explanation: "Linux 7.0 folded `aes_generic` into the main `aes` module. \
                      Configs that still list it in `boot.initrd.availableKernelModules` \
                      fail at the modules-shrunk build step.",
        action: "Remove `aes_generic` from `boot.initrd.availableKernelModules` in your \
                 NixOS config (check `hardware-configuration.nix` as well).",
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
    },
    Finding {
        matcher: "infinite recursion encountered",
        title: "infinite recursion in the Nix expression",
        explanation: "Some attribute depends on itself through a chain of `rec`/let/with. \
                      Often triggered by an override that refers back to the \
                      overridden set. Nix can't evaluate it.",
        action: "Bisect the change: comment out recent `override`/`overrideAttrs` \
                 calls until evaluation succeeds, then reintroduce one at a time.",
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
    },
];

/// Pure core: scan `log` for every pattern and return the ones that
/// matched, in `KNOWN_FINDINGS` order, deduplicated.
pub fn find_issues(log: &str) -> Vec<&'static Finding> {
    let haystack = log.to_lowercase();
    KNOWN_FINDINGS
        .iter()
        .filter(|f| haystack.contains(&f.matcher.to_lowercase()))
        .collect()
}

/// Print a compact postscript of diagnose hints for `raw_output`, or
/// nothing at all when no pattern matches. Shared by `cheni upgrade`
/// and `cheni self-update` for the failure-mode hint injection.
pub fn print_hints_for(raw_output: &str) {
    let findings = find_issues(raw_output);
    if findings.is_empty() {
        return;
    }
    println!(
        "\n{} matched {} known issue(s):",
        "─── cheni diagnose ───".dimmed(),
        findings.len().to_string().bold()
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
        "  {} matched {} known issue(s):\n",
        "·".dimmed(),
        findings.len().to_string().bold()
    );
    for (i, f) in findings.iter().enumerate() {
        println!(
            "{} {}",
            format!("[{}/{}]", i + 1, findings.len()).dimmed(),
            f.title.bold()
        );
        println!("  {}: {}", "why".yellow(), f.explanation);
        println!("  {}: {}", "fix".green(), f.action);
        println!();
    }
}

#[cfg(test)]
#[path = "tests/diagnose.rs"]
mod tests;
