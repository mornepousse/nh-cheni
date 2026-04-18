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
Common workflows:\n  \
  cheni check                  See what's outdated (packages + flake inputs)\n  \
  cheni pin vivaldi            Pin a single package to nixpkgs-latest\n  \
  cheni pin --dev              Pin all minor updates in modules/dev/\n  \
  cheni pin --flakes           Update flake inputs (zen-browser, claude-code, ...)\n  \
  cheni update                 Refresh nixpkgs-latest + apply pinned updates\n  \
  cheni build                  Just rebuild (no input refresh, parses errors)\n  \
  cheni upgrade                Full upgrade: update all inputs, preview, build\n\
\n\
History & rollback:\n  \
  cheni history                List recent generations with package diffs\n  \
  cheni history --limit 30     Show more generations\n  \
  cheni history --diff         Show full per-package diff between generations\n  \
  cheni rollback               Roll back to the previous generation\n  \
  cheni rollback 405           Roll back to a specific generation\n  \
  cheni diff 405 409           Compare two generations\n  \
  cheni history --prune        Pick generations to delete interactively\n  \
  cheni history --delete 405 406  Delete specific generations\n  \
  cheni history --delete 400..410 Delete a range\n  \
  cheni history --keep 20      Keep only the 20 most recent\n  \
  cheni history --older-than 30d  Delete generations older than 30 days\n\
\n\
Discovery:\n  \
  cheni search firefox         Search nixpkgs\n  \
  cheni why kitty              Find which .nix file declares a package\n  \
  cheni status                 Show config path, active pins, flake inputs\n\
\n\
Maintenance:\n  \
  cheni clean                  Remove obsolete pins (caught up by nixpkgs)\n  \
  cheni doctor                 Health checks on the cheni setup\n  \
  cheni self-update            Update cheni itself"
)]
struct Cli {
    /// Increase verbosity (-v for debug, -vv for trace)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Disable colored output
    #[arg(long, global = true)]
    no_color: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Show available package updates (nixpkgs + flake inputs)
    #[command(alias = "ck")]
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
        /// Also list packages classified as "Newer" or "Unknown" so you
        /// can inspect why
        #[arg(long)]
        details: bool,
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

    /// Refresh nixpkgs-latest and rebuild the system (applies pending pins)
    #[command(alias = "up")]
    Update,

    /// Full system upgrade: update all flake inputs, preview, build, clean pins
    #[command(alias = "ug")]
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

    /// Build and switch the current configuration (no input refresh, parses nix errors)
    #[command(alias = "b")]
    Build,

    /// Remove obsolete pins whose nixpkgs version has caught up
    Clean,

    /// Run health checks on the cheni setup (paths, pins, flake, store access)
    Doctor,

    /// Update cheni itself (refresh the cheni flake input and rebuild)
    #[command(name = "self-update")]
    SelfUpdate,

    /// List system generations (or selectively delete them with --prune/--delete/--keep)
    #[command(alias = "h")]
    History {
        /// Show full per-package diff between generations (uses nvd if available)
        #[arg(long)]
        diff: bool,

        /// Show the full per-step package list, even when it overflows one line
        #[arg(short, long)]
        full: bool,

        /// Limit the number of generations shown (default: 10)
        #[arg(long, value_name = "N")]
        limit: Option<usize>,

        /// Delete the listed generations (numbers or ranges, e.g. "405" "400..410")
        #[arg(long, value_name = "TARGET", num_args = 1..)]
        delete: Vec<String>,

        /// Pick generations to delete from an interactive multi-select list
        #[arg(short, long)]
        prune: bool,

        /// Delete the oldest generations, keeping only the N most recent
        #[arg(long, value_name = "N")]
        keep: Option<usize>,

        /// Delete generations older than this duration (e.g. "30d", "2w", "6m")
        #[arg(long, value_name = "DURATION")]
        older_than: Option<String>,

        /// After deletion, run nix-collect-garbage to reclaim disk space
        #[arg(long)]
        gc: bool,

        /// Skip the deletion confirmation prompt
        #[arg(short, long)]
        yes: bool,
    },

    /// Roll back to the previous generation (or a specific one)
    #[command(alias = "rb")]
    Rollback {
        /// Generation number to roll back to (omit for the previous generation)
        target: Option<u32>,
    },

    /// Compare two specific generations (uses nvd if available)
    Diff {
        /// Source generation number
        from: u32,
        /// Target generation number
        to: u32,
    },

    /// Search nixpkgs for a package
    #[command(alias = "s")]
    Search {
        /// Search query (e.g. "firefox", "rust analyzer")
        query: String,
    },

    /// Find which .nix file in the config declares a given package
    Why {
        /// Package name to search for
        package: String,
    },

    /// First-time setup: add the nixpkgs-latest input + overlay to your flake
    Init,

    /// Show config path, active pins, and flake input ages
    #[command(alias = "st")]
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

    // No subcommand → launch the interactive menu (or print help if not a TTY).
    let command = match cli.command {
        Some(c) => c,
        None => {
            use std::io::IsTerminal;
            if std::io::stdout().is_terminal() && std::io::stdin().is_terminal() {
                cmd::interactive::run().await?;
                return Ok(());
            } else {
                use clap::CommandFactory;
                Cli::command().print_help()?;
                println!();
                return Ok(());
            }
        }
    };

    // Dispatch to the right command
    match command {
        Commands::Check {
            dev, apps, desktop, hardware, category, details,
        } => {
            let cat = resolve_category(dev, apps, desktop, hardware, category);
            cmd::check::run(cat.as_deref(), details).await?;
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

        Commands::SelfUpdate => {
            cmd::self_update::run()?;
        }

        Commands::History { diff, full, limit, delete, prune, keep, older_than, gc, yes } => {
            cmd::history::run(cmd::history::HistoryOptions {
                diff,
                full,
                limit,
                delete,
                prune,
                keep,
                older_than,
                gc,
                yes,
            })?;
        }

        Commands::Rollback { target } => {
            cmd::rollback::run(target)?;
        }

        Commands::Diff { from, to } => {
            cmd::diff::run(from, to)?;
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
