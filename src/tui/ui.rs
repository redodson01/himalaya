use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap},
    Frame,
};

use crate::tui::app::{
    App, EnvelopeData, FolderEntry, FolderEnvelopeState, FolderSection, MoveFolderPickerState,
    Status, View,
};

const FROM_COLOR: Color = Color::Cyan;
const FLAGGED_COLOR: Color = Color::Yellow;
const HEADER_COLOR: Color = Color::Cyan;
const SECTION_HEADER_COLOR: Color = Color::LightRed;

/// Column widths shared by all envelope tables (main list and folder envelope list).
const ENVELOPE_WIDTHS: [Constraint; 11] = [
    Constraint::Length(1),
    Constraint::Length(6),
    Constraint::Length(1),
    Constraint::Length(1),
    Constraint::Percentage(25),
    Constraint::Length(1),
    Constraint::Length(1),
    Constraint::Percentage(50),
    Constraint::Length(1),
    Constraint::Length(1),
    Constraint::Length(16),
];

/// Build a table header row for envelope lists.
fn envelope_header() -> Row<'static> {
    Row::new([
        Cell::from(" "),
        Cell::from("FLAGS"),
        Cell::from(" "),
        Cell::from(" "),
        Cell::from("FROM"),
        Cell::from(" "),
        Cell::from(" "),
        Cell::from("SUBJECT"),
        Cell::from(" "),
        Cell::from(" "),
        Cell::from("DATE"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD))
    .bottom_margin(1)
}

/// Build a single styled table row for an envelope.
fn envelope_row(e: &EnvelopeData, is_selected: bool) -> Row<'_> {
    let highlight = if is_selected {
        Modifier::REVERSED
    } else {
        Modifier::empty()
    };

    let base_modifier = if e.unseen {
        Modifier::BOLD
    } else {
        Modifier::empty()
    };

    let dim = if e.unseen {
        Modifier::empty()
    } else {
        Modifier::DIM
    };

    let flag_style = if e.flagged {
        Style::default()
            .fg(FLAGGED_COLOR)
            .add_modifier(base_modifier | highlight)
    } else {
        Style::default().add_modifier(dim | base_modifier | highlight)
    };

    let from_style = if e.unseen {
        Style::default().add_modifier(Modifier::BOLD | highlight)
    } else {
        Style::default().fg(FROM_COLOR).add_modifier(highlight)
    };

    let from_combined = from_style.add_modifier(base_modifier);
    let subject_style = Style::default().add_modifier(base_modifier | highlight);
    let date_style = Style::default().add_modifier(dim | base_modifier | highlight);

    Row::new([
        Cell::from(" ").style(flag_style),
        Cell::from(e.flags.as_str()).style(flag_style),
        Cell::from(" ").style(flag_style),
        Cell::from(" ").style(from_combined),
        Cell::from(e.from.as_str()).style(from_combined),
        Cell::from(" ").style(from_combined),
        Cell::from(" ").style(subject_style),
        Cell::from(e.subject.as_str()).style(subject_style),
        Cell::from(" ").style(subject_style),
        Cell::from(" ").style(date_style),
        Cell::from(e.date.as_str()).style(date_style),
    ])
}

pub fn render(frame: &mut Frame, app: &App) {
    match &app.view {
        View::EnvelopeList => render_envelope_list(frame, app),
        View::MessageRead {
            content,
            scroll,
            folder_context,
        } => {
            let active_env = match folder_context {
                Some(ctx) => ctx.envelopes.get(ctx.selected),
                None => app.envelopes.get(app.selected),
            };
            render_message(frame, content, *scroll, app, active_env)
        }
        View::FolderList(state) => {
            render_folder_list(frame, &state.folders, &state.sections, state.selected, app)
        }
        View::FolderEnvelopeList(state) => render_folder_envelope_list(frame, state, app),
        View::MoveFolderPicker(state) => render_move_folder_picker(frame, state, app),
    }
}

/// Returns dynamic labels for the read/unread and flag toggle hints based on
/// the currently selected envelope's state.
fn toggle_labels(env: Option<&EnvelopeData>) -> (&'static str, &'static str) {
    if let Some(env) = env {
        (
            if env.unseen {
                ": mark read | "
            } else {
                ": mark unread | "
            },
            if env.flagged { ": unflag" } else { ": flag" },
        )
    } else {
        (": mark read/unread | ", ": flag/unflag")
    }
}

