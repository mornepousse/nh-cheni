//! cheni — Granular package updates for NixOS.
//!
//! A CLI tool that lets you check, select, and apply updates
//! per-package on NixOS, integrated with your flake configuration.

mod api;
mod cmd;
mod http;
mod nix;
mod output;
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
Daily flow:\n  \
  cheni                          Interactive menu (state snapshot + action picker)\n  \
  cheni check                    See what's outdated (Repology + flake input ages)\n  \
  cheni check --pending          Add closure dry-run (kernel + base packages too)\n  \
  cheni upgrade                  Full upgrade: refresh, preview, rebuild\n  \
  cheni upgrade --boot           Stage for next boot (when nh refuses live switch)\n  \
  cheni status                   Where am I — config, pins, flake input ages\n\
\n\
Per-package policy:\n  \
  cheni pin <pkg>                Pin to nixpkgs-latest (get a newer version)\n  \
  cheni pin -c <category>        Pin all minor updates in modules/<category>/\n  \
  cheni pin --flakes             Update flake inputs (zen-browser, claude-code, …)\n  \
  cheni freeze <pkg>             Hold at current version (inverse of pin)\n  \
  cheni unpin <pkg>              Release a pin (or --all)\n  \
  cheni unfreeze <pkg>           Release a freeze (or --all)\n  \
  cheni clean                    Remove obsolete pins (nixpkgs caught up)\n\
\n\
Build vs upgrade (cheat sheet):\n  \
  build                  =  rebuild with whatever's already in flake.lock\n  \
  upgrade --pins-only    =  refresh nixpkgs-latest only, then rebuild (apply pins)\n  \
  upgrade                =  refresh every flake input, preview, then rebuild\n  \
  upgrade --boot         =  same as upgrade but stages for next boot, no live switch\n\
\n\
History & rollback:\n  \
  cheni history                  List recent generations with diffs\n  \
  cheni history --diff           Full per-package nvd output between gens\n  \
  cheni rollback [N]             Switch to previous gen (or to specific N)\n  \
  cheni diff <from> <to>         Compare two generations\n  \
  cheni history --prune          Delete generations interactively\n  \
  cheni history --keep N         Keep N most recent (delete the rest)\n  \
  cheni history --older-than 30d Delete by age\n\
\n\
Discovery:\n  \
  cheni search <query>           nixpkgs search + Repology + pin/freeze badges\n  \
  cheni why <pkg>                Which .nix file declares this?\n\
\n\
Maintenance:\n  \
  cheni doctor                   Health check (paths, lock, pins, freezes, age)\n  \
  cheni self-update              Update cheni itself (auto-bumps tag pin)\n  \
  cheni verify [tag]             Verify installed cheni signature\n  \
  cheni diagnose [file]          Explain a rebuild log\n\
