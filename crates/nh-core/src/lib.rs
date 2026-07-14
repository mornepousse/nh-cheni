pub mod args;
pub mod checks;
pub mod command;
pub mod progress;
pub mod update;
pub mod util;

pub use command::NIX_BUILD_ERROR_MARKER;

pub const NH_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const NH_REV: Option<&str> = option_env!("NH_REV");
