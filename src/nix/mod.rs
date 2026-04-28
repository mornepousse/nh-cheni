//! NixOS system interaction.
//!
//! Everything that touches the local NixOS system:
//! reading the store, detecting the config, managing pins.

pub mod config;
pub mod eval;
pub mod flake;
pub mod freezes;
pub mod gc;
pub mod git;
pub mod pins;
pub mod store;
pub mod tools;
