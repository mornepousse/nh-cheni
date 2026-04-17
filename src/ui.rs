use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Gauge, Paragraph, Row, Table,
};

use crate::app::{App, InputMode, ViewMode};
use crate::types::UpdateStatus;

/// Colors used in the interface
const COLOR_UP_TO_DATE: Color = Color::Green;
const COLOR_UPDATE: Color = Color::Yellow;
const COLOR_MAJOR: Color = Color::Red;
const COLOR_NEWER: Color = Color::Cyan;
const COLOR_UNKNOWN: Color = Color::DarkGray;
const COLOR_LOADING: Color = Color::DarkGray;
const COLOR_SELECTED: Color = Color::Cyan;
const COLOR_TITLE: Color = Color::White;
const COLOR_SEARCH_ACTIVE: Color = Color::Yellow;

/// Draw the full interface
pub fn draw(frame: &mut Frame, app: &App) {
    // Split the screen into zones
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),  // Title bar
            Constraint::Length(3),  // Search bar + progress
            Constraint::Min(5),    // Package table
            Constraint::Length(1), // Help bar
        ])
        .split(frame.area());

    draw_title_bar(frame, main_chunks[0], app);
    draw_search_bar(frame, main_chunks[1], app);
    draw_package_table(frame, main_chunks[2], app);
    draw_help_bar(frame, main_chunks[3], app);

    // If the detail view is open, draw it on top
    if app.show_detail {
        draw_detail_popup(frame, app);
    }
}

/// Title bar with counters
fn draw_title_bar(frame: &mut Frame, area: Rect, app: &App) {
    let update_count = app.packages.iter()
        .filter(|p| p.status == UpdateStatus::UpdateAvailable)
        .count();

    let mode_label = match app.view_mode {
        ViewMode::All => "All",
        ViewMode::UpdatesOnly => "Updates only",
    };

    let selected_count = app.selected_for_update.len();
    let selected_label = if selected_count > 0 {
        format!(" | {} selected", selected_count)
    } else {
        String::new()
    };

    let title_text = format!(
        " nixup | {} packages | {} updates{} | View: {} ",
        app.filtered_indices.len(),
        update_count,
        selected_label,
        mode_label,
    );

    let title = Paragraph::new(title_text)
        .style(Style::default().fg(COLOR_TITLE).bg(Color::DarkGray));

    frame.render_widget(title, area);
}

/// Search bar and progress gauge
fn draw_search_bar(frame: &mut Frame, area: Rect, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(60), // Search
            Constraint::Percentage(40), // Progress
        ])
        .split(area);

    // -- Search bar --
    let search_style = match app.input_mode {
        InputMode::Search => Style::default().fg(COLOR_SEARCH_ACTIVE),
        InputMode::Normal => Style::default().fg(Color::White),
    };

    let search_label = match app.input_mode {
        InputMode::Search => " / Search: ",
        InputMode::Normal => {
            if app.filter_text.is_empty() {
                " Press / to search "
            } else {
                " Filter: "
            }
        }
    };

    let search_text = format!("{}{}", search_label, app.filter_text);
    let search_block = Block::default().borders(Borders::ALL).title("Search");
    let search_paragraph = Paragraph::new(search_text)
        .style(search_style)
        .block(search_block);

    frame.render_widget(search_paragraph, chunks[0]);

    // Position the cursor if in search mode
    if app.input_mode == InputMode::Search {
        let cursor_x = chunks[0].x + search_label.len() as u16 + app.filter_text.len() as u16 + 1;
        let cursor_y = chunks[0].y + 1;
        frame.set_cursor_position((cursor_x, cursor_y));
    }

    // -- Progress gauge --
    let progress = app.loading_progress();
    let progress_label = if app.is_loading_done() {
        format!("Done ({}/{})", app.checked_count, app.total_count)
    } else {
        format!(
            "Loading {}/{}  ({:.0}%)",
            app.checked_count, app.total_count, progress
        )
    };

    let gauge_color = if app.is_loading_done() {
        Color::Green
    } else {
        Color::Cyan
    };

    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title("Progress"))
        .gauge_style(Style::default().fg(gauge_color))
        .ratio(progress / 100.0)
        .label(progress_label);

    frame.render_widget(gauge, chunks[1]);
}

