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
    let short_hash = run_git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into());
    let commit_count = run_git(&["rev-list", "--count", "HEAD"]).unwrap_or_else(|| "0".into());

    println!("cargo:rustc-env=GIT_SHORT_HASH={}", short_hash);
    println!("cargo:rustc-env=GIT_COMMIT_COUNT={}", commit_count);

    // Recompiler quand le HEAD change : couvre les commits, switch de branche, rebase.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads/");
}

fn run_git(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}
