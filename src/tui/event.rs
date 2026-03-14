use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};

use crate::tui::app::{FolderContext, View};

#[derive(Debug, PartialEq)]
pub enum Action {
    None,
    Quit,
    ReadMessage,
    BackToList,
    BackFromFolder,
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
    BackFromFolderPicker,
    StartSearch,
    SearchChar(char),
    SearchBackspace,
    SearchConfirm,
    SearchCancel,
    MoveMessage,
    ConfirmMove,
    CancelMove,
    ComposeMessage,
    ReplyMessage,
    ReplyAllMessage,
    ForwardMessage,
    EditMessage,
    ConfirmAccountPicker,
    CancelAccountPicker,
}

pub fn handle_event(
    view: &View,
    folder_context: &FolderContext,
    searching: bool,
) -> color_eyre::Result<Action> {
    if !event::poll(std::time::Duration::from_millis(100))? {
        return Ok(Action::None);
    }

    let Event::Key(key) = event::read()? else {
        return Ok(Action::None);
    };

    if key.kind != KeyEventKind::Press {
        return Ok(Action::None);
    }

    Ok(action_for_key(view, folder_context, key.code, searching))
}

/// Pure mapping from (view, key) to action. Separated from handle_event
/// so it can be unit-tested without terminal I/O.
fn action_for_key(
    view: &View,
    folder_context: &FolderContext,
    key: KeyCode,
    searching: bool,
) -> Action {
    if searching {
        return match key {
            KeyCode::Esc => Action::SearchCancel,
            KeyCode::Enter => Action::SearchConfirm,
            KeyCode::Backspace => Action::SearchBackspace,
            KeyCode::Down => Action::SelectNext,
            KeyCode::Up => Action::SelectPrev,
            KeyCode::Char(c) => Action::SearchChar(c),
            _ => Action::None,
        };
    }

    match view {
        View::MessageList => match key {
            KeyCode::Esc | KeyCode::Char('q') => match folder_context {
                FolderContext::AllInboxes => Action::Quit,
                FolderContext::SingleFolder { .. } => Action::BackFromFolder,
            },
            KeyCode::Down | KeyCode::Char('j') => Action::SelectNext,
            KeyCode::Up | KeyCode::Char('k') => Action::SelectPrev,
            KeyCode::Enter => Action::ReadMessage,
            KeyCode::Char('d') => Action::DeleteMessage,
            KeyCode::Char('a') => Action::ArchiveMessage,
            KeyCode::Char('r') => Action::ToggleRead,
            KeyCode::Char('f') => Action::ToggleFlag,
            KeyCode::Char('m') => Action::MoveMessage,
            KeyCode::Char('N') => Action::ComposeMessage,
            KeyCode::Char('R') => Action::ReplyMessage,
            KeyCode::Char('A') => Action::ReplyAllMessage,
            KeyCode::Char('E') => Action::EditMessage,
            KeyCode::Char('F') => Action::ForwardMessage,
            KeyCode::Char('\\') => Action::OpenFolderList,
            KeyCode::Char('/') => Action::StartSearch,
            _ => Action::None,
        },
        View::FolderList(_) => match key {
            KeyCode::Esc | KeyCode::Char('q') => Action::BackFromFolderPicker,
            KeyCode::Down | KeyCode::Char('j') => Action::FolderSelectNext,
            KeyCode::Up | KeyCode::Char('k') => Action::FolderSelectPrev,
            KeyCode::Enter => Action::SelectFolder,
            KeyCode::Char('/') => Action::StartSearch,
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
            KeyCode::Char('m') => Action::MoveMessage,
            KeyCode::Char('E') => Action::EditMessage,
            KeyCode::Char('N') => Action::ComposeMessage,
            KeyCode::Char('R') => Action::ReplyMessage,
            KeyCode::Char('A') => Action::ReplyAllMessage,
            KeyCode::Char('F') => Action::ForwardMessage,
            _ => Action::None,
        },
        View::MoveFolderPicker(_) => match key {
            KeyCode::Esc | KeyCode::Char('q') => Action::CancelMove,
            KeyCode::Down | KeyCode::Char('j') => Action::FolderSelectNext,
            KeyCode::Up | KeyCode::Char('k') => Action::FolderSelectPrev,
            KeyCode::Enter => Action::ConfirmMove,
            KeyCode::Char('/') => Action::StartSearch,
            _ => Action::None,
        },
        View::AccountPicker(_) => match key {
            KeyCode::Esc | KeyCode::Char('q') => Action::CancelAccountPicker,
            KeyCode::Down | KeyCode::Char('j') => Action::FolderSelectNext,
            KeyCode::Up | KeyCode::Char('k') => Action::FolderSelectPrev,
            KeyCode::Enter => Action::ConfirmAccountPicker,
            KeyCode::Char('/') => Action::StartSearch,
            _ => Action::None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_inboxes() -> FolderContext {
        FolderContext::AllInboxes
    }

    fn single_folder() -> FolderContext {
        FolderContext::SingleFolder {
            folder_name: "Sent".to_string(),
            account_key: "test".to_string(),
        }
    }

    fn list_view() -> View {
        View::MessageList
    }

    fn message_view() -> View {
        View::MessageRead {
            content: String::new(),
            scroll: 0,
        }
    }

    // --- Message list view (AllInboxes) ---

    #[test]
    fn list_q_quits() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Char('q'), false),
            Action::Quit
        );
    }

    #[test]
    fn list_esc_quits() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Esc, false),
            Action::Quit
        );
    }

    #[test]
    fn list_q_back_from_folder_in_single_folder() {
        assert_eq!(
            action_for_key(&list_view(), &single_folder(), KeyCode::Char('q'), false),
            Action::BackFromFolder
        );
    }

    #[test]
    fn list_esc_back_from_folder_in_single_folder() {
        assert_eq!(
            action_for_key(&list_view(), &single_folder(), KeyCode::Esc, false),
            Action::BackFromFolder
        );
    }

    #[test]
    fn list_j_selects_next() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Char('j'), false),
            Action::SelectNext
        );
    }

    #[test]
    fn list_k_selects_prev() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Char('k'), false),
            Action::SelectPrev
        );
    }

    #[test]
    fn list_enter_reads_message() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Enter, false),
            Action::ReadMessage
        );
    }

    #[test]
    fn list_d_deletes() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Char('d'), false),
            Action::DeleteMessage
        );
    }

    #[test]
    fn list_a_archives() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Char('a'), false),
            Action::ArchiveMessage
        );
    }

    #[test]
    fn list_r_toggles_read() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Char('r'), false),
            Action::ToggleRead
        );
    }

    #[test]
    fn list_f_toggles_flag() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Char('f'), false),
            Action::ToggleFlag
        );
    }

    #[test]
    fn list_unknown_key_is_none() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Char('z'), false),
            Action::None
        );
    }

    // --- Envelope list: \ opens folders ---

    #[test]
    fn list_backslash_opens_folders() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Char('\\'), false),
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
        })
    }

    #[test]
    fn folder_j_selects_next() {
        assert_eq!(
            action_for_key(&folder_view(), &all_inboxes(), KeyCode::Char('j'), false),
            Action::FolderSelectNext
        );
    }

    #[test]
    fn folder_k_selects_prev() {
        assert_eq!(
            action_for_key(&folder_view(), &all_inboxes(), KeyCode::Char('k'), false),
            Action::FolderSelectPrev
        );
    }

    #[test]
    fn folder_enter_selects() {
        assert_eq!(
            action_for_key(&folder_view(), &all_inboxes(), KeyCode::Enter, false),
            Action::SelectFolder
        );
    }

    #[test]
    fn folder_esc_goes_back() {
        assert_eq!(
            action_for_key(&folder_view(), &all_inboxes(), KeyCode::Esc, false),
            Action::BackFromFolderPicker
        );
    }

    #[test]
    fn folder_q_goes_back() {
        assert_eq!(
            action_for_key(&folder_view(), &all_inboxes(), KeyCode::Char('q'), false),
            Action::BackFromFolderPicker
        );
    }

    #[test]
    fn folder_b_is_none() {
        assert_eq!(
            action_for_key(&folder_view(), &all_inboxes(), KeyCode::Char('b'), false),
            Action::None
        );
    }

    #[test]
    fn folder_unknown_is_none() {
        assert_eq!(
            action_for_key(&folder_view(), &all_inboxes(), KeyCode::Char('z'), false),
            Action::None
        );
    }

    // --- Message read view ---

    #[test]
    fn message_esc_goes_back() {
        assert_eq!(
            action_for_key(&message_view(), &all_inboxes(), KeyCode::Esc, false),
            Action::BackToList
        );
    }

    #[test]
    fn message_q_goes_back() {
        assert_eq!(
            action_for_key(&message_view(), &all_inboxes(), KeyCode::Char('q'), false),
            Action::BackToList
        );
    }

    #[test]
    fn message_j_scrolls_down() {
        assert_eq!(
            action_for_key(&message_view(), &all_inboxes(), KeyCode::Char('j'), false),
            Action::ScrollDown
        );
    }

    #[test]
    fn message_k_scrolls_up() {
        assert_eq!(
            action_for_key(&message_view(), &all_inboxes(), KeyCode::Char('k'), false),
            Action::ScrollUp
        );
    }

    #[test]
    fn message_d_deletes() {
        assert_eq!(
            action_for_key(&message_view(), &all_inboxes(), KeyCode::Char('d'), false),
            Action::DeleteMessage
        );
    }

    #[test]
    fn message_a_archives() {
        assert_eq!(
            action_for_key(&message_view(), &all_inboxes(), KeyCode::Char('a'), false),
            Action::ArchiveMessage
        );
    }

    #[test]
    fn message_r_toggles_read() {
        assert_eq!(
            action_for_key(&message_view(), &all_inboxes(), KeyCode::Char('r'), false),
            Action::ToggleRead
        );
    }

    #[test]
    fn message_n_next_message() {
        assert_eq!(
            action_for_key(&message_view(), &all_inboxes(), KeyCode::Char('n'), false),
            Action::NextMessage
        );
    }

    #[test]
    fn message_f_toggles_flag() {
        assert_eq!(
            action_for_key(&message_view(), &all_inboxes(), KeyCode::Char('f'), false),
            Action::ToggleFlag
        );
    }

    #[test]
    fn message_b_is_none() {
        assert_eq!(
            action_for_key(&message_view(), &all_inboxes(), KeyCode::Char('b'), false),
            Action::None
        );
    }

    #[test]
    fn message_unknown_key_is_none() {
        assert_eq!(
            action_for_key(&message_view(), &all_inboxes(), KeyCode::Char('z'), false),
            Action::None
        );
    }

    // --- Search key bindings ---

    #[test]
    fn slash_starts_search_in_message_list() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Char('/'), false),
            Action::StartSearch
        );
    }

    #[test]
    fn slash_starts_search_in_folder_list() {
        assert_eq!(
            action_for_key(&folder_view(), &all_inboxes(), KeyCode::Char('/'), false),
            Action::StartSearch
        );
    }

    #[test]
    fn slash_is_none_in_message_read() {
        assert_eq!(
            action_for_key(&message_view(), &all_inboxes(), KeyCode::Char('/'), false),
            Action::None
        );
    }

    #[test]
    fn search_mode_esc_cancels() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Esc, true),
            Action::SearchCancel
        );
    }

    #[test]
    fn search_mode_enter_confirms() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Enter, true),
            Action::SearchConfirm
        );
    }

    #[test]
    fn search_mode_char_becomes_search_char() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Char('x'), true),
            Action::SearchChar('x')
        );
    }

    #[test]
    fn search_mode_backspace() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Backspace, true),
            Action::SearchBackspace
        );
    }

    #[test]
    fn search_mode_j_is_search_char() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Char('j'), true),
            Action::SearchChar('j')
        );
    }

    #[test]
    fn search_mode_k_is_search_char() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Char('k'), true),
            Action::SearchChar('k')
        );
    }

    #[test]
    fn search_mode_down_selects_next() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Down, true),
            Action::SelectNext
        );
    }

    #[test]
    fn search_mode_up_selects_prev() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Up, true),
            Action::SelectPrev
        );
    }

    // --- MoveMessage keybinding ---

    #[test]
    fn list_m_moves_message() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Char('m'), false),
            Action::MoveMessage
        );
    }

    #[test]
    fn message_m_moves_message() {
        assert_eq!(
            action_for_key(&message_view(), &all_inboxes(), KeyCode::Char('m'), false),
            Action::MoveMessage
        );
    }

    // --- MoveFolderPicker keybindings ---

    fn move_picker_view() -> View {
        use crate::tui::app::MoveFolderPickerState;
        View::MoveFolderPicker(MoveFolderPickerState {
            folders: Vec::new(),
            selected: 0,
            source_envelope_id: String::new(),
            source_envelope_index: 0,
            source_folder: String::new(),
            account_key: String::new(),
        })
    }

    #[test]
    fn move_picker_esc_cancels() {
        assert_eq!(
            action_for_key(&move_picker_view(), &all_inboxes(), KeyCode::Esc, false),
            Action::CancelMove
        );
    }

    #[test]
    fn move_picker_q_cancels() {
        assert_eq!(
            action_for_key(
                &move_picker_view(),
                &all_inboxes(),
                KeyCode::Char('q'),
                false
            ),
            Action::CancelMove
        );
    }

    #[test]
    fn move_picker_j_selects_next() {
        assert_eq!(
            action_for_key(
                &move_picker_view(),
                &all_inboxes(),
                KeyCode::Char('j'),
                false
            ),
            Action::FolderSelectNext
        );
    }

    #[test]
    fn move_picker_k_selects_prev() {
        assert_eq!(
            action_for_key(
                &move_picker_view(),
                &all_inboxes(),
                KeyCode::Char('k'),
                false
            ),
            Action::FolderSelectPrev
        );
    }

    #[test]
    fn move_picker_enter_confirms_move() {
        assert_eq!(
            action_for_key(&move_picker_view(), &all_inboxes(), KeyCode::Enter, false),
            Action::ConfirmMove
        );
    }

    #[test]
    fn move_picker_slash_starts_search() {
        assert_eq!(
            action_for_key(
                &move_picker_view(),
                &all_inboxes(),
                KeyCode::Char('/'),
                false
            ),
            Action::StartSearch
        );
    }

    #[test]
    fn move_picker_unknown_is_none() {
        assert_eq!(
            action_for_key(
                &move_picker_view(),
                &all_inboxes(),
                KeyCode::Char('z'),
                false
            ),
            Action::None
        );
    }

    // --- Compose keybindings ---

    #[test]
    fn list_n_composes() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Char('N'), false),
            Action::ComposeMessage
        );
    }

    #[test]
    fn list_r_replies() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Char('R'), false),
            Action::ReplyMessage
        );
    }

    #[test]
    fn list_a_reply_all() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Char('A'), false),
            Action::ReplyAllMessage
        );
    }

    #[test]
    fn list_f_forwards() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Char('F'), false),
            Action::ForwardMessage
        );
    }

    #[test]
    fn message_n_composes() {
        assert_eq!(
            action_for_key(&message_view(), &all_inboxes(), KeyCode::Char('N'), false),
            Action::ComposeMessage
        );
    }

    #[test]
    fn message_r_replies() {
        assert_eq!(
            action_for_key(&message_view(), &all_inboxes(), KeyCode::Char('R'), false),
            Action::ReplyMessage
        );
    }

    #[test]
    fn message_a_reply_all() {
        assert_eq!(
            action_for_key(&message_view(), &all_inboxes(), KeyCode::Char('A'), false),
            Action::ReplyAllMessage
        );
    }

    #[test]
    fn message_f_forwards() {
        assert_eq!(
            action_for_key(&message_view(), &all_inboxes(), KeyCode::Char('F'), false),
            Action::ForwardMessage
        );
    }

    // --- EditMessage keybindings ---

    #[test]
    fn list_e_edits() {
        assert_eq!(
            action_for_key(&list_view(), &all_inboxes(), KeyCode::Char('E'), false),
            Action::EditMessage
        );
    }

    #[test]
    fn message_e_edits() {
        assert_eq!(
            action_for_key(&message_view(), &all_inboxes(), KeyCode::Char('E'), false),
            Action::EditMessage
        );
    }

    // --- AccountPicker keybindings ---

    fn account_picker_view() -> View {
        use crate::tui::app::AccountPickerState;
        View::AccountPicker(AccountPickerState {
            accounts: vec!["work".to_string(), "personal".to_string()],
            selected: 0,
            previous_view: Box::new(View::MessageList),
        })
    }

    #[test]
    fn account_picker_esc_cancels() {
        assert_eq!(
            action_for_key(&account_picker_view(), &all_inboxes(), KeyCode::Esc, false),
            Action::CancelAccountPicker
        );
    }

    #[test]
    fn account_picker_q_cancels() {
        assert_eq!(
            action_for_key(
                &account_picker_view(),
                &all_inboxes(),
                KeyCode::Char('q'),
                false
            ),
            Action::CancelAccountPicker
        );
    }

    #[test]
    fn account_picker_j_selects_next() {
        assert_eq!(
            action_for_key(
                &account_picker_view(),
                &all_inboxes(),
                KeyCode::Char('j'),
                false
            ),
            Action::FolderSelectNext
        );
    }

    #[test]
    fn account_picker_k_selects_prev() {
        assert_eq!(
            action_for_key(
                &account_picker_view(),
                &all_inboxes(),
                KeyCode::Char('k'),
                false
            ),
            Action::FolderSelectPrev
        );
    }

    #[test]
    fn account_picker_enter_confirms() {
        assert_eq!(
            action_for_key(
                &account_picker_view(),
                &all_inboxes(),
                KeyCode::Enter,
                false
            ),
            Action::ConfirmAccountPicker
        );
    }

    #[test]
    fn account_picker_unknown_is_none() {
        assert_eq!(
            action_for_key(
                &account_picker_view(),
                &all_inboxes(),
                KeyCode::Char('z'),
                false
            ),
            Action::None
        );
    }
}