\n\
Environment:\n  \
  CHENI_CONFIG=<path>            Override the NixOS flake directory\n  \
  CHENI_HTTP_TIMEOUT=<secs>      Per-request HTTP timeout (default 30, min 5)\n  \
  NO_COLOR=1                     Disable coloured output"
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

        /// Append a closure dry-run section: what derivations would
        /// change at the next rebuild (kernel + base system included,
        /// not just module-named packages). Adds 30–60s of evaluation.
        #[arg(long)]
        pending: bool,
    },

    /// Pin a package (or list active pins when called with no arguments)
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

        /// Skip the confirmation prompt
        #[arg(short, long)]
        yes: bool,

        /// Remove all pins at once
        #[arg(short = 'a', long)]
        all: bool,
    },

    /// Hold a package at its current version (inverse of `pin`: freezes ≠ pins)
    Freeze {
        /// Package name to freeze. Omit to list current freezes.
        package: Option<String>,

        /// Track the latest `MAJOR.y.z` instead of locking one specific
        /// version. With `--major 9`, `cheni upgrade` will bump the
        /// frozen rev to today's nixpkgs as long as upstream is still
        /// on major 9, and hold it once upstream moves to 10.
        #[arg(long, value_name = "N")]
        major: Option<u32>,
    },

    /// Release a frozen package (or all freezes with --all)
    Unfreeze {
        /// Package name to unfreeze
        package: Option<String>,

        /// Skip the confirmation prompt
        #[arg(short, long)]
        yes: bool,

        /// Remove every freeze at once
        #[arg(short = 'a', long)]
        all: bool,
    },

    /// Full system upgrade: refresh flake inputs, preview, rebuild, clean pins
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

        /// Refresh ONLY nixpkgs-latest (the pin overlay source) instead of every input.
        /// Replaces the old `cheni update` workflow.
        #[arg(long)]
        pins_only: bool,

        /// Stage the new generation for next boot (nh os boot) instead of
        /// live-switching. Required when a critical component is changing
        /// (dbus → dbus-broker, init swap, …). cheni auto-detects the
        /// case and offers to flip to boot mode interactively.
        #[arg(long)]
        boot: bool,
    },

    /// Rebuild the current flake state, no input refresh
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
        /// Skip the confirmation prompt
        #[arg(short, long)]
        yes: bool,
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

    /// Internal: print completion candidates for a kind (pins,
    /// freezes, generations, categories). Used by the shell
    /// completion scripts emitted by `cheni completion`.
    ///
    /// Hidden from `--help` so it stays an implementation detail —
    /// the contract is: one candidate per line on stdout, exit 0
    /// even when nothing matches (empty list).
    #[command(hide = true, name = "__complete")]
    Complete {
        /// Kind of completion to print: pins, freezes, generations,
        /// or categories.
        kind: String,
    },
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
        Commands::Check { category, details, json, refresh, pending } => {
            cmd::check::run(category.as_deref(), details, json, refresh, pending).await
        }
        Commands::Pin { package, category, flakes, force } => {
            dispatch_pin(package, category, flakes, force).await
        }
        Commands::Unpin { package, all, yes } => dispatch_unpin(package, all, yes),
        Commands::Freeze { package, major } => dispatch_freeze(package, major),
        Commands::Unfreeze { package, all, yes } => dispatch_unfreeze(package, all, yes),
        Commands::Upgrade { gc, no_clean_pins, yes, pins_only, boot } => {
            cmd::upgrade::run(cmd::upgrade::UpgradeOptions {
                gc,
                no_clean_pins,
                yes,
                pins_only,
                boot,
            })
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
        Commands::Rollback { target, yes } => cmd::rollback::run(target, yes),
        Commands::Diff { from, to } => cmd::diff::run(from, to),
        Commands::Search { query } => cmd::search::run(&query).await,
        Commands::Why { package } => cmd::why::run(&package),
        Commands::Clean => cmd::clean::run(),
        Commands::Init => cmd::init::run(),
        Commands::Status => cmd::status::run(),
        Commands::BugReport => cmd::bug_report::run(),
        Commands::Completion { shell } => emit_completion(shell),
        Commands::Man => emit_man_page(),
        Commands::Complete { kind } => emit_complete_candidates(&kind),
    }
}

/// Helper for the shell completion scripts: print candidates for a
/// given kind, one per line. Stays cheap (no network, no eval) so the
/// shell can call it on every Tab without lag. Always exits 0; an
/// empty list is valid output (a `cheni unpin` against a config with
/// no pins shouldn't error out the shell).
fn emit_complete_candidates(kind: &str) -> Result<()> {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let cfg = nix::config::detect().ok();
    match kind {
        "pins" => {
            if let Some(cfg) = &cfg {
                if let Ok(pins) = nix::pins::read(&cfg.flake_dir) {
                    for p in pins {
                        let _ = writeln!(out, "{}", p);
                    }
                }
            }
        }
        "freezes" => {
            if let Some(cfg) = &cfg {
                if let Ok(map) = nix::freezes::read(&cfg.flake_dir) {
                    for k in map.keys() {
                        let _ = writeln!(out, "{}", k);
                    }
                }
            }
        }
        "generations" => {
            if let Ok(entries) = std::fs::read_dir("/nix/var/nix/profiles") {
                let mut nums: Vec<u32> = entries
                    .flatten()
                    .filter_map(|e| {
                        e.file_name().to_str().and_then(|n| {
                            n.strip_prefix("system-")?
                                .strip_suffix("-link")?
                                .parse::<u32>()
                                .ok()
                        })
                    })
                    .collect();
                // Newest first — most rollback / diff invocations
                // target a recent gen, so put those at the top of
                // the completion menu.
                nums.sort_unstable_by(|a, b| b.cmp(a));
                for n in nums {
                    let _ = writeln!(out, "{}", n);
                }
            }
        }
        "categories" => {
            if let Some(cfg) = &cfg {
                for c in nix::config::list_module_categories(&cfg.flake_dir) {
                    let _ = writeln!(out, "{}", c);
                }
            }
        }
        _ => {} // unknown kind: empty output, exit 0
    }
    Ok(())
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
        // No selector — list current pins so `cheni pin` on its own
        // answers "what's pinned right now?" without having to grep
        // through package-pins.json or run `cheni status`.
        cmd::pin::list_pins()
    }
}

