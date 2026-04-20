//! Interactive menu shown when `cheni` is invoked with no subcommand.
//!
//! Displays a short status line then a keyboard-navigable list of actions.
//! Selecting an action either runs the corresponding command directly or
//! prompts for any extra input it needs (package name, generation number,
//! search query, …).

use anyhow::Result;
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Input, Select};

use crate::nix::{config, pins};

use super::obsolete::count_obsolete_pins;

/// One entry in the interactive menu.
struct MenuEntry {
    /// Label shown in the picker.
    label: &'static str,
    /// Short description after the label.
    hint: &'static str,
    /// Action to dispatch when chosen.
    action: Action,
}

#[derive(Clone, Copy)]
enum Action {
    Check,
    PinPackage,
    PinFlakes,
    Unpin,
    Update,
    Upgrade,
    Build,
    History,
    Rollback,
    Diff,
    Prune,
    Search,
    Why,
    Status,
    Clean,
    Doctor,
    SelfUpdate,
    Init,
    Quit,
}

/// Run the interactive menu.
pub async fn run() -> Result<()> {
    print_banner()?;

    let entries = build_menu();
    let labels: Vec<String> = entries
        .iter()
        .map(|e| format!("{:<22} {}", e.label, e.hint.dimmed()))
        .collect();

    let theme = ColorfulTheme::default();
    let selection = Select::with_theme(&theme)
        .with_prompt("What do you want to do?")
        .items(&labels)
        .default(0)
        .interact_opt()?;

    let action = match selection {
        Some(idx) => entries[idx].action,
        None => return Ok(()), // ESC pressed
    };

    println!();
    dispatch(action).await
}

/// Print a one-line status snapshot above the menu.
fn print_banner() -> Result<()> {
    println!("{}", "=== cheni ===".bold());

    if let Ok(nix_config) = config::detect() {
        let pins = pins::read(&nix_config.flake_dir).unwrap_or_default();
        let lock_path = nix_config.flake_dir.join("flake.lock");
        let obsolete = if pins.is_empty() {
            0
        } else {
            count_obsolete_pins(&lock_path, &pins)
        };

        let pin_status = match (pins.len(), obsolete) {
            (0, _) => "no active pins".dimmed().to_string(),
            (n, 0) => format!("{} active pin(s)", n).green().to_string(),
            (n, o) => format!("{} active pin(s), {} obsolete", n, o)
                .yellow()
                .to_string(),
        };

        println!(
            "  {} {}    {} {}",
            "config:".dimmed(),
            nix_config.flake_dir.display(),
            "pins:".dimmed(),
            pin_status,
        );
    }
    println!();
    Ok(())
}

/// Static menu definition. Order = most common actions first.
fn build_menu() -> Vec<MenuEntry> {
    vec![
        MenuEntry {
            label: "Check",
            hint: "show available package + flake updates",
            action: Action::Check,
        },
        MenuEntry {
            label: "Update",
            hint: "refresh nixpkgs-latest + apply pinned updates",
            action: Action::Update,
        },
        MenuEntry {
            label: "Upgrade",
            hint: "full upgrade (all inputs, preview, build)",
            action: Action::Upgrade,
        },
        MenuEntry {
            label: "Pin package",
            hint: "pin a single package to nixpkgs-latest",
            action: Action::PinPackage,
        },
        MenuEntry {
            label: "Pin --flakes",
            hint: "update non-nixpkgs flake inputs",
            action: Action::PinFlakes,
        },
        MenuEntry {
            label: "Unpin",
            hint: "remove a single pin",
            action: Action::Unpin,
        },
        MenuEntry {
            label: "Build",
            hint: "rebuild without refreshing inputs",
            action: Action::Build,
        },
        MenuEntry {
            label: "History",
            hint: "list recent generations with diffs",
            action: Action::History,
        },
        MenuEntry {
            label: "Rollback",
            hint: "switch back to a previous generation",
            action: Action::Rollback,
        },
        MenuEntry {
            label: "Diff",
            hint: "compare two specific generations",
            action: Action::Diff,
        },
        MenuEntry {
            label: "Prune",
            hint: "pick old generations to delete",
            action: Action::Prune,
        },
        MenuEntry {
            label: "Search",
            hint: "search nixpkgs",
            action: Action::Search,
        },
        MenuEntry {
            label: "Why",
            hint: "find which .nix file declares a package",
            action: Action::Why,
        },
        MenuEntry {
            label: "Status",
            hint: "show config, active pins, flake input ages",
            action: Action::Status,
        },
        MenuEntry {
            label: "Clean",
            hint: "remove obsolete pins",
            action: Action::Clean,
        },
        MenuEntry {
            label: "Doctor",
            hint: "health checks on the cheni setup",
            action: Action::Doctor,
        },
        MenuEntry {
            label: "Self-update",
            hint: "refresh the cheni flake input + rebuild",
            action: Action::SelfUpdate,
        },
        MenuEntry {
            label: "Init",
            hint: "first-time setup of nixpkgs-latest in your flake",
            action: Action::Init,
        },
        MenuEntry {
            label: "Quit",
            hint: "exit without doing anything",
            action: Action::Quit,
        },
    ]
}

