use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Wrap},
    Frame,
};

use crate::tui::app::{App, Status, View};

const FROM_COLOR: Color = Color::Cyan;
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

    let header_style = Style::default().add_modifier(Modifier::BOLD);
    let header = Row::new([
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
    .style(header_style)
    .bottom_margin(1);

    let rows: Vec<Row> = app
        .envelopes
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let is_selected = app.selected == i;
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
        })
        .collect();

    let table = Table::new(
        rows,
        [
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
        ],
    )
    .column_spacing(0)
    .header(header)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" {} ", app.folder)),
    );

    let mut state = TableState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(table, chunks[0], &mut state);

    let chunks_bottom =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(chunks[1]);

    let status_line: Line = if let Some(status) = &app.status {
        match status {
            Status::Working(msg) => Line::from(Span::styled(
                format!(" {msg}"),
                Style::default().fg(Color::Yellow),
            )),
            Status::Error(msg) => Line::from(Span::styled(
                format!(" {msg}"),
                Style::default().fg(Color::Red),
            )),
        }
    } else {
        Line::from(vec![
            Span::styled(" q", Style::default().fg(Color::Yellow)),
            Span::raw(": quit | "),
            Span::styled("Enter", Style::default().fg(Color::Yellow)),
            Span::raw(": read | "),
            Span::styled("j/k", Style::default().fg(Color::Yellow)),
            Span::raw(": navigate | "),
            Span::styled("d", Style::default().fg(Color::Yellow)),
            Span::raw(": delete | "),
            Span::styled("a", Style::default().fg(Color::Yellow)),
            Span::raw(": archive"),
        ])
    };
    frame.render_widget(Paragraph::new(status_line), chunks_bottom[0]);

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
        chunks_bottom[1],
    );
}

fn render_message(frame: &mut Frame, content: &str, scroll: u16) {
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(frame.area());

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

    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Message "))
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));

    frame.render_widget(paragraph, chunks[0]);

    let status = Line::from(vec![
        Span::styled(" Esc/q", Style::default().fg(Color::Yellow)),
        Span::raw(": back | "),
        Span::styled("j/k", Style::default().fg(Color::Yellow)),
        Span::raw(": scroll | "),
        Span::styled("d", Style::default().fg(Color::Yellow)),
        Span::raw(": delete | "),
        Span::styled("a", Style::default().fg(Color::Yellow)),
        Span::raw(": archive"),
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
        // These match is_header_line syntax but should only be styled
        // when they appear before the first blank line (enforced by
        // render_message, not by is_header_line itself).
        assert!(is_header_line("Note: something important"));
    }
}
