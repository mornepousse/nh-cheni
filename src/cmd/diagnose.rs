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
