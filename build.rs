//! Build script — capture git metadata at compile time.
//!
//! Emits two env vars consumed by `main.rs` to build the user-facing
//! version string `0.1.{count}-alpha ({hash})`:
//!   - GIT_SHORT_HASH:   short SHA of HEAD.
//!   - GIT_COMMIT_COUNT: total commit count, used as the patch number
//!     so every commit bumps `cheni --version`.
//!
//! Cargo.toml's `version = "0.1.0-alpha"` stays static (Cargo demands a
//! literal SemVer in the manifest); the displayed version is reconstructed
//! at compile time and may differ — that's intentional.

fn main() {
    // Allow the Nix build (sandbox, no .git) to inject values from flake.nix
    // via its `env` attribute. If the vars are pre-set we take them as-is;
    // otherwise we fall back to running git, and ultimately to placeholders
    // so a tarball build (no git, no env injection) still compiles cleanly.
    let short_hash = env_or_git("CHENI_GIT_SHORT_HASH", &["rev-parse", "--short", "HEAD"])
        .unwrap_or_else(|| "unknown".into());
    let commit_count = env_or_git("CHENI_GIT_COMMIT_COUNT", &["rev-list", "--count", "HEAD"])
        .unwrap_or_else(|| "0".into());

    println!("cargo:rustc-env=GIT_SHORT_HASH={}", short_hash);
    println!("cargo:rustc-env=GIT_COMMIT_COUNT={}", commit_count);

    // Recompile when HEAD changes: covers commits, branch switches, rebases.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads/");
    // Also re-run if the injected vars change (Nix eval gave us new values).
    println!("cargo:rerun-if-env-changed=CHENI_GIT_SHORT_HASH");
    println!("cargo:rerun-if-env-changed=CHENI_GIT_COMMIT_COUNT");
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
