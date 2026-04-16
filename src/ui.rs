use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Gauge, Paragraph, Row, Table,
};

use crate::app::{App, InputMode, ViewMode};
use crate::types::UpdateStatus;

/// Couleurs utilisées dans l'interface
const COLOR_UP_TO_DATE: Color = Color::Green;
const COLOR_UPDATE: Color = Color::Yellow;
const COLOR_NEWER: Color = Color::Cyan;
const COLOR_UNKNOWN: Color = Color::DarkGray;
const COLOR_LOADING: Color = Color::DarkGray;
const COLOR_SELECTED: Color = Color::Cyan;
const COLOR_TITLE: Color = Color::White;
const COLOR_SEARCH_ACTIVE: Color = Color::Yellow;

/// Dessine l'interface complète
pub fn draw(frame: &mut Frame, app: &App) {
    // Découper l'écran en zones
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),  // Barre de titre
            Constraint::Length(3),  // Barre de recherche + progression
            Constraint::Min(5),    // Table des paquets
            Constraint::Length(1), // Barre d'aide
        ])
        .split(frame.area());

    draw_title_bar(frame, main_chunks[0], app);
    draw_search_bar(frame, main_chunks[1], app);
    draw_package_table(frame, main_chunks[2], app);
    draw_help_bar(frame, main_chunks[3], app);

    // Si la vue détaillée est ouverte, la dessiner par-dessus
    if app.show_detail {
        draw_detail_popup(frame, app);
    }
}

/// Barre de titre avec compteurs
fn draw_title_bar(frame: &mut Frame, area: Rect, app: &App) {
    let update_count = app.packages.iter()
        .filter(|p| p.status == UpdateStatus::UpdateAvailable)
        .count();

    let mode_label = match app.view_mode {
        ViewMode::All => "Tous",
        ViewMode::UpdatesOnly => "Mises a jour",
    };

    let title_text = format!(
        " nix-update-checker | {} paquets | {} mises a jour | Vue: {} ",
        app.filtered_indices.len(),
        update_count,
        mode_label,
    );

    let title = Paragraph::new(title_text)
        .style(Style::default().fg(COLOR_TITLE).bg(Color::DarkGray));

    frame.render_widget(title, area);
}

/// Barre de recherche et jauge de progression
fn draw_search_bar(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(60), // Recherche
            Constraint::Percentage(40), // Progression
        ])
        .split(area);

    // -- Barre de recherche --
    let search_style = match app.input_mode {
        InputMode::Search => Style::default().fg(COLOR_SEARCH_ACTIVE),
        InputMode::Normal => Style::default().fg(Color::White),
    };

    let search_label = match app.input_mode {
        InputMode::Search => " / Recherche: ",
        InputMode::Normal => {
            if app.filter_text.is_empty() {
                " Appuyer / pour chercher "
            } else {
                " Filtre: "
            }
        }
    };

    let search_text = format!("{}{}", search_label, app.filter_text);
    let search_block = Block::default().borders(Borders::ALL).title("Recherche");
    let search_paragraph = Paragraph::new(search_text)
        .style(search_style)
        .block(search_block);

    frame.render_widget(search_paragraph, chunks[0]);

    // Positionner le curseur si en mode recherche
    if app.input_mode == InputMode::Search {
        let cursor_x = chunks[0].x + search_label.len() as u16 + app.filter_text.len() as u16 + 1;
        let cursor_y = chunks[0].y + 1;
        frame.set_cursor_position((cursor_x, cursor_y));
    }

    // -- Jauge de progression --
    let progress = app.loading_progress();
    let progress_label = if app.is_loading_done() {
        format!("Termine ({}/{})", app.checked_count, app.total_count)
    } else {
        format!(
            "Chargement {}/{}  ({:.0}%)",
            app.checked_count, app.total_count, progress
        )
    };

    let gauge_color = if app.is_loading_done() {
        Color::Green
    } else {
        Color::Cyan
    };

    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title("Progression"))
        .gauge_style(Style::default().fg(gauge_color))
        .ratio(progress / 100.0)
        .label(progress_label);

    frame.render_widget(gauge, chunks[1]);
}