/// Build the compose hints line shown on the second row of the bottom bar.
fn compose_hints_line() -> Line<'static> {
    Line::from(vec![
        Span::styled(" N", Style::default().fg(Color::Yellow)),
        Span::raw(": new | "),
        Span::styled("R", Style::default().fg(Color::Yellow)),
        Span::raw(": reply | "),
        Span::styled("A", Style::default().fg(Color::Yellow)),
        Span::raw(": reply all | "),
        Span::styled("F", Style::default().fg(Color::Yellow)),
        Span::raw(": forward"),
    ])
}

/// Renders the flag legend (Seen Flagged Answered Deleted drafT) into the
/// given area, right-aligned.
fn render_flag_legend(frame: &mut Frame, area: ratatui::layout::Rect) {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let flag_key = Line::from(vec![
        Span::raw("S"),
        Span::styled("een ", dim),
        Span::raw("F"),
        Span::styled("lagged ", dim),
        Span::raw("A"),
        Span::styled("nswered ", dim),
        Span::raw("D"),
        Span::styled("eleted ", dim),
        Span::styled("Draf", dim),
        Span::raw("T "),
    ]);
    frame.render_widget(
        Paragraph::new(flag_key).alignment(ratatui::layout::Alignment::Right),
        area,
    );
}

fn render_search_bar(frame: &mut Frame, area: ratatui::layout::Rect, query: &str) {
    let text = Line::from(vec![
        Span::styled("/", Style::default().fg(Color::Yellow)),
        Span::raw(query),
        Span::styled("_", Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(text), area);
}

/// Render the search-mode bottom bar. Returns `true` if handled (caller should
/// skip normal bottom-bar rendering).
fn render_search_bottom(frame: &mut Frame, area: ratatui::layout::Rect, app: &App) -> bool {
    let Some(search) = &app.search else {
        return false;
    };
    if let Some(status) = &app.status {
        let (msg, color) = match status {
            Status::Working(msg) => (msg.as_str(), Color::Yellow),
            Status::Error(msg) => (msg.as_str(), Color::Red),
        };
        let line = Line::from(Span::styled(
            format!(" {msg}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(Paragraph::new(line), area);
    } else {
        render_search_bar(frame, area, &search.query);
    }
    true
}

fn render_envelope_list(frame: &mut Frame, app: &App) {
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(frame.area());

    let searching = app.search.is_some();
    let matched_indices = app.search.as_ref().map(|s| &s.matched_indices[..]);
    let search_selected = app.search.as_ref().map(|s| s.selected).unwrap_or(0);

    // Determine which envelope indices to show
    let visible_indices: Vec<usize> = match matched_indices {
        Some(indices) => indices.to_vec(),
        None => (0..app.envelopes.len()).collect(),
    };
    let visible_set: std::collections::HashSet<usize> = visible_indices.iter().copied().collect();

    // Build section starts for visible items only
    let section_starts: std::collections::HashMap<usize, (&str, bool)> = if searching {
        let mut map = std::collections::HashMap::new();
        let mut first = true;
        for section in &app.sections {
            // Find first visible index in this section
            let first_visible =
                (section.start..section.start + section.count).find(|i| visible_set.contains(i));
            if let Some(idx) = first_visible {
                let is_first = first;
                first = false;
                map.insert(idx, (section.name.as_str(), is_first));
            }
        }
        map
    } else {
        let mut first = true;
        app.sections
            .iter()
            .filter(|s| s.count > 0)
            .map(|s| {
                let is_first = first;
                first = false;
                (s.start, (s.name.as_str(), is_first))
            })
            .collect()
    };

    let mut rows: Vec<Row> = Vec::new();
    let mut item_to_table_row: Vec<usize> = Vec::new();

    for (pos, &i) in visible_indices.iter().enumerate() {
        let e = &app.envelopes[i];
        if let Some((account_name, is_first)) = section_starts.get(&i) {
            if !is_first {
                rows.push(Row::new(std::iter::repeat_with(|| Cell::from("")).take(11)));
            }
            let style = Style::default()
                .fg(SECTION_HEADER_COLOR)
                .add_modifier(Modifier::BOLD);
            let mut cells: Vec<Cell> = std::iter::repeat_with(|| Cell::from("")).take(11).collect();
            cells[4] = Cell::from(account_name.to_string()).style(style);
            rows.push(Row::new(cells));
        }

        item_to_table_row.push(rows.len());
        let is_selected = if searching {
            pos == search_selected
        } else {
            i == app.selected
        };
        rows.push(envelope_row(e, is_selected));
    }

    let table = Table::new(rows, ENVELOPE_WIDTHS)
        .column_spacing(0)
        .header(envelope_header())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", app.folder)),
        );

    let highlight_pos = if searching {
        search_selected
    } else {
        visible_indices
            .iter()
            .position(|&i| i == app.selected)
            .unwrap_or(0)
    };
    let table_selected = item_to_table_row.get(highlight_pos).copied().unwrap_or(0);
    let mut state = TableState::default().with_selected(Some(table_selected));
    frame.render_stateful_widget(table, chunks[0], &mut state);

    if !render_search_bottom(frame, chunks[1], app) {
        let rows =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(chunks[1]);
        let chunks_bottom =
            Layout::horizontal([Constraint::Percentage(65), Constraint::Percentage(35)])
                .split(rows[0]);

        let status_line = if let Some(status) = &app.status {
            let (msg, color) = match status {
                Status::Working(msg) => (msg.as_str(), Color::Yellow),
                Status::Error(msg) => (msg.as_str(), Color::Red),
            };
            Line::from(Span::styled(
                format!(" {msg}"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ))
        } else {
            let (read_label, flag_label) = toggle_labels(app.envelopes.get(app.selected));
            Line::from(vec![
                Span::styled(" Esc/q", Style::default().fg(Color::Yellow)),
                Span::raw(": quit | "),
                Span::styled("j/k", Style::default().fg(Color::Yellow)),
                Span::raw(": navigate | "),
                Span::styled("Enter", Style::default().fg(Color::Yellow)),
                Span::raw(": read | "),
                Span::styled("r", Style::default().fg(Color::Yellow)),
                Span::raw(read_label),
                Span::styled("f", Style::default().fg(Color::Yellow)),
                Span::raw(flag_label),
                Span::raw(" | "),
                Span::styled("d", Style::default().fg(Color::Yellow)),
                Span::raw(": delete | "),
                Span::styled("a", Style::default().fg(Color::Yellow)),
                Span::raw(": archive | "),
                Span::styled("m", Style::default().fg(Color::Yellow)),
                Span::raw(": move | "),
                Span::styled("\\", Style::default().fg(Color::Yellow)),
                Span::raw(": folders | "),
                Span::styled("/", Style::default().fg(Color::Yellow)),
                Span::raw(": search"),
            ])
        };
        frame.render_widget(Paragraph::new(status_line), chunks_bottom[0]);
        render_flag_legend(frame, chunks_bottom[1]);
        frame.render_widget(Paragraph::new(compose_hints_line()), rows[1]);
    }
}

fn render_folder_list(
    frame: &mut Frame,
    folders: &[FolderEntry],
    sections: &[FolderSection],
    selected: usize,
    app: &App,
) {
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(frame.area());

    let searching = app.search.is_some();
    let search_selected = app.search.as_ref().map(|s| s.selected).unwrap_or(0);

    let visible_indices: Vec<usize> = match app.search.as_ref() {
        Some(s) => s.matched_indices.clone(),
        None => (0..folders.len()).collect(),
    };
    let visible_set: std::collections::HashSet<usize> = visible_indices.iter().copied().collect();

    let section_starts: std::collections::HashMap<usize, &str> = if searching {
        let mut map = std::collections::HashMap::new();
        for section in sections {
            let first_visible =
                (section.start..section.start + section.count).find(|i| visible_set.contains(i));
            if let Some(idx) = first_visible {
                map.insert(idx, section.name.as_str());
            }
        }
        map
    } else {
        sections
            .iter()
            .filter(|s| s.count > 0)
            .map(|s| (s.start, s.name.as_str()))
            .collect()
    };

    let mut rows: Vec<Row> = Vec::new();
    let mut folder_to_table_row: Vec<usize> = Vec::new();

    for &i in visible_indices.iter() {
        let f = &folders[i];
        if let Some(account_name) = section_starts.get(&i) {
            rows.push(Row::new([Cell::from("")]));
            let style = Style::default()
                .fg(SECTION_HEADER_COLOR)
                .add_modifier(Modifier::BOLD);
            rows.push(Row::new([
                Cell::from(format!("  {account_name}")).style(style)
            ]));
        }

        folder_to_table_row.push(rows.len());
        rows.push(Row::new([Cell::from(format!("  {}", f.name))]));
    }

    let table = Table::new(rows, [Constraint::Percentage(100)])
        .column_spacing(0)
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Select Folder "),
        );

    let highlight_pos = if searching {
        search_selected
    } else {
        visible_indices
            .iter()
            .position(|&i| i == selected)
            .unwrap_or(0)
    };
    let table_selected = folder_to_table_row.get(highlight_pos).copied().unwrap_or(0);
    let mut state = TableState::default().with_selected(Some(table_selected));
    frame.render_stateful_widget(table, chunks[0], &mut state);

    if !render_search_bottom(frame, chunks[1], app) {
        let status_line = if let Some(status) = &app.status {
            let (msg, color) = match status {
                Status::Working(msg) => (msg.as_str(), Color::Yellow),
                Status::Error(msg) => (msg.as_str(), Color::Red),
            };
            Line::from(Span::styled(
                format!(" {msg}"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ))
        } else {
            Line::from(vec![
                Span::styled(" Esc/q", Style::default().fg(Color::Yellow)),
                Span::raw(": back | "),
                Span::styled("j/k", Style::default().fg(Color::Yellow)),
                Span::raw(": navigate | "),
                Span::styled("Enter", Style::default().fg(Color::Yellow)),
                Span::raw(": select folder | "),
                Span::styled("/", Style::default().fg(Color::Yellow)),
                Span::raw(": search"),
            ])
        };
        frame.render_widget(Paragraph::new(status_line), chunks[1]);
    }
}

fn render_folder_envelope_list(frame: &mut Frame, fe_state: &FolderEnvelopeState, app: &App) {
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(frame.area());

    let searching = app.search.is_some();
    let search_selected = app.search.as_ref().map(|s| s.selected).unwrap_or(0);

    let visible_indices: Vec<usize> = match app.search.as_ref() {
        Some(s) => s.matched_indices.clone(),
        None => (0..fe_state.envelopes.len()).collect(),
    };

    let mut rows: Vec<Row> = Vec::new();
    let mut item_to_table_row: Vec<usize> = Vec::new();

    // Account header
    if !visible_indices.is_empty() {
        let style = Style::default()
            .fg(SECTION_HEADER_COLOR)
            .add_modifier(Modifier::BOLD);
        let mut cells: Vec<Cell> = std::iter::repeat_with(|| Cell::from("")).take(11).collect();
        cells[4] = Cell::from(fe_state.account_key.to_string()).style(style);
        rows.push(Row::new(cells));
    }

    for (pos, &i) in visible_indices.iter().enumerate() {
        item_to_table_row.push(rows.len());
        let is_selected = if searching {
            pos == search_selected
        } else {
            i == fe_state.selected
        };
        rows.push(envelope_row(&fe_state.envelopes[i], is_selected));
    }

    let table = Table::new(rows, ENVELOPE_WIDTHS)
        .column_spacing(0)
        .header(envelope_header())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", fe_state.folder_name)),
        );

    let highlight_pos = if searching {
        search_selected
    } else {
        visible_indices
            .iter()
            .position(|&i| i == fe_state.selected)
            .unwrap_or(0)
    };
    let table_selected = item_to_table_row.get(highlight_pos).copied().unwrap_or(0);
    let mut table_state = TableState::default().with_selected(Some(table_selected));
    frame.render_stateful_widget(table, chunks[0], &mut table_state);

    if !render_search_bottom(frame, chunks[1], app) {
        let rows =
            Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(chunks[1]);
        let chunks_bottom =
            Layout::horizontal([Constraint::Percentage(65), Constraint::Percentage(35)])
                .split(rows[0]);

        let status_line = if let Some(status) = &app.status {
            let (msg, color) = match status {
                Status::Working(msg) => (msg.as_str(), Color::Yellow),
                Status::Error(msg) => (msg.as_str(), Color::Red),
            };
            Line::from(Span::styled(
                format!(" {msg}"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ))
        } else {
            let (read_label, flag_label) = toggle_labels(fe_state.envelopes.get(fe_state.selected));
            Line::from(vec![
                Span::styled(" Esc/q", Style::default().fg(Color::Yellow)),
                Span::raw(": back | "),
                Span::styled("j/k", Style::default().fg(Color::Yellow)),
                Span::raw(": navigate | "),
                Span::styled("Enter", Style::default().fg(Color::Yellow)),
                Span::raw(": read | "),
                Span::styled("r", Style::default().fg(Color::Yellow)),
                Span::raw(read_label),
                Span::styled("f", Style::default().fg(Color::Yellow)),
                Span::raw(flag_label),
                Span::raw(" | "),
                Span::styled("d", Style::default().fg(Color::Yellow)),
                Span::raw(": delete | "),
                Span::styled("a", Style::default().fg(Color::Yellow)),
                Span::raw(": archive | "),
                Span::styled("m", Style::default().fg(Color::Yellow)),
                Span::raw(": move | "),
                Span::styled("/", Style::default().fg(Color::Yellow)),
                Span::raw(": search"),
            ])
        };
        frame.render_widget(Paragraph::new(status_line), chunks_bottom[0]);
        render_flag_legend(frame, chunks_bottom[1]);
        frame.render_widget(Paragraph::new(compose_hints_line()), rows[1]);
    }
}

fn render_move_folder_picker(frame: &mut Frame, state: &MoveFolderPickerState, app: &App) {
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(frame.area());

    let searching = app.search.is_some();
    let search_selected = app.search.as_ref().map(|s| s.selected).unwrap_or(0);

    let visible_indices: Vec<usize> = match app.search.as_ref() {
        Some(s) => s.matched_indices.clone(),
        None => (0..state.folders.len()).collect(),
    };

    let mut rows: Vec<Row> = Vec::new();
    let mut folder_to_table_row: Vec<usize> = Vec::new();

    // Account header before the first folder
    if !visible_indices.is_empty() {
        rows.push(Row::new([Cell::from("")]));
        let style = Style::default()
            .fg(SECTION_HEADER_COLOR)
            .add_modifier(Modifier::BOLD);
        rows.push(Row::new([
            Cell::from(format!("  {}", state.account_key)).style(style)
        ]));
    }

    for &i in visible_indices.iter() {
        folder_to_table_row.push(rows.len());
        rows.push(Row::new([Cell::from(format!(
            "  {}",
            state.folders[i].name
        ))]));
    }

    let table = Table::new(rows, [Constraint::Percentage(100)])
        .column_spacing(0)
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Move to Folder "),
        );

    let highlight_pos = if searching {
        search_selected
    } else {
        visible_indices
            .iter()
            .position(|&i| i == state.selected)
            .unwrap_or(0)
    };
    let table_selected = folder_to_table_row.get(highlight_pos).copied().unwrap_or(0);
    let mut table_state = TableState::default().with_selected(Some(table_selected));
    frame.render_stateful_widget(table, chunks[0], &mut table_state);

    if !render_search_bottom(frame, chunks[1], app) {
        let status_line = if let Some(status) = &app.status {
            let (msg, color) = match status {
                Status::Working(msg) => (msg.as_str(), Color::Yellow),
                Status::Error(msg) => (msg.as_str(), Color::Red),
            };
            Line::from(Span::styled(
                format!(" {msg}"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ))
        } else {
            Line::from(vec![
                Span::styled(" Esc/q", Style::default().fg(Color::Yellow)),
                Span::raw(": cancel | "),
                Span::styled("j/k", Style::default().fg(Color::Yellow)),
                Span::raw(": navigate | "),
                Span::styled("Enter", Style::default().fg(Color::Yellow)),
                Span::raw(": move here | "),
                Span::styled("/", Style::default().fg(Color::Yellow)),
                Span::raw(": search"),
            ])
        };
        frame.render_widget(Paragraph::new(status_line), chunks[1]);
    }
}

fn render_message(
    frame: &mut Frame,
    content: &str,
    scroll: u16,
    app: &App,
    active_env: Option<&EnvelopeData>,
) {
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).split(frame.area());

    // Color header lines (e.g. "From: ...", "Subject: ...") differently from body.
    // Headers only appear before the first blank line.
    let mut in_headers = true;
    let lines: Vec<Line> = content
        .lines()
        .map(|line| {
            if in_headers && line.is_empty() {
                in_headers = false;
            }
            if in_headers && is_header_line(line) {
                if let Some((key, value)) = line.split_once(": ") {
                    Line::from(vec![
                        Span::styled(
                            format!("{key}: "),
                            Style::default()
                                .fg(HEADER_COLOR)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(value.to_string()),
                    ])
                } else {
                    Line::styled(line.to_string(), Style::default().fg(HEADER_COLOR))
                }
            } else {
                Line::raw(line.to_string())
            }
        })
        .collect();

    let title = if let Some(env) = active_env {
        if env.flags.is_empty() {
            " Message ".to_string()
        } else {
            format!(" Message [{}] ", env.flags)
        }
    } else {
        " Message ".to_string()
    };

    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));

    frame.render_widget(paragraph, chunks[0]);

    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(chunks[1]);
    let chunks_bottom =
        Layout::horizontal([Constraint::Percentage(65), Constraint::Percentage(35)]).split(rows[0]);

    let status_line = if let Some(s) = &app.status {
        let (msg, color) = match s {
            Status::Working(msg) => (msg.as_str(), Color::Yellow),
            Status::Error(msg) => (msg.as_str(), Color::Red),
        };
        Line::from(Span::styled(
            format!(" {msg}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ))
    } else {
        let (read_label, flag_label) = toggle_labels(active_env);
        Line::from(vec![
            Span::styled(" Esc/q", Style::default().fg(Color::Yellow)),
            Span::raw(": back | "),
            Span::styled("j/k", Style::default().fg(Color::Yellow)),
            Span::raw(": scroll | "),
            Span::styled("n", Style::default().fg(Color::Yellow)),
            Span::raw(": next | "),
            Span::styled("r", Style::default().fg(Color::Yellow)),
            Span::raw(read_label),
            Span::styled("f", Style::default().fg(Color::Yellow)),
            Span::raw(flag_label),
            Span::raw(" | "),
            Span::styled("d", Style::default().fg(Color::Yellow)),
            Span::raw(": delete | "),
            Span::styled("a", Style::default().fg(Color::Yellow)),
            Span::raw(": archive | "),
            Span::styled("m", Style::default().fg(Color::Yellow)),
            Span::raw(": move"),
        ])
    };
    frame.render_widget(Paragraph::new(status_line), chunks_bottom[0]);
    render_flag_legend(frame, chunks_bottom[1]);
    frame.render_widget(Paragraph::new(compose_hints_line()), rows[1]);
}

/// Check if a line looks like an email header (e.g. "From: ...", "Subject: ...").
/// Headers appear at the top of the message before the first blank line.
/// We identify them by common header name patterns with a colon.
fn is_header_line(line: &str) -> bool {
    let Some((key, _)) = line.split_once(": ") else {
        return false;
    };
    // Header keys are ASCII alphanumeric plus hyphens, no spaces
    !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_line_standard_headers() {
        assert!(is_header_line("From: alice@example.com"));
        assert!(is_header_line("Subject: Hello"));
        assert!(is_header_line("Date: 2025-01-01"));
        assert!(is_header_line("Content-Type: text/plain"));
        assert!(is_header_line("X-Custom-Header: value"));
    }

    #[test]
    fn header_line_rejects_non_headers() {
        assert!(!is_header_line("Hello, world!"));
        assert!(!is_header_line("Dear Alice: how are you"));
        assert!(!is_header_line(""));
        assert!(!is_header_line("no colon here"));
        assert!(!is_header_line(": empty key"));
        // These match is_header_line syntax but should only be styled
        // when they appear before the first blank line (enforced by
        // render_message, not by is_header_line itself).
        assert!(is_header_line("Note: something important"));
    }
}
