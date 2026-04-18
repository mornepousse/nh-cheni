//! External data sources.
//!
//! Handles communication with the Repology API and caching
//! of results to avoid unnecessary network requests.

pub mod cache;
pub mod net;
pub mod repology;
