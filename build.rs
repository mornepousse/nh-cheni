//! Build script — capture the version string at compile time.
//!
//! Emits one env var, `GIT_DESCRIBE`, used as the binary's --version
//! string. Resolution order:
//!
//!   1. `$CHENI_GIT_DESCRIBE` if pre-set (the Nix sandbox path —
//!      flake.nix injects it from `self.shortRev`/`self.dirtyShortRev`).
//!   2. `git describe --tags --always --dirty` (cargo build with .git
//!      available — yields `v0.1.0`, `v0.1.0-5-g37073ac`, or just
//!      `37073ac` when no tag exists yet).
//!   3. `"unknown"` (no env, no git, no .git/) — keeps the build alive.
//!
//! Cargo.toml's `version = "0.1.0-alpha"` stays static (Cargo demands a
//! literal SemVer in the manifest); the displayed version is reconstructed
//! at compile time and intentionally diverges.

fn main() {
    let describe = env_or_git(
        "CHENI_GIT_DESCRIBE",
        &["describe", "--tags", "--always", "--dirty"],
    )
    .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=GIT_DESCRIBE={}", describe);

    // Recompile when HEAD or tags change: covers commits, branch
    // switches, rebases, and `git tag` / `git tag -d`.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads/");
    println!("cargo:rerun-if-changed=.git/refs/tags/");
    // Also re-run if the injected var changes (Nix eval gave us a new value).
    println!("cargo:rerun-if-env-changed=CHENI_GIT_DESCRIBE");
}

fn env_or_git(env_name: &str, git_args: &[&str]) -> Option<String> {
    if let Ok(v) = std::env::var(env_name) {
        let trimmed = v.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    let out = std::process::Command::new("git").args(git_args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
