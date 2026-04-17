//! nixup — Granular package updates for NixOS.
//!
//! A CLI tool that lets you check, select, and apply updates
//! per-package on NixOS, integrated with your flake configuration.

mod api;
mod cmd;
mod nix;
mod version;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

/// Granular package updates for NixOS.
#[derive(Parser)]
#[command(name = "nixup", version, about)]
struct Cli {
    /// Increase verbosity (-v for debug, -vv for trace)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Disable colored output
    #[arg(long)]
    no_color: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show available package updates
    Check {
        /// Filter by module category (e.g. --dev, --apps)
        #[arg(long)]
        dev: bool,
        #[arg(long)]
        apps: bool,
        #[arg(long)]
        desktop: bool,
        #[arg(long)]
        hardware: bool,
        /// Custom category name
        #[arg(long, value_name = "CATEGORY")]
        category: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Respect NO_COLOR environment variable
    // See https://no-color.org/
    if cli.no_color || std::env::var("NO_COLOR").is_ok() {
        colored::control::set_override(false);
    }

    // Set up logging based on verbosity level
    let filter = match cli.verbose {
        0 => "warn",
        1 => "nixup=debug",
        _ => "nixup=trace",
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(filter)),
        )
        .with_target(false)
        .without_time()
        .init();

    // Dispatch to the right command
    match cli.command {
        Commands::Check {
            dev,
            apps,
            desktop,
            hardware,
            category,
        } => {
            // Determine which category filter to apply
            let cat = if dev {
                Some("dev")
            } else if apps {
                Some("apps")
            } else if desktop {
                Some("desktop")
            } else if hardware {
                Some("hardware")
            } else {
                category.as_deref()
            };

            cmd::check::run(cat).await?;
        }
    }

    Ok(())
}
