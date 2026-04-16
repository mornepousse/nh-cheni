mod api;
mod app;
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

/// Durée du tick pour le polling des événements (en millisecondes)
const TICK_RATE_MS: u64 = 50;

#[tokio::main]
async fn main() -> Result<()> {
    // Lire les paquets installés depuis le store
    let packages = store::read_installed_packages()
        .context("Erreur lors de la lecture des paquets installés")?;

    if packages.is_empty() {
        eprintln!("Aucun paquet trouvé dans le nix store.");
        return Ok(());
    }

    // Créer l'état de l'application
    let mut app = App::new(packages);

    // Canal pour recevoir les résultats de l'API
    let (tx, mut rx) = mpsc::unbounded_channel::<api::ApiResult>();

    // Collecter les noms de paquets pour les requêtes API
    let package_names: Vec<String> = app.packages.iter()
        .map(|p| p.name.clone())
        .collect();

    // Lancer la récupération des versions en arrière-plan
    tokio::spawn(async move {
        let _ = api::fetch_latest_versions(package_names, tx).await;
    });

    // Configurer le terminal
    let terminal = setup_terminal()
        .context("Erreur lors de la configuration du terminal")?;

    // Boucle principale avec gestion de la restauration du terminal
    let result = run_app(terminal, &mut app, &mut rx).await;

    // Restaurer le terminal quoi qu'il arrive
    restore_terminal()
        .context("Erreur lors de la restauration du terminal")?;

    result
}

/// Configure le terminal pour le mode TUI
fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode().context("Impossible d'activer le raw mode")?;

    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)
        .context("Impossible d'entrer dans l'écran alternatif")?;

    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)
        .context("Impossible de créer le terminal ratatui")?;

    // Installer un hook de panique pour restaurer le terminal
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = restore_terminal();
        original_hook(panic_info);
    }));

    Ok(terminal)
}

/// Restaure le terminal dans son état normal
fn restore_terminal() -> Result<()> {
    disable_raw_mode().context("Impossible de désactiver le raw mode")?;

    let mut stdout = io::stdout();
    execute!(stdout, LeaveAlternateScreen)
        .context("Impossible de quitter l'écran alternatif")?;

    Ok(())
}

/// Boucle principale de l'application
async fn run_app(
    mut terminal: Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    rx: &mut mpsc::UnboundedReceiver<api::ApiResult>,
) -> Result<()> {
    let tick_duration = Duration::from_millis(TICK_RATE_MS);

    loop {
        // Dessiner l'interface
        terminal.draw(|frame| ui::draw(frame, app))?;

        // Traiter les résultats de l'API qui sont arrivés
        while let Ok(result) = rx.try_recv() {
            app.apply_api_result(result);
        }

        // Vérifier si l'utilisateur veut quitter
        if app.should_quit {
            return Ok(());
        }

        // Attendre un événement clavier ou timeout
        let has_event = event::poll(tick_duration)
            .context("Erreur lors du polling des événements")?;

        if !has_event {
            continue;
        }

        let event = event::read().context("Erreur lors de la lecture d'un événement")?;

        // Traiter l'événement
        if let Event::Key(key) = event {
            handle_key_event(app, key);
        }
    }
}

/// Traite un événement clavier
fn handle_key_event(app: &mut App, key: event::KeyEvent) {
    match app.input_mode {
        InputMode::Normal => handle_normal_key(app, key),
        InputMode::Search => handle_search_key(app, key),
    }
}

/// Gère les touches en mode normal
fn handle_normal_key(app: &mut App, key: event::KeyEvent) {
    match key.code {
        // Quitter
        KeyCode::Char('q') => {
            app.should_quit = true;
        }
        // Ctrl+C pour quitter aussi
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        // Navigation vers le haut
        KeyCode::Up | KeyCode::Char('k') => {
            app.move_up();
        }
        // Navigation vers le bas
        KeyCode::Down | KeyCode::Char('j') => {
            app.move_down();
        }
        // Entrer en mode recherche
        KeyCode::Char('/') => {
            app.input_mode = InputMode::Search;
        }
        // Basculer la vue (tous / mises à jour seulement)
        KeyCode::Tab => {
            app.toggle_view_mode();
        }
        // Afficher/masquer les détails
        KeyCode::Enter => {
            app.toggle_detail();
        }
        // Fermer le popup de détails avec Escape
        KeyCode::Esc => {
            if app.show_detail {
                app.show_detail = false;
            }
        }
        _ => {}
    }
}

/// Gère les touches en mode recherche
fn handle_search_key(app: &mut App, key: event::KeyEvent) {
    match key.code {
        // Quitter la recherche
        KeyCode::Esc => {
            app.input_mode = InputMode::Normal;
        }
        // Valider la recherche
        KeyCode::Enter => {
            app.input_mode = InputMode::Normal;
        }
        // Effacer un caractère
        KeyCode::Backspace => {
            app.filter_text.pop();
            app.rebuild_filtered_list();
        }
        // Ajouter un caractère
        KeyCode::Char(c) => {
            app.filter_text.push(c);
            app.rebuild_filtered_list();
        }
        _ => {}
    }
}