fn dispatch_unpin(package: Option<String>, all: bool, yes: bool) -> Result<()> {
    if all {
        cmd::pin::unpin_all(yes)
    } else if let Some(name) = package {
        cmd::pin::unpin_one(&name, yes)
    } else {
        anyhow::bail!(
            "Specify a package name or --all.\n\
             Usage: cheni unpin <package>\n\
             Usage: cheni unpin --all"
        );
    }
}

/// `cheni freeze` — one arg selects freeze-a-package, no arg lists freezes.
/// Matches `cheni pin`'s empty-arg listing behaviour so the two commands
/// feel like a matched pair.
fn dispatch_freeze(package: Option<String>, major: Option<u32>) -> Result<()> {
    match package {
        Some(name) => cmd::freeze::freeze_one(&name, major),
        None => cmd::freeze::list_freezes(),
    }
}

/// `cheni unfreeze` — same shape as `dispatch_unpin`.
fn dispatch_unfreeze(package: Option<String>, all: bool, yes: bool) -> Result<()> {
    if all {
        cmd::unfreeze::unfreeze_all(yes)
    } else if let Some(name) = package {
        cmd::unfreeze::unfreeze_one(&name, yes)
    } else {
        anyhow::bail!(
            "Specify a package name or --all.\n\
             Usage: cheni unfreeze <package>\n\
             Usage: cheni unfreeze --all"
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

    // For zsh we post-process clap_complete's output to swap the
    // `_default` fallback completer at specific positional-arg
    // hooks for our own dynamic completers (pinned package names,
    // generation numbers, module categories, frozen package
    // names). This lets `cheni unpin <Tab>` actually list the
    // user's pins, `cheni rollback <Tab>` list available
    // generations, and so on — turning what was static-only flag
    // completion into a useful daily-driver tool.
    if matches!(shell, clap_complete::Shell::Zsh) {
        let patched = augment_zsh_completion(std::str::from_utf8(&buf).unwrap_or(""));
        return write_ignoring_broken_pipe(patched.as_bytes());
    }

    write_ignoring_broken_pipe(&buf)
}

/// Inject dynamic completers into clap_complete's zsh output.
///
/// Each substitution swaps the trailing `:_default` (clap_complete's
/// generic fallback) for a `:_cheni_complete_<kind>` function we
/// define in the appended block at the end of the file. The
/// substitutions are deliberately literal-string-based: clap_complete's
/// position-spec format is stable enough that an exact match is
/// safer than a regex that could match unrelated lines.
///
/// New entries here pair with new function names in
/// [`zsh_dynamic_completers_block`] — the two stay in lockstep so a
/// missed pairing surfaces at runtime as "no completion offered" not
/// as a malformed script.
fn augment_zsh_completion(src: &str) -> String {
    let substitutions: &[(&str, &str)] = &[
        // Generation-number positions (rollback / diff / history --delete).
        (
            "'::target -- Generation number to roll back to (omit for the previous generation):_default'",
            "'::target -- Generation number to roll back to (omit for the previous generation):_cheni_complete_generations'",
        ),
        (
            "':from -- Source generation number:_default'",
            "':from -- Source generation number:_cheni_complete_generations'",
        ),
        (
            "':to -- Target generation number:_default'",
            "':to -- Target generation number:_cheni_complete_generations'",
        ),
        (
            "'*--delete=[Delete the listed generations (numbers or ranges, e.g. \"405\" \"400..410\")]:TARGET:_default'",
            "'*--delete=[Delete the listed generations (numbers or ranges, e.g. \"405\" \"400..410\")]:TARGET:_cheni_complete_generations'",
        ),
        // Pin / freeze positional-arg positions.
        (
            "'::package -- Package name to unpin:_default'",
            "'::package -- Package name to unpin:_cheni_complete_pins'",
        ),
        (
            "'::package -- Package name to unfreeze:_default'",
            "'::package -- Package name to unfreeze:_cheni_complete_freezes'",
        ),
        // `cheni freeze <pkg>` — completes from already-installed
        // names. We hand the freezes file too as a fallback for
        // re-freezing after unfreeze, but the primary completion
        // source is the store. The current `pins` helper would be
        // wrong (these are different sets), so fall back to the
        // freezes set for now — same result for the round-trip
        // unfreeze→freeze pattern.
        (
            "'::package -- Package name to freeze. Omit to list current freezes:_default'",
            "'::package -- Package name to freeze. Omit to list current freezes:_cheni_complete_freezes'",
        ),
        // Module-category arguments.
        (
            "'-c+[Restrict the scan to a single module category (auto-detected from modules/ — e.g. \"dev\", \"apps\", or any subdirectory name)]:CATEGORY:_default'",
            "'-c+[Restrict the scan to a single module category (auto-detected from modules/ — e.g. \"dev\", \"apps\", or any subdirectory name)]:CATEGORY:_cheni_complete_categories'",
        ),
        (
            "'--category=[Restrict the scan to a single module category (auto-detected from modules/ — e.g. \"dev\", \"apps\", or any subdirectory name)]:CATEGORY:_default'",
            "'--category=[Restrict the scan to a single module category (auto-detected from modules/ — e.g. \"dev\", \"apps\", or any subdirectory name)]:CATEGORY:_cheni_complete_categories'",
        ),
        (
            "'-c+[Pin all minor updates in a module category (e.g. \"dev\" → modules/dev/)]:CATEGORY:_default'",
            "'-c+[Pin all minor updates in a module category (e.g. \"dev\" → modules/dev/)]:CATEGORY:_cheni_complete_categories'",
        ),
        (
            "'--category=[Pin all minor updates in a module category (e.g. \"dev\" → modules/dev/)]:CATEGORY:_default'",
            "'--category=[Pin all minor updates in a module category (e.g. \"dev\" → modules/dev/)]:CATEGORY:_cheni_complete_categories'",
        ),
    ];

    let mut out = src.to_string();
    for (needle, replacement) in substitutions {
        out = out.replace(needle, replacement);
    }
    out.push('\n');
    out.push_str(zsh_dynamic_completers_block());
    out
}

/// zsh function block appended at the end of the completion script.
/// Each `_cheni_complete_<kind>` shells out to `cheni __complete <kind>`
/// and feeds the resulting one-name-per-line output to `_describe`.
/// The shell-out is cheap (no eval, no network) so per-Tab latency
/// stays imperceptible.
fn zsh_dynamic_completers_block() -> &'static str {
    "\n\
# --- cheni dynamic completion helpers (post-pended by `cheni completion zsh`) ---\n\
\n\
_cheni_complete_pins() {\n\
    local -a pins\n\
    pins=(${(f)\"$(cheni __complete pins 2>/dev/null)\"})\n\
    _describe 'pinned package' pins\n\
}\n\
\n\
_cheni_complete_freezes() {\n\
    local -a freezes\n\
    freezes=(${(f)\"$(cheni __complete freezes 2>/dev/null)\"})\n\
    _describe 'frozen package' freezes\n\
}\n\
\n\
_cheni_complete_generations() {\n\
    local -a gens\n\
    gens=(${(f)\"$(cheni __complete generations 2>/dev/null)\"})\n\
    _describe 'generation' gens\n\
}\n\
\n\
_cheni_complete_categories() {\n\
    local -a cats\n\
    cats=(${(f)\"$(cheni __complete categories 2>/dev/null)\"})\n\
    _describe 'module category' cats\n\
}\n"
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
