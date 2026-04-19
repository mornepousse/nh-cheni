//! Build script — capture the version string at compile time.
//!
//! Emits `GIT_DESCRIBE`, used by the binary as its `--version` string
//! and by the panic/bug-report path for crash triage. Resolution order:
//!
//!   1. `git describe --tags --always --dirty` — rich output on a dev
//!      machine where `.git/` is available: exact tag (`v0.1.0-alpha`),
//!      `v0.1.0-alpha-5-g37073ac` after N commits, `-dirty` suffix when
//!      there are uncommitted changes.
//!
//!   2. `./VERSION` file shipped in the source tree. Present in every
//!      copy of the repo (checked-in), so it's the one thing the Nix
//!      sandbox can always read — `.git/` is absent there, so git
//!      describe would otherwise fall through.
//!
//!   3. `"unknown"` if neither works — keeps the build alive rather than
//!      aborting over a cosmetic.
//!
//! Release workflow: bump `VERSION` + Cargo.toml together and tag the
//! commit (`git tag vX.Y.Z`). The tag and the file stay in lockstep so
//! cargo-local builds and Nix sandbox builds agree on the name.
//!
//! Cargo.toml's `version = "0.1.0-alpha"` stays the literal SemVer
//! Cargo demands in its manifest; the *displayed* version is
//! reconstructed at compile time and may carry more detail.

fn main() {
    let describe = git_describe()
        .or_else(read_version_file)
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=GIT_DESCRIBE={}", describe);

    // Recompile when HEAD, tags, or the VERSION file change.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads/");
    println!("cargo:rerun-if-changed=.git/refs/tags/");
    println!("cargo:rerun-if-changed=VERSION");
}

fn git_describe() -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn read_version_file() -> Option<String> {
    let s = std::fs::read_to_string("VERSION").ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
