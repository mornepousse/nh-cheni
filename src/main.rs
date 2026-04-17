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
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Disable colored output
    #[arg(long, global = true)]
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

    /// Pin a package to nixpkgs-latest for update
    Pin {
        /// Package name to pin (omit to use --dev/--apps/etc.)
        package: Option<String>,

        /// Pin all minor updates in modules/dev/
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

        /// Allow pinning major version updates
        #[arg(long)]
        force: bool,
    },

    /// Remove a package pin
    Unpin {
        /// Package name to unpin (omit with --all to clear all)
        package: Option<String>,

        /// Remove all pins
        #[arg(long)]
        all: bool,
    },

    /// Apply pinned updates (update nixpkgs-latest + rebuild)
    Update,

    /// Show current status (config, active pins)
    Status,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Respect NO_COLOR environment variable (https://no-color.org/)
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
            dev, apps, desktop, hardware, category,
        } => {
            let cat = resolve_category(dev, apps, desktop, hardware, category);
            cmd::check::run(cat.as_deref()).await?;
        }

        Commands::Pin {
            package, dev, apps, desktop, hardware, category, force,
        } => {
            if let Some(name) = package {
                // Pin a single package
                cmd::pin::pin_one(&name, force).await?;
            } else {
                // Pin by category
                let cat = resolve_category(dev, apps, desktop, hardware, category);
                match cat {
                    Some(c) => cmd::pin::pin_category(&c, force).await?,
                    None => {
                        anyhow::bail!(
                            "Specify a package name or a category.\n\
                             Usage: nixup pin <package>\n\
                             Usage: nixup pin --dev"
                        );
                    }
                }
            }
        }

        Commands::Unpin { package, all } => {
            if all {
                cmd::pin::unpin_all()?;
            } else if let Some(name) = package {
                cmd::pin::unpin_one(&name)?;
            } else {
                anyhow::bail!(
                    "Specify a package name or --all.\n\
                     Usage: nixup unpin <package>\n\
                     Usage: nixup unpin --all"
                );
            }
        }

        Commands::Update => {
            cmd::update::run()?;
        }

        Commands::Status => {
            cmd::status::run()?;
        }
    }

    Ok(())
}

/// Resolve category flags into a single Option<String>.
fn resolve_category(
    dev: bool,
    apps: bool,
    desktop: bool,
    hardware: bool,
    custom: Option<String>,
) -> Option<String> {
    if dev {
        Some("dev".to_string())
    } else if apps {
        Some("apps".to_string())
    } else if desktop {
        Some("desktop".to_string())
    } else if hardware {
        Some("hardware".to_string())
    } else {
        custom
    }
}