/// Package table
fn draw_package_table(frame: &mut Frame, area: Rect, app: &App) {
    // Table header
    let header_cells = ["", "Name", "Installed", "Available", "Status"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(COLOR_TITLE).add_modifier(Modifier::BOLD)));
    let header = Row::new(header_cells).height(1);

    // Table rows
    let rows: Vec<Row> = app.filtered_indices.iter()
        .map(|&idx| {
            let pkg = &app.packages[idx];
            let is_checked = app.selected_for_update.contains(&idx);

            // Selection indicator
            let check = if is_checked {
                "[x]"
            } else if pkg.status == UpdateStatus::UpdateAvailable {
                "[ ]"
            } else if pkg.status == UpdateStatus::MajorUpdate {
                "[!]"
            } else {
                "   "
            };

            // Color based on status
            let status_color = match pkg.status {
                UpdateStatus::UpToDate => COLOR_UP_TO_DATE,
                UpdateStatus::UpdateAvailable => COLOR_UPDATE,
                UpdateStatus::MajorUpdate => COLOR_MAJOR,
                UpdateStatus::Newer => COLOR_NEWER,
                UpdateStatus::Unknown => COLOR_UNKNOWN,
                UpdateStatus::Loading => COLOR_LOADING,
            };

            // Status text
            let status_text = match pkg.status {
                UpdateStatus::UpToDate => "OK",
                UpdateStatus::UpdateAvailable => "UPDATE",
                UpdateStatus::MajorUpdate => "MAJOR",
                UpdateStatus::Unknown => "?",
                UpdateStatus::Newer => "NEWER",
                UpdateStatus::Loading => "...",
            };

            // Available version
            let available_text = match &pkg.latest_version {
                Some(v) => v.as_str(),
                None => "-",
            };

            let cells = vec![
                Cell::from(check).style(Style::default().fg(if is_checked { Color::Green } else { Color::DarkGray })),
                Cell::from(pkg.name.clone()),
                Cell::from(pkg.installed_version.clone()),
                Cell::from(available_text.to_string()),
                Cell::from(status_text).style(Style::default().fg(status_color)),
            ];

            Row::new(cells)
        })
        .collect();

    // Column widths
    let widths = [
        Constraint::Length(3),
        Constraint::Percentage(33),
        Constraint::Percentage(23),
        Constraint::Percentage(23),
        Constraint::Percentage(13),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title("Packages"))
        .row_highlight_style(
            Style::default()
                .fg(COLOR_SELECTED)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">> ");

    // Use a TableState for highlighting
    let mut table_state = ratatui::widgets::TableState::default();
    table_state.select(Some(app.selected));

    frame.render_stateful_widget(table, area, &mut table_state);
}

/// Help bar at the bottom
fn draw_help_bar(frame: &mut Frame, area: Rect, app: &App) {
    let help_text = match app.input_mode {
        InputMode::Search => {
            vec![
                Span::styled(" Esc", Style::default().fg(Color::Yellow)),
                Span::raw(" close search  "),
                Span::styled("Enter", Style::default().fg(Color::Yellow)),
                Span::raw(" confirm"),
            ]
        }
        InputMode::Normal => {
            let mut spans = vec![
                Span::styled(" j/k", Style::default().fg(Color::Yellow)),
                Span::raw(" navigate  "),
                Span::styled("/", Style::default().fg(Color::Yellow)),
                Span::raw(" search  "),
                Span::styled("Tab", Style::default().fg(Color::Yellow)),
                Span::raw(" view  "),
                Span::styled("space", Style::default().fg(Color::Green)),
                Span::raw(" select  "),
                Span::styled("U", Style::default().fg(Color::Red)),
                Span::raw(" force major  "),
                Span::styled("u", Style::default().fg(Color::Green)),
                Span::raw(" update  "),
                Span::styled("Enter", Style::default().fg(Color::Yellow)),
                Span::raw(" details  "),
                Span::styled("q", Style::default().fg(Color::Yellow)),
                Span::raw(" quit"),
            ];
            // Show update message if there is one
            if let Some(ref msg) = app.update_message {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(msg.clone(), Style::default().fg(Color::Cyan)));
            }
            spans
        }
    };

    let help_line = Line::from(help_text);
    let help = Paragraph::new(help_line)
        .style(Style::default().bg(Color::DarkGray));

    frame.render_widget(help, area);
}

/// Detail popup for the selected package
fn draw_detail_popup(frame: &mut Frame, app: &App) {
    let selected_pkg = match app.selected_package() {
        Some(p) => p,
        None => return,
    };

    // Calculate the popup area (centered, 60% width, fixed height)
    let area = frame.area();
    let popup_width = (area.width as f32 * 0.6) as u16;
    let popup_height = 12;
    let popup_x = (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    // Clear the popup area
    frame.render_widget(Clear, popup_area);

    // Popup content
    let status_text = match selected_pkg.status {
        UpdateStatus::UpToDate => "Up to date",
        UpdateStatus::UpdateAvailable => "Update available (minor)",
        UpdateStatus::MajorUpdate => "Major update (breaking changes possible)",
        UpdateStatus::Newer => "Newer than unstable",
        UpdateStatus::Unknown => "Unknown",
        UpdateStatus::Loading => "Loading...",
    };

    let status_color = match selected_pkg.status {
        UpdateStatus::UpToDate => COLOR_UP_TO_DATE,
        UpdateStatus::UpdateAvailable => COLOR_UPDATE,
        UpdateStatus::MajorUpdate => COLOR_MAJOR,
        UpdateStatus::Newer => COLOR_NEWER,
        UpdateStatus::Unknown => COLOR_UNKNOWN,
        UpdateStatus::Loading => COLOR_LOADING,
    };

    let latest_text = selected_pkg.latest_version
        .as_deref()
        .unwrap_or("-");

    let description_text = selected_pkg.description
        .as_deref()
        .unwrap_or("No description");

    let homepage_text = selected_pkg.homepage
        .as_deref()
        .unwrap_or("No homepage");

    let lines = vec![
        Line::from(vec![
            Span::styled("  Name:        ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&selected_pkg.name),
        ]),
        Line::from(vec![
            Span::styled("  Installed:   ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&selected_pkg.installed_version),
        ]),
        Line::from(vec![
            Span::styled("  Available:   ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(latest_text),
        ]),
        Line::from(vec![
            Span::styled("  Status:      ", Style::default().add_modifier(Modifier::BOLD)),
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
