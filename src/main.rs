//! cheni — Granular package updates for NixOS.
//!
//! A CLI tool that lets you check, select, and apply updates
//! per-package on NixOS, integrated with your flake configuration.

mod api;
mod cmd;
mod nix;
mod release;
mod util;
mod version;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

/// Granular package updates for NixOS.
#[derive(Parser)]
#[command(
    name = "cheni",
    version = env!("GIT_DESCRIBE"),
    about = "Granular package updates for NixOS",
    long_about = "Granular package updates for NixOS.\n\n\
        cheni lets you check, select, and apply updates per-package\n\
        on NixOS, integrated with your flake configuration.\n\
        Packages are pinned to nixpkgs-latest for safe, incremental updates.",
    after_help = "\
Common workflows:\n  \
  cheni check                  See what's outdated (packages + flake inputs)\n  \
  cheni check -c dev           Restrict to modules/dev/\n  \
  cheni pin vivaldi            Pin a single package to nixpkgs-latest\n  \
  cheni pin -c dev             Pin all minor updates in modules/dev/\n  \
  cheni pin --flakes           Update flake inputs (zen-browser, claude-code, ...)\n  \
  cheni build                  Just rebuild the current flake state (old 'update' alias)\n  \
  cheni update                 Refresh nixpkgs-latest + apply pinned updates\n  \
  cheni upgrade                Full upgrade: update ALL inputs, preview, build (old 'upgrade')\n\
\n\
Build vs update vs upgrade — the short version:\n  \
  build      =  nothing fetched, just rebuild with what's already in flake.lock\n  \
  update     =  refresh nixpkgs-latest only, then rebuild  (applies pending pins)\n  \
  upgrade    =  refresh every flake input, preview, then rebuild\n\
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
  cheni self-update            Update cheni itself\n  \
  cheni verify                 Check the installed cheni against a signed release\n  \
  cheni diagnose [file]        Scan a rebuild log for known-issue hints\n\
\n\
Environment:\n  \
  CHENI_CONFIG=<path>          Override the NixOS flake directory\n  \
  CHENI_HTTP_TIMEOUT=<secs>    Per-request HTTP timeout (default 30, min 5)\n  \
  NO_COLOR=1                   Disable coloured output"
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
        /// Restrict the scan to a single module category (auto-detected
        /// from modules/ — e.g. "dev", "apps", or any subdirectory name).
        #[arg(short = 'c', long, value_name = "CATEGORY")]
        category: Option<String>,

        /// Also list packages classified as "Newer" or "Unknown" so you
        /// can inspect why
        #[arg(long)]
        details: bool,

        /// Machine-readable JSON output on stdout (disables spinners + colour)
        #[arg(long)]
        json: bool,

        /// Ignore the on-disk Repology cache and re-fetch every lookup
        #[arg(long)]
        refresh: bool,
    },

    /// Pin a package or category for update via nixpkgs-latest
    Pin {
        /// Package name to pin (e.g. "vivaldi", "zen-browser")
        package: Option<String>,

        /// Pin all minor updates in a module category (e.g. "dev" → modules/dev/)
        #[arg(short = 'c', long, value_name = "CATEGORY")]
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

    /// Refresh nixpkgs-latest + rebuild (applies pending pins — see 'build' for plain rebuild)
    #[command(alias = "up")]
    Update,

    /// Full system upgrade: refresh ALL flake inputs, preview, build, clean pins
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

    /// Rebuild the current flake state, no input refresh (equivalent of the old 'update' alias)
    #[command(alias = "b")]
    Build,

    /// Remove obsolete pins whose nixpkgs version has caught up
    Clean,

    /// Run health checks on the cheni setup (paths, pins, flake, store access)
    Doctor,

    /// Update cheni itself (refresh the cheni flake input and rebuild)
    #[command(name = "self-update")]
    SelfUpdate {
        /// Skip the minisign signature check. Use only when recovering
        /// from a broken release, a key rotation, or for local testing.
        #[arg(long)]
        allow_unsigned: bool,
    },

    /// Verify that the installed cheni matches a signed release
    Verify {
        /// Tag to verify (defaults to the installed version).
        #[arg(long)]
        tag: Option<String>,
    },

    /// Scan a build log (file or stdin) and surface known-issue hints
    Diagnose {
        /// Path to a log file. Reads from stdin when omitted.
        path: Option<std::path::PathBuf>,
    },

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

    /// Print a diagnostic report to paste into a GitLab issue
    #[command(name = "bug-report")]
    BugReport,

    /// Emit shell completions for bash / zsh / fish / elvish / powershell
    ///
    /// Pipe to your shell's completion dir, e.g.
    ///   cheni completion zsh  > ~/.zfunc/_cheni
    ///   cheni completion fish > ~/.config/fish/completions/cheni.fish
    Completion {
        /// Target shell
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },

    /// Emit a roff (groff) man page on stdout — pipe to a file, e.g.
    ///   cheni man > /usr/local/share/man/man1/cheni.1
    Man,
}

