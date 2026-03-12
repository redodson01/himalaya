use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap},
    Frame,
};

use crate::tui::app::{App, View};

const FLAG_COLOR: Color = Color::DarkGray;
const FROM_COLOR: Color = Color::Cyan;
const DATE_COLOR: Color = Color::DarkGray;
const UNSEEN_COLOR: Color = Color::White;
const FLAGGED_COLOR: Color = Color::Yellow;
const HEADER_COLOR: Color = Color::Cyan;

pub fn render(frame: &mut Frame, app: &App) {
    match &app.view {
        View::EnvelopeList => render_envelope_list(frame, app),
        View::MessageRead {
            content, scroll, ..
        } => render_message(frame, content, *scroll),
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
            let base_modifier = if e.unseen {
                Modifier::BOLD
            } else {
                Modifier::empty()
            };

            let flag_style = if e.flagged {
                Style::default()
                    .fg(FLAGGED_COLOR)
                    .add_modifier(base_modifier)
            } else {
                Style::default().fg(FLAG_COLOR).add_modifier(base_modifier)
            };

            let from_color = if e.unseen { UNSEEN_COLOR } else { FROM_COLOR };

            Row::new([
                Cell::from(e.flags.as_str()).style(flag_style),
                Cell::from(e.from.as_str())
                    .style(Style::default().fg(from_color).add_modifier(base_modifier)),
                Cell::from(e.subject.as_str()).style(Style::default().add_modifier(base_modifier)),
                Cell::from(e.date.as_str())
                    .style(Style::default().fg(DATE_COLOR).add_modifier(base_modifier)),
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

fn render_message(frame: &mut Frame, content: &str, scroll: u16) {
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(frame.area());

    // Color header lines (e.g. "From: ...", "Subject: ...") differently from body
    let lines: Vec<Line> = content
        .lines()
        .map(|line| {
            if is_header_line(line) {
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

    let paragraph = Paragraph::new(lines)
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
    }
}
