//! cheni — Granular package updates for NixOS.
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
#[command(
    name = "cheni",
    version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("GIT_SHORT_HASH"), ")"),
    about = "Granular package updates for NixOS",
    long_about = "Granular package updates for NixOS.\n\n\
        cheni lets you check, select, and apply updates per-package\n\
        on NixOS, integrated with your flake configuration.\n\
        Packages are pinned to nixpkgs-latest for safe, incremental updates.",
    after_help = "\
Quick start:\n  \
  cheni check          See available updates\n  \
  cheni pin vivaldi    Pin a single package\n  \
  cheni pin --dev      Pin all minor updates in modules/dev/\n  \
  cheni pin --flakes   Update flake inputs (zen-browser, claude-code, ...)\n  \
  cheni update         Apply pinned updates"
)]
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
    /// Show available package updates (nixpkgs + flake inputs)
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

    /// Pin a package or category for update via nixpkgs-latest
    Pin {
        /// Package name to pin (e.g. "vivaldi", "zen-browser")
        package: Option<String>,

        /// Pin all minor updates in modules/dev/
        #[arg(long)]
        dev: bool,
        /// Pin all minor updates in modules/apps/
        #[arg(long)]
        apps: bool,
        /// Pin all minor updates in modules/desktop/
        #[arg(long)]
        desktop: bool,
        /// Pin all minor updates in modules/hardware/
        #[arg(long)]
        hardware: bool,
        /// Custom category name
        #[arg(long, value_name = "CATEGORY")]
        category: Option<String>,

        /// Check and update flake inputs (zen-browser, claude-code, ...)
        #[arg(long)]
        flakes: bool,

        /// Allow pinning major version updates
        #[arg(long)]
        force: bool,
    },

    /// Remove a package pin (or all pins with --all)
    Unpin {
        /// Package name to unpin
        package: Option<String>,

        /// Remove all pins at once
        #[arg(long)]
        all: bool,
    },

    /// Apply pinned updates: refresh nixpkgs-latest and rebuild the system
    Update,

    /// Full system upgrade: update all inputs, preview, rebuild, clean pins
    Upgrade {
        /// Also run garbage collection (DELETES old generations — no rollback!)
        #[arg(long)]
        gc: bool,

        /// Skip cleanup of obsolete pins
        #[arg(long)]
        no_clean_pins: bool,

        /// Skip the preview + confirmation step (non-interactive)
        #[arg(short, long)]
        yes: bool,
    },

    /// Build and switch with human-readable error parsing
    Build,

    /// Remove obsolete pins whose nixpkgs version has caught up
    Clean,

    /// Run health checks on your cheni setup
    Doctor,

    /// Search nixpkgs for a package
    Search {
        /// Search query
        query: String,
    },

    /// Find which .nix file declares a package
    Why {
        /// Package name to search for
        package: String,
    },

    /// First-time setup: add nixpkgs-latest input to your flake
    Init,

    /// Show current status: config path, active pins, and flake inputs
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
        1 => "cheni=debug",
        _ => "cheni=trace",
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
            package, dev, apps, desktop, hardware, category, flakes, force,
        } => {
            if flakes {
                // Mettre à jour les flake inputs
                cmd::pin::pin_flake_inputs().await?;
            } else if let Some(name) = package {
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
                             Usage: cheni pin <package>\n\
                             Usage: cheni pin --dev\n\
                             Usage: cheni pin --flakes"
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
                     Usage: cheni unpin <package>\n\
                     Usage: cheni unpin --all"
                );
            }
        }

        Commands::Update => {
            cmd::update::run()?;
        }

        Commands::Upgrade { gc, no_clean_pins, yes } => {
            cmd::upgrade::run(cmd::upgrade::UpgradeOptions {
                gc,
                no_clean_pins,
                yes,
            })?;
        }

        Commands::Build => {
            cmd::build::run()?;
        }

        Commands::Doctor => {
            cmd::doctor::run()?;
        }

        Commands::Search { query } => {
            cmd::search::run(&query)?;
        }

        Commands::Why { package } => {
            cmd::why::run(&package)?;
        }

        Commands::Clean => {
            cmd::clean::run()?;
        }

        Commands::Init => {
            cmd::init::run()?;
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