/// Install a panic hook that converts unexpected crashes into a friendly
/// message pointing at `cheni bug-report`. The full backtrace is still
/// available via RUST_BACKTRACE for anyone who sets it.
fn install_panic_hook() {
    // Match the format 'cheni --version' prints — using the resolved
    // git-describe output keeps crash reports tied to a real commit.
    let version = env!("GIT_DESCRIBE");
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!(" at {}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(|s| s.as_str()))
            .unwrap_or("<no message>");

        eprintln!();
        eprintln!("\x1b[31;1m✗ cheni crashed unexpectedly\x1b[0m");
        eprintln!();
        eprintln!("  Version: {}", version);
        eprintln!("  Error:   {}{}", payload, location);
        eprintln!();
        eprintln!("This is a bug. Please report it:");
        eprintln!("  1. Gather diagnostic info:  \x1b[1mcheni bug-report > report.md\x1b[0m");
        eprintln!("  2. Open an issue:            https://gitlab.com/harrael/cheni/-/issues/new");
        eprintln!();
        eprintln!("  (Set RUST_BACKTRACE=1 for a full backtrace.)");
    }));
}

#[tokio::main]
async fn main() -> Result<()> {
    install_panic_hook();
    let cli = Cli::parse();
    configure_runtime(&cli);

    let Some(command) = resolve_command(cli.command).await? else {
        return Ok(());
    };
    dispatch(command).await
}

/// Apply the effects of the global flags (-v, --no-color, NO_COLOR env)
/// and initialise tracing. Called exactly once at startup.
fn configure_runtime(cli: &Cli) {
    if cli.no_color || std::env::var("NO_COLOR").is_ok() {
        colored::control::set_override(false);
    }
    let filter = match cli.verbose {
        0 => "warn",
        1 => "cheni=debug",
        _ => "cheni=trace",
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter)),
        )
        .with_target(false)
        .without_time()
        .init();
}

/// Turn an optional `Commands` into the actual command to run, or None
/// if the program already produced its output (interactive menu or a
/// printed `--help`).
///
/// With no subcommand we drop into the interactive menu on a TTY and
/// fall back to `--help` when piped/scripted — the latter is the
/// behaviour users expect when they invoke cheni from a shell pipeline.
async fn resolve_command(cmd: Option<Commands>) -> Result<Option<Commands>> {
    if let Some(c) = cmd {
        return Ok(Some(c));
    }
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() && std::io::stdin().is_terminal() {
        cmd::interactive::run().await?;
    } else {
        use clap::CommandFactory;
        Cli::command().print_help()?;
        println!();
    }
    Ok(None)
}

