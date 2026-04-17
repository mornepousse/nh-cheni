//! NixOS system interaction.
//!
//! Everything that touches the local NixOS system:
//! reading the store, detecting the config, managing pins.

pub mod config;
pub mod pins;
pub mod store;
