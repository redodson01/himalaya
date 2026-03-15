use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};

use crate::tui::app::View;

#[derive(Debug, PartialEq)]
pub enum Action {
    None,
    Quit,
    ReadMessage,
    BackToList,
    ScrollDown,
    ScrollUp,
    SelectNext,
    SelectPrev,
    DeleteMessage,
    ArchiveMessage,
    ToggleRead,
    NextMessage,
    ToggleFlag,
}

pub fn handle_event(view: &View, _searching: bool) -> color_eyre::Result<Action> {
    if !event::poll(std::time::Duration::from_millis(100))? {
        return Ok(Action::None);
    }

    let Event::Key(key) = event::read()? else {
        return Ok(Action::None);
    };

    if key.kind != KeyEventKind::Press {
        return Ok(Action::None);
    }

    Ok(action_for_key(view, key.code, _searching))
}

/// Pure mapping from (view, key) to action. Separated from handle_event
/// so it can be unit-tested without terminal I/O.
fn action_for_key(view: &View, key: KeyCode, _searching: bool) -> Action {
    match view {
        View::MessageList => match key {
            KeyCode::Esc | KeyCode::Char('q') => Action::Quit,
            KeyCode::Down | KeyCode::Char('j') => Action::SelectNext,
            KeyCode::Up | KeyCode::Char('k') => Action::SelectPrev,
            KeyCode::Enter => Action::ReadMessage,
            KeyCode::Char('d') => Action::DeleteMessage,
            KeyCode::Char('a') => Action::ArchiveMessage,
            KeyCode::Char('r') => Action::ToggleRead,
            KeyCode::Char('f') => Action::ToggleFlag,
            _ => Action::None,
        },
        View::MessageRead { .. } => match key {
            KeyCode::Esc | KeyCode::Char('q') => Action::BackToList,
            KeyCode::Down | KeyCode::Char('j') => Action::ScrollDown,
            KeyCode::Up | KeyCode::Char('k') => Action::ScrollUp,
            KeyCode::Char('d') => Action::DeleteMessage,
            KeyCode::Char('a') => Action::ArchiveMessage,
            KeyCode::Char('r') => Action::ToggleRead,
            KeyCode::Char('n') => Action::NextMessage,
            KeyCode::Char('f') => Action::ToggleFlag,
            _ => Action::None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list_view() -> View {
        View::MessageList
    }

    fn message_view() -> View {
        View::MessageRead {
            content: String::new(),
            scroll: 0,
        }
    }

    // --- Message list view ---

    #[test]
    fn list_q_quits() {
        assert_eq!(
            action_for_key(&list_view(), KeyCode::Char('q'), false),
            Action::Quit
        );
    }

    #[test]
    fn list_esc_quits() {
        assert_eq!(
            action_for_key(&list_view(), KeyCode::Esc, false),
            Action::Quit
        );
    }

    #[test]
    fn list_j_selects_next() {
        assert_eq!(
            action_for_key(&list_view(), KeyCode::Char('j'), false),
            Action::SelectNext
        );
    }

    #[test]
    fn list_k_selects_prev() {
        assert_eq!(
            action_for_key(&list_view(), KeyCode::Char('k'), false),
            Action::SelectPrev
        );
    }

    #[test]
    fn list_enter_reads_message() {
        assert_eq!(
            action_for_key(&list_view(), KeyCode::Enter, false),
            Action::ReadMessage
        );
    }

    #[test]
    fn list_d_deletes() {
        assert_eq!(
            action_for_key(&list_view(), KeyCode::Char('d'), false),
            Action::DeleteMessage
        );
    }

    #[test]
    fn list_a_archives() {
        assert_eq!(
            action_for_key(&list_view(), KeyCode::Char('a'), false),
            Action::ArchiveMessage
        );
    }

    #[test]
    fn list_r_toggles_read() {
        assert_eq!(
            action_for_key(&list_view(), KeyCode::Char('r'), false),
            Action::ToggleRead
        );
    }

    #[test]
    fn list_f_toggles_flag() {
        assert_eq!(
            action_for_key(&list_view(), KeyCode::Char('f'), false),
            Action::ToggleFlag
        );
    }

    #[test]
    fn list_unknown_key_is_none() {
        assert_eq!(
            action_for_key(&list_view(), KeyCode::Char('z'), false),
            Action::None
        );
    }

    // --- Message read view ---

    #[test]
    fn message_esc_goes_back() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Esc, false),
            Action::BackToList
        );
    }

    #[test]
    fn message_q_goes_back() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Char('q'), false),
            Action::BackToList
        );
    }

    #[test]
    fn message_j_scrolls_down() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Char('j'), false),
            Action::ScrollDown
        );
    }

    #[test]
    fn message_k_scrolls_up() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Char('k'), false),
            Action::ScrollUp
        );
    }

    #[test]
    fn message_d_deletes() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Char('d'), false),
            Action::DeleteMessage
        );
    }

    #[test]
    fn message_a_archives() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Char('a'), false),
            Action::ArchiveMessage
        );
    }

    #[test]
    fn message_r_toggles_read() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Char('r'), false),
            Action::ToggleRead
        );
    }

    #[test]
    fn message_n_next_message() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Char('n'), false),
            Action::NextMessage
        );
    }

    #[test]
    fn message_f_toggles_flag() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Char('f'), false),
            Action::ToggleFlag
        );
    }

    #[test]
    fn message_b_is_none() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Char('b'), false),
            Action::None
        );
    }

    #[test]
    fn message_unknown_key_is_none() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Char('z'), false),
            Action::None
        );
    }
}
