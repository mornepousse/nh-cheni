//! `cheni verify` command.
//!
//! Verifies that the currently installed cheni (or a user-supplied tag)
//! corresponds to a release signed by the trusted minisign public key.
//! Read-only: it downloads the release tarball + signature and runs
//! the same signature check as `cheni self-update`, but without
//! touching the flake, the Nix store, or the system profile.

use anyhow::{anyhow, Result};
use colored::Colorize;

use crate::release::{self, VerifyReport};

/// Options for `cheni verify`.
pub struct VerifyOptions {
    /// Tag to verify. When `None`, resolves from `env!("GIT_DESCRIBE")`
    /// (stripping any dev-build `-N-gHASH[-dirty]` suffix) so the
    /// default verifies the currently installed version.
    pub tag: Option<String>,
}

/// Run `cheni verify`.
pub async fn run(opts: VerifyOptions) -> Result<()> {
    println!("{}\n", "=== cheni verify ===".bold());

    let installed = env!("GIT_DESCRIBE");
    let tag = resolve_tag(opts.tag.as_deref(), installed)?;

    print_preamble(installed, &tag, opts.tag.is_some());

    match release::verify_release(&tag).await {
        Ok(report) => {
            print_success(&report);
            Ok(())
        }
        Err(e) => {
            print_failure(&tag, &e);
            Err(anyhow!("Verification failed for {}", tag))
        }
    }
}

/// Figure out which tag to verify against.
///
/// - Explicit `--tag vX.Y.Z` wins.
/// - Otherwise use the compile-time `GIT_DESCRIBE`, stripped of any
///   `-N-gHASH[-dirty]` dev suffix.
/// - Refuses to verify when the binary was built without git metadata
///   (`GIT_DESCRIBE == "unknown"`) — there's no tag to point at.
pub(crate) fn resolve_tag(explicit: Option<&str>, installed_describe: &str) -> Result<String> {
    if let Some(t) = explicit {
        return Ok(t.to_string());
    }
    let tag = release::strip_dev_suffix(installed_describe);
    if tag == "unknown" {
        anyhow::bail!(
            "this cheni build has no embedded version (GIT_DESCRIBE=unknown). \
             Use `cheni verify --tag vX.Y.Z` to pick a tag explicitly."
        );
    }
    Ok(tag)
}

fn print_preamble(installed: &str, tag: &str, user_specified: bool) {
    println!("  Installed binary : {}", installed.bold());
    let note = if user_specified {
        "(user override)"
    } else if installed != tag {
        "(dev suffix stripped)"
    } else {
        ""
    };
    if note.is_empty() {
        println!("  Verifying tag    : {}", tag.bold());
    } else {
        println!("  Verifying tag    : {} {}", tag.bold(), note.dimmed());
    }
    println!("  Trust anchor     : public-keys/cheni-release.pub");
    println!();
    println!("{} Downloading tarball + signature...", "[1/2]".dimmed());
    println!("  tarball   {}", release::tarball_url(tag).dimmed());
    println!("  signature {}", release::signature_url(tag).dimmed());
    println!();
    println!("{} Verifying signature...", "[2/2]".dimmed());
}

fn print_success(report: &VerifyReport) {
    println!(
        "  {} Signature valid for {}",
        "✓".green(),
        report.tag.bold()
    );
    println!("    tarball size    : {} bytes", report.tarball_bytes);
    if !report.trusted_comment.is_empty() {
        println!("    trusted comment : {}", report.trusted_comment);
    }
    println!();
    println!(
        "{} This release was signed by the holder of the cheni release key.",
        "✓".green().bold()
    );
}

fn print_failure(tag: &str, err: &anyhow::Error) {
    println!("  {} Verification failed for {}", "✗".red(), tag.bold());
    println!("    {}", err);
    println!();
    println!(
        "{} Do not trust this release without a manual cross-check. Possible causes:",
        "!".red().bold()
    );
    println!("  - No signed release exists yet for {} (404 on the .minisig asset)", tag);
    println!("  - The tarball was fetched from somewhere other than gitlab.com/harrael/cheni");
    println!("  - The release key has rotated and this cheni is too old to know the new key");
    println!("  - Someone tampered with the release");
}

#[cfg(test)]
#[path = "tests/verify.rs"]
mod tests;
