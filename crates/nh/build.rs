//! Compose the cheni-fork's full version string at build time.
//!
//! Combines `CARGO_PKG_VERSION` (e.g. `0.9.0-bootstrap`) with the
//! short git rev — sourced from `NH_REV` (set by `package.nix`
//! during a Nix build) or from `git rev-parse --short HEAD` (during
//! a plain `cargo build` outside Nix). Exports the result as
//! `CHENI_FULL_VERSION` so `interface.rs` can read it via
//! `env!("CHENI_FULL_VERSION")` and pass it to clap's `#[command(
//! version = ...)]`.
//!
//! Result: `nh --version` matches the Nix store-path identifier
//! (`cheni-0.9.0-bootstrap-a6b08e8`) instead of just showing the
//! Cargo workspace version.

fn main() {
    let pkg_version = std::env::var("CARGO_PKG_VERSION")
        .expect("CARGO_PKG_VERSION is always set by Cargo");
    let rev = std::env::var("NH_REV")
        .ok()
        .or_else(short_git_rev)
        .unwrap_or_else(|| "dev".to_string());
    println!("cargo:rustc-env=CHENI_FULL_VERSION={pkg_version}-{rev}");
    println!("cargo:rerun-if-env-changed=NH_REV");
    // Re-run the build script when HEAD moves, so `cargo build` from
    // a dirty tree picks up the new rev without a `cargo clean`.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
}

fn short_git_rev() -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}
