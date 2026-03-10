use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap},
    Frame,
};

use crate::tui::app::{App, View};

pub fn render(frame: &mut Frame, app: &App) {
    match &app.view {
        View::EnvelopeList => render_envelope_list(frame, app),
        View::MessageRead {
            content, scroll, ..
        } => render_message(frame, app, content, *scroll),
    }
}

fn render_envelope_list(frame: &mut Frame, app: &App) {
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(frame.area());

    let header = Row::new(["FLAGS", "FROM", "SUBJECT", "DATE"])
        .style(Style::default().add_modifier(Modifier::BOLD))
        .bottom_margin(1);

    let rows: Vec<Row> = app
        .envelopes
        .iter()
        .map(|e| {
            Row::new([
                Cell::from(e.flags.as_str()),
                Cell::from(e.from.as_str()),
                Cell::from(e.subject.as_str()),
                Cell::from(e.date.as_str()),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(6),
            Constraint::Percentage(25),
            Constraint::Percentage(50),
            Constraint::Length(16),
        ],
    )
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" {} ", app.folder)),
    )
    .row_highlight_style(
        Style::default()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );

    let mut state = TableState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(table, chunks[0], &mut state);

    let status = Line::from(vec![
        Span::styled(" q", Style::default().fg(Color::Yellow)),
        Span::raw(": quit | "),
        Span::styled("Enter", Style::default().fg(Color::Yellow)),
        Span::raw(": read | "),
        Span::styled("j/k", Style::default().fg(Color::Yellow)),
        Span::raw(": navigate"),
    ]);
    frame.render_widget(Paragraph::new(status), chunks[1]);
}

fn render_message(frame: &mut Frame, _app: &App, content: &str, scroll: u16) {
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(frame.area());

    let paragraph = Paragraph::new(content)
        .block(Block::default().borders(Borders::ALL).title(" Message "))
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));

    frame.render_widget(paragraph, chunks[0]);

    let status = Line::from(vec![
        Span::styled(" Esc/q", Style::default().fg(Color::Yellow)),
        Span::raw(": back | "),
        Span::styled("j/k", Style::default().fg(Color::Yellow)),
        Span::raw(": scroll"),
    ]);
    frame.render_widget(Paragraph::new(status), chunks[1]);
}
