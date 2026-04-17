mod api;
mod app;
mod pins;
mod store;
mod types;
mod ui;

use std::io;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::execute;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;

use app::{App, InputMode};

/// Tick duration for event polling (in milliseconds)
const TICK_RATE_MS: u64 = 50;

#[tokio::main]
async fn main() -> Result<()> {
    // Read installed packages from the store
    let packages = store::read_installed_packages()
        .context("Error reading installed packages")?;

    if packages.is_empty() {
        eprintln!("No packages found in the nix store.");
        return Ok(());
    }

    // Create the application state
    let mut app = App::new(packages);

    // Channel to receive API results
    let (tx, mut rx) = mpsc::unbounded_channel::<api::ApiResult>();

    // Collect package names for API requests
    let package_names: Vec<String> = app.packages.iter()
        .map(|p| p.name.clone())
        .collect();

    // Spawn background version fetching
    tokio::spawn(async move {
        let _ = api::fetch_latest_versions(package_names, tx).await;
    });

    // Set up the terminal
    let terminal = setup_terminal()
        .context("Error setting up the terminal")?;

    // Main loop with terminal restoration handling
    let result = run_app(terminal, &mut app, &mut rx).await;

    // Restore the terminal no matter what
    restore_terminal()
        .context("Error restoring the terminal")?;

    result
}

/// Set up the terminal for TUI mode
fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode().context("Unable to enable raw mode")?;

    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)
        .context("Unable to enter alternate screen")?;

    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)
        .context("Unable to create ratatui terminal")?;

    // Install a panic hook to restore the terminal
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = restore_terminal();
        original_hook(panic_info);
    }));

    Ok(terminal)
}

/// Restore the terminal to its normal state
fn restore_terminal() -> Result<()> {
    disable_raw_mode().context("Unable to disable raw mode")?;

    let mut stdout = io::stdout();
    execute!(stdout, LeaveAlternateScreen)
        .context("Unable to leave alternate screen")?;

    Ok(())
}

/// Main application loop
async fn run_app(
    mut terminal: Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    rx: &mut mpsc::UnboundedReceiver<api::ApiResult>,
) -> Result<()> {
    let tick_duration = Duration::from_millis(TICK_RATE_MS);

    loop {
        // Draw the interface
        terminal.draw(|frame| ui::draw(frame, app))?;

        // Process API results that have arrived
        while let Ok(result) = rx.try_recv() {
            app.apply_api_result(result);
        }

        // Check if the user wants to quit
        if app.should_quit {
            return Ok(());
        }

        // Process pending updates
        if !app.pending_updates.is_empty() {
            let names = std::mem::take(&mut app.pending_updates);

            // Restore the terminal to show nix commands
            restore_terminal()?;

            let success = pins::update_packages(&names);

            // Re-setup the terminal
            enable_raw_mode()?;
            execute!(io::stdout(), EnterAlternateScreen)?;
            terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

            if success {
                app.update_message = Some(format!("{} package(s) updated!", names.len()));
                for pkg_name in &names {
                    if let Some(pkg) = app.packages.iter_mut().find(|p| &p.name == pkg_name) {
                        pkg.status = types::UpdateStatus::UpToDate;
                        if let Some(ref latest) = pkg.latest_version {
                            pkg.installed_version = latest.clone();
                        }
                    }
                }
                app.selected_for_update.clear();
                app.rebuild_filtered_list();
            } else {
                app.update_message = Some("Update failed".to_string());
            }
        }

        // Wait for a keyboard event or timeout
        let has_event = event::poll(tick_duration)
            .context("Error polling events")?;

        if !has_event {
            continue;
        }

        let event = event::read().context("Error reading event")?;

        // Process the event
        if let Event::Key(key) = event {
            handle_key_event(app, key);
        }
    }
}

/// Handle a keyboard event
fn handle_key_event(app: &mut App, key: event::KeyEvent) {
    match app.input_mode {
        InputMode::Normal => handle_normal_key(app, key),
        InputMode::Search => handle_search_key(app, key),
    }
}

/// Handle keys in normal mode
fn handle_normal_key(app: &mut App, key: event::KeyEvent) {
    match key.code {
        // Quit
        KeyCode::Char('q') => {
            app.should_quit = true;
        }
        // Ctrl+C to quit as well
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        // Navigate up
        KeyCode::Up | KeyCode::Char('k') => {
            app.move_up();
        }
        // Navigate down
        KeyCode::Down | KeyCode::Char('j') => {
            app.move_down();
        }
        // Enter search mode
        KeyCode::Char('/') => {
            app.input_mode = InputMode::Search;
        }
        // Toggle view (all / updates only)
        KeyCode::Tab => {
            app.toggle_view_mode();
        }
        // Show/hide details
        KeyCode::Enter => {
            app.toggle_detail();
        }
        // Select/deselect package for update (minor only)
        KeyCode::Char(' ') => {
            app.toggle_selected_for_update();
        }
        // Force select major update
        KeyCode::Char('U') => {
            app.toggle_selected_force();
        }
        // Update all selected packages (or the current one if nothing selected)
        KeyCode::Char('u') => {
            let names = app.get_selected_update_names();
            if !names.is_empty() {
                app.pending_updates = names;
            } else if let Some(pkg) = app.selected_package() {
                if pkg.status == types::UpdateStatus::UpdateAvailable {
                    app.pending_updates = vec![pkg.name.clone()];
                }
            }
        }
        // Close the detail popup with Escape
        KeyCode::Esc => {
            if app.show_detail {
                app.show_detail = false;
            } else if app.update_message.is_some() {
                app.update_message = None;
            }
        }
        _ => {}
    }
}

/// Handle keys in search mode
fn handle_search_key(app: &mut App, key: event::KeyEvent) {
    match key.code {
        // Exit search
        KeyCode::Esc => {
            app.input_mode = InputMode::Normal;
        }
        // Confirm search
        KeyCode::Enter => {
            app.input_mode = InputMode::Normal;
        }
        // Delete a character
        KeyCode::Backspace => {
            app.filter_text.pop();
            app.rebuild_filtered_list();
        }
        // Add a character
        KeyCode::Char(c) => {
            app.filter_text.push(c);
            app.rebuild_filtered_list();
        }
        _ => {}
    }
}