/// Dispatch a resolved `Commands` to the right subcommand module. Kept
/// tight on purpose — it's the one-line-per-branch table of contents
/// for everything cheni can do.
async fn dispatch(command: Commands) -> Result<()> {
    match command {
        Commands::Check { category, details, json, refresh } => {
            cmd::check::run(category.as_deref(), details, json, refresh).await
        }
        Commands::Pin { package, category, flakes, force } => {
            dispatch_pin(package, category, flakes, force).await
        }
        Commands::Unpin { package, all } => dispatch_unpin(package, all),
        Commands::Update => cmd::update::run(),
        Commands::Upgrade { gc, no_clean_pins, yes } => {
            cmd::upgrade::run(cmd::upgrade::UpgradeOptions { gc, no_clean_pins, yes })
        }
        Commands::Build => cmd::build::run(),
        Commands::Doctor => cmd::doctor::run(),
        Commands::SelfUpdate { allow_unsigned } => cmd::self_update::run(allow_unsigned).await,
        Commands::Verify { tag } => cmd::verify::run(cmd::verify::VerifyOptions { tag }).await,
        Commands::Diagnose { path } => cmd::diagnose::run(cmd::diagnose::DiagnoseOptions { path }),
        Commands::History { diff, full, limit, delete, prune, keep, older_than, gc, yes } => {
            cmd::history::run(cmd::history::HistoryOptions {
                diff, full, limit, delete, prune, keep, older_than, gc, yes,
            })
        }
        Commands::Rollback { target } => cmd::rollback::run(target),
        Commands::Diff { from, to } => cmd::diff::run(from, to),
        Commands::Search { query } => cmd::search::run(&query),
        Commands::Why { package } => cmd::why::run(&package),
        Commands::Clean => cmd::clean::run(),
        Commands::Init => cmd::init::run(),
        Commands::Status => cmd::status::run(),
        Commands::BugReport => cmd::bug_report::run(),
        Commands::Completion { shell } => emit_completion(shell),
        Commands::Man => emit_man_page(),
    }
}

/// `cheni pin` — one of three mutually-exclusive modes plus a
/// hand-holding bail message when the user forgot the selector.
async fn dispatch_pin(
    package: Option<String>,
    category: Option<String>,
    flakes: bool,
    force: bool,
) -> Result<()> {
    if flakes {
        cmd::pin::pin_flake_inputs().await
    } else if let Some(name) = package {
        cmd::pin::pin_one(&name, force).await
    } else if let Some(cat) = category {
        cmd::pin::pin_category(&cat, force).await
    } else {
        anyhow::bail!(
            "Specify a package name, a category, or --flakes.\n\
             Usage: cheni pin <package>\n\
             Usage: cheni pin --category <name>     (e.g. dev, apps)\n\
             Usage: cheni pin --flakes"
        );
    }
}

fn dispatch_unpin(package: Option<String>, all: bool) -> Result<()> {
    if all {
        cmd::pin::unpin_all()
    } else if let Some(name) = package {
        cmd::pin::unpin_one(&name)
    } else {
        anyhow::bail!(
            "Specify a package name or --all.\n\
             Usage: cheni unpin <package>\n\
             Usage: cheni unpin --all"
        );
    }
}

fn emit_completion(shell: clap_complete::Shell) -> Result<()> {
    use clap::CommandFactory;
    let mut cmd = Cli::command();
    let bin_name = cmd.get_name().to_string();
    // Generate into a buffer first so we can handle BrokenPipe
    // gracefully when the output is piped into `head`, etc. — the
    // upstream `generate()` would panic otherwise.
    let mut buf = Vec::new();
    clap_complete::generate(shell, &mut cmd, bin_name, &mut buf);
    write_ignoring_broken_pipe(&buf)
}

fn emit_man_page() -> Result<()> {
    use clap::CommandFactory;
    let cmd = Cli::command();
    let mut buf = Vec::new();
    clap_mangen::Man::new(cmd).render(&mut buf)?;
    write_ignoring_broken_pipe(&buf)
}

fn write_ignoring_broken_pipe(buf: &[u8]) -> Result<()> {
    use std::io::Write;
    match std::io::stdout().write_all(buf) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(e) => Err(e.into()),
    }
}
