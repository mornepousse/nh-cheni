//! External data sources.
//!
//! Handles communication with the Repology API and caching
//! of results to avoid unnecessary network requests.
//!
//! Shared HTTP helpers (timeouts, body caps, Retry-After) live at
//! `crate::http` — they're used outside `api/` as well (`nix::flake`,
//! `release`) so they don't belong under `api/`.

pub mod cache;
pub mod repology;
