//! Output layer shared across commands that shell out to `nh` / `nix`.
//!
//! Two building blocks:
//!
//! - [`prettify`] — a pure function that strips `/nix/store/<hash>-`
//!   prefixes and similar noise out of individual lines. Used to make
//!   Nix toolchain output readable in-place without losing information
//!   (the package name and version survive the strip).
//! - [`stream`] — a subprocess runner that merges stdout+stderr via a
//!   single pipe (so the emission order is preserved), prints each
//!   line through `prettify_line`, and returns the raw accumulated
//!   output to callers that want to feed it into a structured parser
//!   like `cmd::build::parse_errors` or `cmd::diagnose::find_issues`.

pub mod prettify;
pub mod stream;

use colored::Colorize;

/// Render `[N/total] Title` step header — same shape across every
/// multi-step command (`cheni upgrade`, `cheni self-update`). Single
/// source of truth so adding a step somewhere else gets the same
/// visual without copy-pasting the println.
pub fn print_step(n: usize, total: usize, title: &str) {
    println!("{} {}", format!("[{}/{}]", n, total).dimmed(), title.bold());
}

/// Horizontal rule between steps. Keeps multi-step output skimmable —
/// each step becomes a visually distinct block rather than running
/// into its neighbours.
pub fn print_separator() {
    println!("{}", "───────────────────────────────────────────".dimmed());
}
