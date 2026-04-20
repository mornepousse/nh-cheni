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
