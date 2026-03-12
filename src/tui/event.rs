use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};

use crate::tui::app::View;

pub enum Action {
    None,
    Quit,
    ReadMessage,
    BackToList,
    ScrollDown,
    ScrollUp,
    SelectNext,
    SelectPrev,
}

pub fn handle_event(view: &View) -> color_eyre::Result<Action> {
    if !event::poll(std::time::Duration::from_millis(100))? {
        return Ok(Action::None);
    }

    let Event::Key(key) = event::read()? else {
        return Ok(Action::None);
    };

    if key.kind != KeyEventKind::Press {
        return Ok(Action::None);
    }

    let action = match view {
        View::EnvelopeList => match key.code {
            KeyCode::Char('q') => Action::Quit,
            KeyCode::Down | KeyCode::Char('j') => Action::SelectNext,
            KeyCode::Up | KeyCode::Char('k') => Action::SelectPrev,
            KeyCode::Enter => Action::ReadMessage,
            _ => Action::None,
        },
        View::MessageRead { .. } => match key.code {
            KeyCode::Esc | KeyCode::Char('q') => Action::BackToList,
            KeyCode::Down | KeyCode::Char('j') => Action::ScrollDown,
            KeyCode::Up | KeyCode::Char('k') => Action::ScrollUp,
            _ => Action::None,
        },
    };

    Ok(action)
}