/// Table des paquets
fn draw_package_table(frame: &mut Frame, area: Rect, app: &App) {
    // En-tête du tableau
    let header_cells = ["Nom", "Installe", "Disponible", "Statut"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(COLOR_TITLE).add_modifier(Modifier::BOLD)));
    let header = Row::new(header_cells).height(1);

    // Lignes du tableau
    let rows: Vec<Row> = app.filtered_indices.iter()
        .map(|&idx| {
            let pkg = &app.packages[idx];

            // Couleur selon le statut
            let status_color = match pkg.status {
                UpdateStatus::UpToDate => COLOR_UP_TO_DATE,
                UpdateStatus::UpdateAvailable => COLOR_UPDATE,
                UpdateStatus::Newer => COLOR_NEWER,
                UpdateStatus::Unknown => COLOR_UNKNOWN,
                UpdateStatus::Loading => COLOR_LOADING,
            };

            // Texte du statut
            let status_text = match pkg.status {
                UpdateStatus::UpToDate => "OK",
                UpdateStatus::UpdateAvailable => "UPDATE",
                UpdateStatus::Newer => "NEWER",
                UpdateStatus::Unknown => "?",
                UpdateStatus::Loading => "...",
            };

            // Version disponible
            let available_text = match &pkg.latest_version {
                Some(v) => v.as_str(),
                None => "-",
            };

            let cells = vec![
                Cell::from(pkg.name.clone()),
                Cell::from(pkg.installed_version.clone()),
                Cell::from(available_text.to_string()),
                Cell::from(status_text).style(Style::default().fg(status_color)),
            ];

            Row::new(cells)
        })
        .collect();

    // Largeurs des colonnes
    let widths = [
        Constraint::Percentage(35),
        Constraint::Percentage(25),
        Constraint::Percentage(25),
        Constraint::Percentage(15),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title("Paquets"))
        .row_highlight_style(
            Style::default()
                .fg(COLOR_SELECTED)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    // Utiliser un TableState pour le surlignage
    let mut table_state = ratatui::widgets::TableState::default();
    table_state.select(Some(app.selected));

    frame.render_stateful_widget(table, area, &mut table_state);
}

/// Barre d'aide en bas
fn draw_help_bar(frame: &mut Frame, area: Rect, app: &App) {
    let help_text = match app.input_mode {
        InputMode::Search => {
            vec![
                Span::styled(" Echap", Style::default().fg(Color::Yellow)),
                Span::raw(" quitter recherche  "),
                Span::styled("Entree", Style::default().fg(Color::Yellow)),
                Span::raw(" valider"),
            ]
        }
        InputMode::Normal => {
            vec![
                Span::styled(" j/k", Style::default().fg(Color::Yellow)),
                Span::raw(" naviguer  "),
                Span::styled("/", Style::default().fg(Color::Yellow)),
                Span::raw(" chercher  "),
                Span::styled("Tab", Style::default().fg(Color::Yellow)),
                Span::raw(" vue  "),
                Span::styled("Entree", Style::default().fg(Color::Yellow)),
                Span::raw(" details  "),
                Span::styled("q", Style::default().fg(Color::Yellow)),
                Span::raw(" quitter"),
            ]
        }
    };

    let help_line = Line::from(help_text);
    let help = Paragraph::new(help_line)
        .style(Style::default().bg(Color::DarkGray));

    frame.render_widget(help, area);
}

/// Popup de détail pour le paquet sélectionné
fn draw_detail_popup(frame: &mut Frame, app: &App) {
    let selected_pkg = match app.selected_package() {
        Some(p) => p,
        None => return,
    };

    // Calculer la zone du popup (centré, 60% de largeur, hauteur fixe)
    let area = frame.area();
    let popup_width = (area.width as f32 * 0.6) as u16;
    let popup_height = 12;
    let popup_x = (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    // Effacer la zone du popup
    frame.render_widget(Clear, popup_area);

    // Contenu du popup
    let status_text = match selected_pkg.status {
        UpdateStatus::UpToDate => "A jour",
        UpdateStatus::UpdateAvailable => "Mise a jour disponible",
        UpdateStatus::Newer => "Version plus recente qu'unstable",
        UpdateStatus::Unknown => "Inconnu",
        UpdateStatus::Loading => "Chargement...",
    };

    let status_color = match selected_pkg.status {
        UpdateStatus::UpToDate => COLOR_UP_TO_DATE,
        UpdateStatus::UpdateAvailable => COLOR_UPDATE,
        UpdateStatus::Newer => COLOR_NEWER,
        UpdateStatus::Unknown => COLOR_UNKNOWN,
        UpdateStatus::Loading => COLOR_LOADING,
    };

    let latest_text = selected_pkg.latest_version
        .as_deref()
        .unwrap_or("-");

    let description_text = selected_pkg.description
        .as_deref()
        .unwrap_or("Pas de description");

    let homepage_text = selected_pkg.homepage
        .as_deref()
        .unwrap_or("Pas de page d'accueil");

    let lines = vec![
        Line::from(vec![
            Span::styled("  Nom:         ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&selected_pkg.name),
        ]),
        Line::from(vec![
            Span::styled("  Installe:    ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&selected_pkg.installed_version),
        ]),
        Line::from(vec![
            Span::styled("  Disponible:  ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(latest_text),
        ]),
        Line::from(vec![
            Span::styled("  Statut:      ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(status_text, Style::default().fg(status_color)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Description: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(description_text),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Homepage:    ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(homepage_text),
        ]),
    ];

    let popup_title = format!(" {} ", selected_pkg.name);
    let popup = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(popup_title)
                .style(Style::default().bg(Color::Black)),
        );

    frame.render_widget(popup, popup_area);
}