/// Run the action chosen from the menu, prompting for any extra input.
async fn dispatch(action: Action) -> Result<()> {
    let theme = ColorfulTheme::default();
    match action {
        Action::Check => super::check::run(None, false, false, false).await?,
        Action::Update => super::update::run()?,
        Action::Upgrade => super::upgrade::run(default_upgrade_options())?,
        Action::PinPackage => dispatch_pin_package(&theme).await?,
        Action::PinFlakes => super::pin::pin_flake_inputs().await?,
        Action::Unpin => dispatch_unpin(&theme)?,
        Action::Build => super::build::run()?,
        Action::History => super::history::run(default_history_options(false))?,
        Action::Prune => super::history::run(default_history_options(true))?,
        Action::Rollback => dispatch_rollback(&theme)?,
        Action::Diff => dispatch_diff(&theme)?,
        Action::Search => dispatch_search(&theme)?,
        Action::Why => dispatch_why(&theme)?,
        Action::Status => super::status::run()?,
        Action::Clean => super::clean::run()?,
        Action::Doctor => super::doctor::run()?,
        Action::SelfUpdate => super::self_update::run(false).await?,
        Action::Init => super::init::run()?,
        Action::Quit => {}
    }
    Ok(())
}

/// Upgrade defaults when launched from the interactive menu: no GC,
/// keep pins, ask for confirmation (yes=false). These mirror what the
/// user would get with a bare `cheni upgrade` from the shell.
fn default_upgrade_options() -> super::upgrade::UpgradeOptions {
    super::upgrade::UpgradeOptions {
        gc: false,
        no_clean_pins: false,
        yes: false,
    }
}

/// History defaults shared between the "history" and "prune" menu
/// entries — the only difference is the `prune` flag.
fn default_history_options(prune: bool) -> super::history::HistoryOptions {
    super::history::HistoryOptions {
        diff: false,
        full: false,
        limit: None,
        delete: Vec::new(),
        prune,
        keep: None,
        older_than: None,
        gc: false,
        yes: false,
    }
}

async fn dispatch_pin_package(theme: &ColorfulTheme) -> Result<()> {
    let name: String = Input::with_theme(theme)
        .with_prompt("Package to pin")
        .interact_text()?;
    let force: bool = dialoguer::Confirm::with_theme(theme)
        .with_prompt("Allow major version bump?")
        .default(false)
        .interact()?;
    super::pin::pin_one(&name, force).await
}

fn dispatch_unpin(theme: &ColorfulTheme) -> Result<()> {
    let name: String = Input::with_theme(theme)
        .with_prompt("Package to unpin")
        .interact_text()?;
    super::pin::unpin_one(&name, false)
}

fn dispatch_rollback(theme: &ColorfulTheme) -> Result<()> {
    let target: String = Input::with_theme(theme)
        .with_prompt("Generation number (empty = previous)")
        .allow_empty(true)
        .interact_text()?;
    let parsed = if target.trim().is_empty() {
        None
    } else {
        Some(target.trim().parse::<u32>()?)
    };
    super::rollback::run(parsed, false)
}

fn dispatch_diff(theme: &ColorfulTheme) -> Result<()> {
    let from: u32 = Input::with_theme(theme).with_prompt("From generation").interact_text()?;
    let to: u32 = Input::with_theme(theme).with_prompt("To generation").interact_text()?;
    super::diff::run(from, to)
}

fn dispatch_search(theme: &ColorfulTheme) -> Result<()> {
    let query: String = Input::with_theme(theme)
        .with_prompt("Search query")
        .interact_text()?;
    super::search::run(&query)
}

fn dispatch_why(theme: &ColorfulTheme) -> Result<()> {
    let package: String = Input::with_theme(theme)
        .with_prompt("Package name")
        .interact_text()?;
    super::why::run(&package)
}
