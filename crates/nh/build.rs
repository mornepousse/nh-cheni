//! Compose the cheni-fork's full version string at build time.
//!
//! The workspace version follows option B: `<nh-base>+cheni.<cheni-ver>`
//! (e.g. `4.3.2+cheni.0.1.0`). This script decomposes it and combines
//! with the short git rev to produce the user-facing version string,
//! exported as the `CHENI_FULL_VERSION` env var so `interface.rs` can
//! read it via `env!("CHENI_FULL_VERSION")` and pass it to clap.
//!
//! Output format: `<nh-base> (cheni <cheni-ver>, <rev>)` — e.g.
//! `4.3.2 (cheni 0.1.0, b5bbdf0)`. Combined with clap's command name
//! prefix, `nh --version` displays:
//!
//!     nh 4.3.2 (cheni 0.1.0, b5bbdf0)
//!
//! Both numbers are immediately readable: which nh release we forked
//! from (4.3.2) and which iteration of our cheni layer we're at (0.1.0).
//!
//! The git rev comes from:
//!   1. `NH_REV` env var set by `package.nix` during `nix build`
//!   2. `git rev-parse --short=7 HEAD` as a fallback during plain
//!      `cargo build` outside Nix
//!   3. literal `"dev"` if neither works (no git, no env var)

fn main() {
    let pkg_version = std::env::var("CARGO_PKG_VERSION")
        .expect("CARGO_PKG_VERSION is always set by Cargo");
    let (nh_base, cheni_ver) = decompose_version(&pkg_version);
    let rev = std::env::var("NH_REV")
        .ok()
        .or_else(short_git_rev)
        .unwrap_or_else(|| "dev".to_string());

    let display = format!("{nh_base} (cheni {cheni_ver}, {rev})");
    println!("cargo:rustc-env=CHENI_FULL_VERSION={display}");

    // Also export the components individually so other modules
    // (bug-report, doctor) can reach them at compile time without
    // re-decomposing the workspace version string.
    println!("cargo:rustc-env=CHENI_NH_BASE={nh_base}");
    println!("cargo:rustc-env=CHENI_LAYER_VERSION={cheni_ver}");
    println!("cargo:rustc-env=CHENI_GIT_REV={rev}");

    println!("cargo:rerun-if-env-changed=NH_REV");
    // Re-run the build script when HEAD moves so a dirty-tree build
    // picks up the new rev without a `cargo clean`.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
}

/// Split `<nh-base>+cheni.<cheni-ver>` into its two halves. If the
/// `+cheni.` marker is absent (e.g. the version was edited away from
/// option B by mistake), the whole string is the nh-base and cheni-ver
/// degrades to "?".
fn decompose_version(full: &str) -> (&str, &str) {
    match full.split_once("+cheni.") {
        Some((base, layer)) => (base, layer),
        None => (full, "?"),
    }
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
