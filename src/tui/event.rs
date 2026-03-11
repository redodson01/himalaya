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
    OpenFolderList,
    SelectFolder,
    FolderSelectNext,
    FolderSelectPrev,
    BackFromFolders,
    BackFromFolderEnvelopes,
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

    Ok(action_for_key(view, key.code))
}

/// Pure mapping from (view, key) to action. Separated from handle_event
/// so it can be unit-tested without terminal I/O.
fn action_for_key(view: &View, key: KeyCode) -> Action {
    match view {
        View::EnvelopeList => match key {
            KeyCode::Esc | KeyCode::Char('q') => Action::Quit,
            KeyCode::Down | KeyCode::Char('j') => Action::SelectNext,
            KeyCode::Up | KeyCode::Char('k') => Action::SelectPrev,
            KeyCode::Enter => Action::ReadMessage,
            KeyCode::Char('d') => Action::DeleteMessage,
            KeyCode::Char('a') => Action::ArchiveMessage,
            KeyCode::Char('r') => Action::ToggleRead,
            KeyCode::Char('f') => Action::ToggleFlag,
            KeyCode::Char('g') => Action::OpenFolderList,
            _ => Action::None,
        },
        View::FolderList(_) => match key {
            KeyCode::Esc | KeyCode::Char('b') => Action::BackFromFolders,
            KeyCode::Down | KeyCode::Char('j') => Action::FolderSelectNext,
            KeyCode::Up | KeyCode::Char('k') => Action::FolderSelectPrev,
            KeyCode::Enter => Action::SelectFolder,
            _ => Action::None,
        },
        View::FolderEnvelopeList(_) => match key {
            KeyCode::Esc | KeyCode::Char('b') => Action::BackFromFolderEnvelopes,
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
            KeyCode::Esc | KeyCode::Char('b') => Action::BackToList,
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
        View::EnvelopeList
    }

    fn message_view() -> View {
        View::MessageRead {
            content: String::new(),
            scroll: 0,
            folder_context: None,
        }
    }

    // --- Envelope list view ---

    #[test]
    fn list_q_quits() {
        assert_eq!(
            action_for_key(&list_view(), KeyCode::Char('q')),
            Action::Quit
        );
    }

    #[test]
    fn list_esc_quits() {
        assert_eq!(action_for_key(&list_view(), KeyCode::Esc), Action::Quit);
    }

    #[test]
    fn list_j_selects_next() {
        assert_eq!(
            action_for_key(&list_view(), KeyCode::Char('j')),
            Action::SelectNext
        );
    }

    #[test]
    fn list_k_selects_prev() {
        assert_eq!(
            action_for_key(&list_view(), KeyCode::Char('k')),
            Action::SelectPrev
        );
    }

    #[test]
    fn list_enter_reads_message() {
        assert_eq!(
            action_for_key(&list_view(), KeyCode::Enter),
            Action::ReadMessage
        );
    }

    #[test]
    fn list_d_deletes() {
        assert_eq!(
            action_for_key(&list_view(), KeyCode::Char('d')),
            Action::DeleteMessage
        );
    }

    #[test]
    fn list_a_archives() {
        assert_eq!(
            action_for_key(&list_view(), KeyCode::Char('a')),
            Action::ArchiveMessage
        );
    }

    #[test]
    fn list_r_toggles_read() {
        assert_eq!(
            action_for_key(&list_view(), KeyCode::Char('r')),
            Action::ToggleRead
        );
    }

    #[test]
    fn list_f_toggles_flag() {
        assert_eq!(
            action_for_key(&list_view(), KeyCode::Char('f')),
            Action::ToggleFlag
        );
    }

    #[test]
    fn list_unknown_key_is_none() {
        assert_eq!(
            action_for_key(&list_view(), KeyCode::Char('z')),
            Action::None
        );
    }

    // --- Envelope list: g opens folders ---

    #[test]
    fn list_g_opens_folders() {
        assert_eq!(
            action_for_key(&list_view(), KeyCode::Char('g')),
            Action::OpenFolderList
        );
    }

    // --- Folder list view ---

    fn folder_view() -> View {
        use crate::tui::app::FolderListState;
        View::FolderList(FolderListState {
            folders: Vec::new(),
            sections: Vec::new(),
            selected: 0,
            saved_envelope_selected: 0,
        })
    }

    #[test]
    fn folder_j_selects_next() {
        assert_eq!(
            action_for_key(&folder_view(), KeyCode::Char('j')),
            Action::FolderSelectNext
        );
    }

    #[test]
    fn folder_k_selects_prev() {
        assert_eq!(
            action_for_key(&folder_view(), KeyCode::Char('k')),
            Action::FolderSelectPrev
        );
    }

    #[test]
    fn folder_enter_selects() {
        assert_eq!(
            action_for_key(&folder_view(), KeyCode::Enter),
            Action::SelectFolder
        );
    }

    #[test]
    fn folder_esc_goes_back() {
        assert_eq!(
            action_for_key(&folder_view(), KeyCode::Esc),
            Action::BackFromFolders
        );
    }

    #[test]
    fn folder_b_goes_back() {
        assert_eq!(
            action_for_key(&folder_view(), KeyCode::Char('b')),
            Action::BackFromFolders
        );
    }

    #[test]
    fn folder_q_is_none() {
        assert_eq!(
            action_for_key(&folder_view(), KeyCode::Char('q')),
            Action::None
        );
    }

    #[test]
    fn folder_unknown_is_none() {
        assert_eq!(
            action_for_key(&folder_view(), KeyCode::Char('z')),
            Action::None
        );
    }

    // --- Folder envelope list view ---

    fn folder_envelope_view() -> View {
        use crate::tui::app::{FolderEnvelopeState, FolderListState};
        View::FolderEnvelopeList(FolderEnvelopeState {
            envelopes: Vec::new(),
            selected: 0,
            folder_name: "Sent".to_string(),
            account_key: String::new(),
            parent: FolderListState {
                folders: Vec::new(),
                sections: Vec::new(),
                selected: 0,
                saved_envelope_selected: 0,
            },
        })
    }

    #[test]
    fn folder_envelope_esc_goes_back() {
        assert_eq!(
            action_for_key(&folder_envelope_view(), KeyCode::Esc),
            Action::BackFromFolderEnvelopes
        );
    }

    #[test]
    fn folder_envelope_b_goes_back() {
        assert_eq!(
            action_for_key(&folder_envelope_view(), KeyCode::Char('b')),
            Action::BackFromFolderEnvelopes
        );
    }

    #[test]
    fn folder_envelope_j_selects_next() {
        assert_eq!(
            action_for_key(&folder_envelope_view(), KeyCode::Char('j')),
            Action::SelectNext
        );
    }

    #[test]
    fn folder_envelope_enter_reads() {
        assert_eq!(
            action_for_key(&folder_envelope_view(), KeyCode::Enter),
            Action::ReadMessage
        );
    }

    #[test]
    fn folder_envelope_d_deletes() {
        assert_eq!(
            action_for_key(&folder_envelope_view(), KeyCode::Char('d')),
            Action::DeleteMessage
        );
    }

    #[test]
    fn folder_envelope_q_is_none() {
        assert_eq!(
            action_for_key(&folder_envelope_view(), KeyCode::Char('q')),
            Action::None
        );
    }

    // --- Message read view ---

    #[test]
    fn message_esc_goes_back() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Esc),
            Action::BackToList
        );
    }

    #[test]
    fn message_b_goes_back() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Char('b')),
            Action::BackToList
        );
    }

    #[test]
    fn message_j_scrolls_down() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Char('j')),
            Action::ScrollDown
        );
    }

    #[test]
    fn message_k_scrolls_up() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Char('k')),
            Action::ScrollUp
        );
    }

    #[test]
    fn message_d_deletes() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Char('d')),
            Action::DeleteMessage
        );
    }

    #[test]
    fn message_a_archives() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Char('a')),
            Action::ArchiveMessage
        );
    }

    #[test]
    fn message_r_toggles_read() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Char('r')),
            Action::ToggleRead
        );
    }

    #[test]
    fn message_n_next_message() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Char('n')),
            Action::NextMessage
        );
    }

    #[test]
    fn message_f_toggles_flag() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Char('f')),
            Action::ToggleFlag
        );
    }

    #[test]
    fn message_q_is_not_back() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Char('q')),
            Action::None
        );
    }

    #[test]
    fn message_unknown_key_is_none() {
        assert_eq!(
            action_for_key(&message_view(), KeyCode::Char('z')),
            Action::None
        );
    }
}
