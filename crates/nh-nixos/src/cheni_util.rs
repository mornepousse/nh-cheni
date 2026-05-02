//! Shared utilities for cheni-spec modules.
//!
//! Sub-modules collect helpers that several cheni-spec modules used
//! to duplicate: atomic file writes, RFC 3339 / ISO date conversion,
//! package-name validation, and `flake.lock` input lookups. Lifted
//! here when the third caller appeared (audit pass 2026-05-02).
//!
//! Layout follows the post-2018 Rust convention: this file declares
//! the sub-modules, each one lives in its own file under
//! `cheni_util/<name>.rs`.

pub mod atomic;
pub mod flake;
pub mod time;
pub mod validation;
