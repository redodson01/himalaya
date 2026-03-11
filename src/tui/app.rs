use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as MatcherConfig, Matcher, Utf32Str};
use pimalaya_tui::himalaya::config::Envelope;

/// Sort order for flag characters: Seen, Flagged, Answered, Deleted, Draft.
fn flag_sort_key(c: &char) -> u8 {
    match c {
        'S' => 0,
        'F' => 1,
        'A' => 2,
        'D' => 3,
        'T' => 4,
        _ => 5,
    }
}

/// Sort a flags string into canonical SFADT order.
pub fn sort_flags(flags: &str) -> String {
    let mut chars: Vec<char> = flags.chars().collect();
    chars.sort_by_key(flag_sort_key);
    chars.into_iter().collect()
}

/// Owned envelope data extracted from pimalaya_tui's Envelope type.
pub struct EnvelopeData {
    pub id: String,
    pub subject: String,
    pub from: String,
    pub date: String,
    pub flags: String,
    pub unseen: bool,
    pub flagged: bool,
    pub account: String,
}

/// Tracks a contiguous range of envelopes belonging to one account.
pub struct AccountSection {
    pub name: String,
    pub start: usize,
    pub count: usize,
}

impl From<&Envelope> for EnvelopeData {
    fn from(env: &Envelope) -> Self {
        use pimalaya_tui::himalaya::config::Flag;

        let from = env
            .from
            .name
            .as_deref()
            .unwrap_or(&env.from.addr)
            .to_string();

        let unseen = !env.flags.contains(&Flag::Seen);
        let flagged = env.flags.contains(&Flag::Flagged);

        let flags: String = env
            .flags
            .iter()
            .map(|f| match f {
                Flag::Seen => "S",
                Flag::Answered => "A",
                Flag::Flagged => "F",
                Flag::Deleted => "D",
                Flag::Draft => "T",
                Flag::Custom(_) => "*",
            })
            .collect();
        let flags = sort_flags(&flags);

        Self {
            id: env.id.clone(),
            subject: env.subject.clone(),
            from,
            date: env.date.clone(),
            flags,
            unseen,
            flagged,
            account: String::new(),
        }
    }
}

pub struct FolderEntry {
    pub name: String,
    pub account: String,
}

pub struct FolderSection {
    pub name: String,
    pub start: usize,
    pub count: usize,
}

pub struct SearchState {
    pub query: String,
    pub matched_indices: Vec<usize>,
    pub selected: usize,
}

pub struct FolderListState {
    pub folders: Vec<FolderEntry>,
    pub sections: Vec<FolderSection>,
    pub selected: usize,
    pub saved_envelope_selected: usize,
}

pub struct FolderEnvelopeState {
    pub envelopes: Vec<EnvelopeData>,
    pub selected: usize,
    pub folder_name: String,
    pub account_key: String,
    pub parent: FolderListState,
}

impl FolderEnvelopeState {
    pub fn remove_envelope(&mut self, index: usize) -> Option<EnvelopeData> {
        if index >= self.envelopes.len() {
            return None;
        }
        let removed = self.envelopes.remove(index);
        if !self.envelopes.is_empty() {
            self.selected = self.selected.min(self.envelopes.len() - 1);
        } else {
            self.selected = 0;
        }
        Some(removed)
    }
}

pub struct MoveFolderPickerState {
    pub folders: Vec<FolderEntry>,
    pub selected: usize,
    pub source_envelope_id: String,
    pub source_envelope_index: usize,
    pub source_folder: String,
    pub account_key: String,
    pub return_to_folder: bool,
    pub folder_envelope_state: Option<Box<FolderEnvelopeState>>,
}

pub enum View {
    EnvelopeList,
    MessageRead {
        content: String,
        scroll: u16,
        folder_context: Option<Box<FolderEnvelopeState>>,
    },
    FolderList(FolderListState),
    FolderEnvelopeList(FolderEnvelopeState),
    MoveFolderPicker(MoveFolderPickerState),
}

pub enum Status {
    Working(String),
    Error(String),
}

pub struct App {
    pub envelopes: Vec<EnvelopeData>,
    pub sections: Vec<AccountSection>,
    pub selected: usize,
    pub view: View,
    pub folder: String,
    pub should_quit: bool,
    pub status: Option<Status>,
    pub search: Option<SearchState>,
}

impl App {
    pub fn new(envelopes: Vec<EnvelopeData>, folder: String) -> Self {
        Self {
            envelopes,
            sections: Vec::new(),
            selected: 0,
            view: View::EnvelopeList,
            folder,
            should_quit: false,
            status: None,
            search: None,
        }
    }

    pub fn with_sections(mut self, sections: Vec<AccountSection>) -> Self {
        self.sections = sections;
        self
    }

    pub fn select_next(&mut self) {
        if !self.envelopes.is_empty() {
            self.selected = (self.selected + 1).min(self.envelopes.len() - 1);
        }
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn folder_select_next(&mut self) {
        match &mut self.view {
            View::FolderList(state) => {
                if !state.folders.is_empty() {
                    state.selected = (state.selected + 1).min(state.folders.len() - 1);
                }
            }
            View::MoveFolderPicker(state) => {
                if !state.folders.is_empty() {
                    state.selected = (state.selected + 1).min(state.folders.len() - 1);
                }
            }
            _ => {}
        }
    }

    pub fn folder_select_prev(&mut self) {
        match &mut self.view {
            View::FolderList(state) => {
                state.selected = state.selected.saturating_sub(1);
            }
            View::MoveFolderPicker(state) => {
                state.selected = state.selected.saturating_sub(1);
            }
            _ => {}
        }
    }

    /// Remove the envelope at the given index, updating sections accordingly.
    /// Returns the removed envelope data, or `None` if the index is out of bounds.
    pub fn remove_envelope(&mut self, index: usize) -> Option<EnvelopeData> {
        if index >= self.envelopes.len() {
            return None;
        }

        let removed = self.envelopes.remove(index);

        // Update sections: find the section containing this index
        let mut section_to_remove = None;
        for (si, section) in self.sections.iter_mut().enumerate() {
            if index >= section.start && index < section.start + section.count {
                section.count -= 1;
                if section.count == 0 {
                    section_to_remove = Some(si);
                }
            } else if section.start > index {
                section.start -= 1;
            }
        }

        if let Some(si) = section_to_remove {
            self.sections.remove(si);
        }

        // Clamp selected index
        if !self.envelopes.is_empty() {
            self.selected = self.selected.min(self.envelopes.len() - 1);
        } else {
            self.selected = 0;
        }

        Some(removed)
    }

    pub fn start_search(&mut self) {
        let item_count = match &self.view {
            View::EnvelopeList => self.envelopes.len(),
            View::FolderList(state) => state.folders.len(),
            View::FolderEnvelopeList(state) => state.envelopes.len(),
            View::MoveFolderPicker(state) => state.folders.len(),
            View::MessageRead { .. } => return, // no-op
        };
        self.search = Some(SearchState {
            query: String::new(),
            matched_indices: (0..item_count).collect(),
            selected: 0,
        });
    }

    pub fn cancel_search(&mut self) {
        self.search = None;
    }

    /// Confirms search, maps selection back, returns `true` if a valid item was selected.
    /// Note: search state is kept alive so the filtered view persists during loading.
    /// Callers should call `cancel_search()` after the follow-up action completes.
    pub fn confirm_search(&mut self) -> bool {
        let Some(search) = &self.search else {
            return false;
        };
        if search.matched_indices.is_empty() {
            self.search = None;
            return false;
        }
        let original_index = search.matched_indices[search.selected];
        match &mut self.view {
            View::EnvelopeList => self.selected = original_index,
            View::FolderList(state) => state.selected = original_index,
            View::FolderEnvelopeList(state) => state.selected = original_index,
            View::MoveFolderPicker(state) => state.selected = original_index,
            View::MessageRead { .. } => {}
        }
        true
    }

    pub fn search_push_char(&mut self, c: char) {
        if let Some(search) = &mut self.search {
            search.query.push(c);
            self.recompute_search_matches();
        }
    }

    pub fn search_pop_char(&mut self) {
        if let Some(search) = &mut self.search {
            search.query.pop();
            self.recompute_search_matches();
        }
    }

    pub fn search_select_next(&mut self) {
        if let Some(search) = &mut self.search {
            if !search.matched_indices.is_empty() {
                search.selected = (search.selected + 1).min(search.matched_indices.len() - 1);
            }
        }
    }

    pub fn search_select_prev(&mut self) {
        if let Some(search) = &mut self.search {
            search.selected = search.selected.saturating_sub(1);
        }
    }

    fn recompute_search_matches(&mut self) {
        let search = match &mut self.search {
            Some(s) => s,
            None => return,
        };

        if search.query.is_empty() {
            let len = match &self.view {
                View::EnvelopeList => self.envelopes.len(),
                View::FolderList(state) => state.folders.len(),
                View::FolderEnvelopeList(state) => state.envelopes.len(),
                View::MoveFolderPicker(state) => state.folders.len(),
                View::MessageRead { .. } => 0,
            };
            search.matched_indices = (0..len).collect();
            search.selected = search
                .selected
                .min(search.matched_indices.len().saturating_sub(1));
            return;
        }

        let mut matcher = Matcher::new(MatcherConfig::DEFAULT);
        let pattern = Pattern::new(
            &search.query,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );

        let searchable_texts: Vec<String> = match &self.view {
            View::EnvelopeList => self
                .envelopes
                .iter()
                .map(|e| format!("{} {}", e.subject, e.from))
                .collect(),
            View::FolderList(state) => state.folders.iter().map(|f| f.name.clone()).collect(),
            View::FolderEnvelopeList(state) => state
                .envelopes
                .iter()
                .map(|e| format!("{} {}", e.subject, e.from))
                .collect(),
            View::MoveFolderPicker(state) => state.folders.iter().map(|f| f.name.clone()).collect(),
            View::MessageRead { .. } => Vec::new(),
        };

        let mut buf = Vec::new();
        search.matched_indices = searchable_texts
            .iter()
            .enumerate()
            .filter(|(_, text)| {
                let haystack = Utf32Str::new(text, &mut buf);
                pattern.score(haystack, &mut matcher).is_some()
            })
            .map(|(i, _)| i)
            .collect();

        search.selected = search
            .selected
            .min(search.matched_indices.len().saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_envelope(id: &str, subject: &str) -> EnvelopeData {
        EnvelopeData {
            id: id.to_string(),
            subject: subject.to_string(),
            from: "test@example.com".to_string(),
            date: "2025-01-01 00:00".to_string(),
            flags: String::new(),
            unseen: false,
            flagged: false,
            account: String::new(),
        }
    }

    #[test]
    fn new_app_defaults() {
        let app = App::new(vec![], "INBOX".to_string());
        assert_eq!(app.selected, 0);
        assert_eq!(app.folder, "INBOX");
        assert!(!app.should_quit);
        assert!(matches!(app.view, View::EnvelopeList));
    }

    #[test]
    fn select_next_advances() {
        let envelopes = vec![make_envelope("1", "a"), make_envelope("2", "b")];
        let mut app = App::new(envelopes, "INBOX".to_string());
        assert_eq!(app.selected, 0);
        app.select_next();
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn select_next_clamps_at_end() {
        let envelopes = vec![make_envelope("1", "a"), make_envelope("2", "b")];
        let mut app = App::new(envelopes, "INBOX".to_string());
        app.select_next();
        app.select_next();
        app.select_next();
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn select_next_empty_list() {
        let mut app = App::new(vec![], "INBOX".to_string());
        app.select_next();
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn select_prev_decrements() {
        let envelopes = vec![make_envelope("1", "a"), make_envelope("2", "b")];
        let mut app = App::new(envelopes, "INBOX".to_string());
        app.selected = 1;
        app.select_prev();
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn select_prev_clamps_at_zero() {
        let mut app = App::new(vec![make_envelope("1", "a")], "INBOX".to_string());
        app.select_prev();
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn envelope_data_from_envelope() {
        use pimalaya_tui::himalaya::config::{Envelope, Flag, Flags, Mailbox};

        let env = Envelope {
            id: "42".to_string(),
            subject: "Test Subject".to_string(),
            from: Mailbox {
                name: Some("Alice".to_string()),
                addr: "alice@example.com".to_string(),
            },
            to: Mailbox {
                name: None,
                addr: "bob@example.com".to_string(),
            },
            date: "2025-01-15 10:30".to_string(),
            flags: Flags([Flag::Seen, Flag::Flagged].into_iter().collect()),
            has_attachment: false,
        };

        let data = EnvelopeData::from(&env);
        assert_eq!(data.id, "42");
        assert_eq!(data.subject, "Test Subject");
        assert_eq!(data.from, "Alice");
        assert_eq!(data.date, "2025-01-15 10:30");
        // Flags are from a HashSet so order is nondeterministic
        assert!(data.flags.contains('S'));
        assert!(data.flags.contains('F'));
        assert!(!data.unseen); // has Seen flag
        assert!(data.flagged); // has Flagged flag
    }

    #[test]
    fn envelope_data_uses_addr_when_no_name() {
        use pimalaya_tui::himalaya::config::{Envelope, Flags, Mailbox};

        let env = Envelope {
            id: "1".to_string(),
            subject: String::new(),
            from: Mailbox {
                name: None,
                addr: "bob@example.com".to_string(),
            },
            to: Mailbox {
                name: None,
                addr: String::new(),
            },
            date: String::new(),
            flags: Flags(Default::default()),
            has_attachment: false,
        };

        let data = EnvelopeData::from(&env);
        assert_eq!(data.from, "bob@example.com");
        assert!(data.flags.is_empty());
        assert!(data.unseen); // no Seen flag
        assert!(!data.flagged); // no Flagged flag
    }

    #[test]
    fn multi_section_navigation() {
        let envelopes = vec![
            make_envelope("1", "a"),
            make_envelope("2", "b"),
            make_envelope("3", "c"),
        ];
        let sections = vec![
            AccountSection {
                name: "work".to_string(),
                start: 0,
                count: 2,
            },
            AccountSection {
                name: "personal".to_string(),
                start: 2,
                count: 1,
            },
        ];
        let mut app = App::new(envelopes, "INBOX".to_string()).with_sections(sections);

        assert_eq!(app.selected, 0);
        app.select_next();
        assert_eq!(app.selected, 1);
        app.select_next();
        assert_eq!(app.selected, 2);

        // Clamp at end
        app.select_next();
        assert_eq!(app.selected, 2);
    }

    #[test]
    fn remove_envelope_basic() {
        let envelopes = vec![
            make_envelope("1", "a"),
            make_envelope("2", "b"),
            make_envelope("3", "c"),
        ];
        let mut app = App::new(envelopes, "INBOX".to_string());
        app.selected = 1;
        let removed = app.remove_envelope(1);
        assert_eq!(removed.unwrap().id, "2");
        assert_eq!(app.envelopes.len(), 2);
        assert_eq!(app.selected, 1); // clamped to last item
        assert_eq!(app.envelopes[1].id, "3");
    }

    #[test]
    fn remove_envelope_last_item() {
        let envelopes = vec![make_envelope("1", "a"), make_envelope("2", "b")];
        let mut app = App::new(envelopes, "INBOX".to_string());
        app.selected = 1;
        app.remove_envelope(1);
        assert_eq!(app.envelopes.len(), 1);
        assert_eq!(app.selected, 0); // clamped
    }

    #[test]
    fn remove_envelope_only_item() {
        let envelopes = vec![make_envelope("1", "a")];
        let mut app = App::new(envelopes, "INBOX".to_string());
        app.remove_envelope(0);
        assert!(app.envelopes.is_empty());
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn remove_envelope_out_of_bounds() {
        let mut app = App::new(vec![], "INBOX".to_string());
        assert!(app.remove_envelope(0).is_none());
    }

    #[test]
    fn remove_envelope_updates_sections() {
        let envelopes = vec![
            make_envelope("1", "a"),
            make_envelope("2", "b"),
            make_envelope("3", "c"),
            make_envelope("4", "d"),
        ];
        let sections = vec![
            AccountSection {
                name: "work".to_string(),
                start: 0,
                count: 2,
            },
            AccountSection {
                name: "personal".to_string(),
                start: 2,
                count: 2,
            },
        ];
        let mut app = App::new(envelopes, "INBOX".to_string()).with_sections(sections);

        // Remove from first section
        app.remove_envelope(0);
        assert_eq!(app.sections[0].count, 1);
        assert_eq!(app.sections[0].start, 0);
        assert_eq!(app.sections[1].start, 1); // shifted
        assert_eq!(app.sections[1].count, 2);
    }

    #[test]
    fn remove_envelope_removes_empty_section() {
        let envelopes = vec![make_envelope("1", "a"), make_envelope("2", "b")];
        let sections = vec![
            AccountSection {
                name: "work".to_string(),
                start: 0,
                count: 1,
            },
            AccountSection {
                name: "personal".to_string(),
                start: 1,
                count: 1,
            },
        ];
        let mut app = App::new(envelopes, "INBOX".to_string()).with_sections(sections);

        // Remove only item in first section
        app.remove_envelope(0);
        assert_eq!(app.sections.len(), 1);
        assert_eq!(app.sections[0].name, "personal");
        assert_eq!(app.sections[0].start, 0);
    }

    fn make_folder_list_view(count: usize, selected: usize) -> View {
        let folders = (0..count)
            .map(|i| FolderEntry {
                name: format!("folder{i}"),
                account: String::new(),
            })
            .collect();
        View::FolderList(FolderListState {
            folders,
            sections: Vec::new(),
            selected,
            saved_envelope_selected: 0,
        })
    }

    #[test]
    fn folder_select_next_advances() {
        let mut app = App::new(vec![], "INBOX".to_string());
        app.view = make_folder_list_view(3, 0);
        app.folder_select_next();
        if let View::FolderList(state) = &app.view {
            assert_eq!(state.selected, 1);
        } else {
            panic!("expected FolderList view");
        }
    }

    #[test]
    fn folder_select_next_clamps_at_end() {
        let mut app = App::new(vec![], "INBOX".to_string());
        app.view = make_folder_list_view(2, 1);
        app.folder_select_next();
        app.folder_select_next();
        if let View::FolderList(state) = &app.view {
            assert_eq!(state.selected, 1);
        } else {
            panic!("expected FolderList view");
        }
    }

    #[test]
    fn folder_select_next_empty_list() {
        let mut app = App::new(vec![], "INBOX".to_string());
        app.view = make_folder_list_view(0, 0);
        app.folder_select_next();
        if let View::FolderList(state) = &app.view {
            assert_eq!(state.selected, 0);
        } else {
            panic!("expected FolderList view");
        }
    }

    #[test]
    fn folder_select_prev_decrements() {
        let mut app = App::new(vec![], "INBOX".to_string());
        app.view = make_folder_list_view(3, 2);
        app.folder_select_prev();
        if let View::FolderList(state) = &app.view {
            assert_eq!(state.selected, 1);
        } else {
            panic!("expected FolderList view");
        }
    }

    #[test]
    fn folder_select_prev_clamps_at_zero() {
        let mut app = App::new(vec![], "INBOX".to_string());
        app.view = make_folder_list_view(3, 0);
        app.folder_select_prev();
        if let View::FolderList(state) = &app.view {
            assert_eq!(state.selected, 0);
        } else {
            panic!("expected FolderList view");
        }
    }

    #[test]
    fn folder_select_noop_on_wrong_view() {
        let mut app = App::new(vec![], "INBOX".to_string());
        app.folder_select_next();
        assert!(matches!(app.view, View::EnvelopeList));
        app.folder_select_prev();
        assert!(matches!(app.view, View::EnvelopeList));
    }

    #[test]
    fn folder_envelope_remove_basic() {
        let mut state = FolderEnvelopeState {
            envelopes: vec![
                make_envelope("1", "a"),
                make_envelope("2", "b"),
                make_envelope("3", "c"),
            ],
            selected: 1,
            folder_name: "Sent".to_string(),
            account_key: String::new(),
            parent: FolderListState {
                folders: Vec::new(),
                sections: Vec::new(),
                selected: 0,
                saved_envelope_selected: 0,
            },
        };
        let removed = state.remove_envelope(1);
        assert_eq!(removed.unwrap().id, "2");
        assert_eq!(state.envelopes.len(), 2);
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn folder_envelope_remove_clamps_selected() {
        let mut state = FolderEnvelopeState {
            envelopes: vec![make_envelope("1", "a"), make_envelope("2", "b")],
            selected: 1,
            folder_name: "Sent".to_string(),
            account_key: String::new(),
            parent: FolderListState {
                folders: Vec::new(),
                sections: Vec::new(),
                selected: 0,
                saved_envelope_selected: 0,
            },
        };
        state.remove_envelope(1);
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn folder_envelope_remove_only_item() {
        let mut state = FolderEnvelopeState {
            envelopes: vec![make_envelope("1", "a")],
            selected: 0,
            folder_name: "Sent".to_string(),
            account_key: String::new(),
            parent: FolderListState {
                folders: Vec::new(),
                sections: Vec::new(),
                selected: 0,
                saved_envelope_selected: 0,
            },
        };
        state.remove_envelope(0);
        assert!(state.envelopes.is_empty());
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn folder_envelope_remove_out_of_bounds() {
        let mut state = FolderEnvelopeState {
            envelopes: vec![],
            selected: 0,
            folder_name: "Sent".to_string(),
            account_key: String::new(),
            parent: FolderListState {
                folders: Vec::new(),
                sections: Vec::new(),
                selected: 0,
                saved_envelope_selected: 0,
            },
        };
        assert!(state.remove_envelope(0).is_none());
    }

    #[test]
    fn sort_flags_sfadt_order() {
        assert_eq!(sort_flags("FS"), "SF");
        assert_eq!(sort_flags("DATSF"), "SFADT");
        assert_eq!(sort_flags("S"), "S");
        assert_eq!(sort_flags(""), "");
        assert_eq!(sort_flags("*F"), "F*");
    }

    // --- Search tests ---

    #[test]
    fn start_search_initializes_all_indices() {
        let envelopes = vec![make_envelope("1", "a"), make_envelope("2", "b")];
        let mut app = App::new(envelopes, "INBOX".to_string());
        app.start_search();
        let search = app.search.as_ref().unwrap();
        assert_eq!(search.query, "");
        assert_eq!(search.matched_indices, vec![0, 1]);
        assert_eq!(search.selected, 0);
    }

    #[test]
    fn search_push_char_filters() {
        let envelopes = vec![
            make_envelope("1", "hello world"),
            make_envelope("2", "goodbye"),
            make_envelope("3", "hello there"),
        ];
        let mut app = App::new(envelopes, "INBOX".to_string());
        app.start_search();
        app.search_push_char('h');
        app.search_push_char('e');
        app.search_push_char('l');
        let search = app.search.as_ref().unwrap();
        assert!(search.matched_indices.contains(&0));
        assert!(search.matched_indices.contains(&2));
        assert!(!search.matched_indices.contains(&1));
    }

    #[test]
    fn search_pop_char_widens() {
        let envelopes = vec![
            make_envelope("1", "hello world"),
            make_envelope("2", "goodbye"),
        ];
        let mut app = App::new(envelopes, "INBOX".to_string());
        app.start_search();
        app.search_push_char('h');
        app.search_push_char('e');
        assert_eq!(app.search.as_ref().unwrap().matched_indices.len(), 1);
        app.search_pop_char();
        app.search_pop_char();
        // empty query shows all
        assert_eq!(app.search.as_ref().unwrap().matched_indices.len(), 2);
    }

    #[test]
    fn confirm_search_maps_selection() {
        let envelopes = vec![
            make_envelope("1", "alpha"),
            make_envelope("2", "beta"),
            make_envelope("3", "gamma"),
        ];
        let mut app = App::new(envelopes, "INBOX".to_string());
        app.start_search();
        // Filter to just "beta" (index 1)
        app.search_push_char('b');
        app.search_push_char('e');
        app.search_push_char('t');
        let search = app.search.as_ref().unwrap();
        assert_eq!(search.matched_indices, vec![1]);
        assert_eq!(search.selected, 0);
        assert!(app.confirm_search());
        assert!(app.search.is_some()); // search stays active for loading draw
        assert_eq!(app.selected, 1);
        app.cancel_search(); // caller clears
        assert!(app.search.is_none());
    }

    #[test]
    fn confirm_search_empty_results_noop() {
        let envelopes = vec![make_envelope("1", "hello")];
        let mut app = App::new(envelopes, "INBOX".to_string());
        app.selected = 0;
        app.start_search();
        app.search_push_char('z');
        app.search_push_char('z');
        app.search_push_char('z');
        assert!(app.search.as_ref().unwrap().matched_indices.is_empty());
        assert!(!app.confirm_search());
        assert!(app.search.is_none()); // cleared on empty results
        assert_eq!(app.selected, 0); // unchanged
    }

    #[test]
    fn cancel_search_clears_state() {
        let mut app = App::new(vec![make_envelope("1", "a")], "INBOX".to_string());
        app.start_search();
        assert!(app.search.is_some());
        app.cancel_search();
        assert!(app.search.is_none());
    }

    #[test]
    fn search_select_next_prev() {
        let envelopes = vec![
            make_envelope("1", "aa"),
            make_envelope("2", "ab"),
            make_envelope("3", "ac"),
        ];
        let mut app = App::new(envelopes, "INBOX".to_string());
        app.start_search();
        // All 3 matched
        assert_eq!(app.search.as_ref().unwrap().selected, 0);
        app.search_select_next();
        assert_eq!(app.search.as_ref().unwrap().selected, 1);
        app.search_select_next();
        assert_eq!(app.search.as_ref().unwrap().selected, 2);
        app.search_select_next(); // clamp
        assert_eq!(app.search.as_ref().unwrap().selected, 2);
        app.search_select_prev();
        assert_eq!(app.search.as_ref().unwrap().selected, 1);
        app.search_select_prev();
        assert_eq!(app.search.as_ref().unwrap().selected, 0);
        app.search_select_prev(); // clamp
        assert_eq!(app.search.as_ref().unwrap().selected, 0);
    }

    #[test]
    fn search_empty_list() {
        let mut app = App::new(vec![], "INBOX".to_string());
        app.start_search();
        let search = app.search.as_ref().unwrap();
        assert!(search.matched_indices.is_empty());
        assert_eq!(search.selected, 0);
        // Shouldn't panic
        app.search_select_next();
        app.search_select_prev();
        app.confirm_search();
    }

    // --- MoveFolderPicker tests ---

    fn make_move_picker_view(count: usize, selected: usize) -> View {
        let folders = (0..count)
            .map(|i| FolderEntry {
                name: format!("folder{i}"),
                account: String::new(),
            })
            .collect();
        View::MoveFolderPicker(MoveFolderPickerState {
            folders,
            selected,
            source_envelope_id: "1".to_string(),
            source_envelope_index: 0,
            source_folder: "INBOX".to_string(),
            account_key: String::new(),
            return_to_folder: false,
            folder_envelope_state: None,
        })
    }

    #[test]
    fn move_picker_select_next_advances() {
        let mut app = App::new(vec![], "INBOX".to_string());
        app.view = make_move_picker_view(3, 0);
        app.folder_select_next();
        if let View::MoveFolderPicker(state) = &app.view {
            assert_eq!(state.selected, 1);
        } else {
            panic!("expected MoveFolderPicker view");
        }
    }

    #[test]
    fn move_picker_select_next_clamps() {
        let mut app = App::new(vec![], "INBOX".to_string());
        app.view = make_move_picker_view(2, 1);
        app.folder_select_next();
        if let View::MoveFolderPicker(state) = &app.view {
            assert_eq!(state.selected, 1);
        } else {
            panic!("expected MoveFolderPicker view");
        }
    }

    #[test]
    fn move_picker_select_prev_decrements() {
        let mut app = App::new(vec![], "INBOX".to_string());
        app.view = make_move_picker_view(3, 2);
        app.folder_select_prev();
        if let View::MoveFolderPicker(state) = &app.view {
            assert_eq!(state.selected, 1);
        } else {
            panic!("expected MoveFolderPicker view");
        }
    }

    #[test]
    fn move_picker_select_prev_clamps_at_zero() {
        let mut app = App::new(vec![], "INBOX".to_string());
        app.view = make_move_picker_view(3, 0);
        app.folder_select_prev();
        if let View::MoveFolderPicker(state) = &app.view {
            assert_eq!(state.selected, 0);
        } else {
            panic!("expected MoveFolderPicker view");
        }
    }

    #[test]
    fn move_picker_search_filters_folders() {
        let mut app = App::new(vec![], "INBOX".to_string());
        app.view = make_move_picker_view(3, 0); // folder0, folder1, folder2
        app.start_search();
        app.search_push_char('1'); // should match "folder1"
        let search = app.search.as_ref().unwrap();
        assert!(search.matched_indices.contains(&1));
        assert!(!search.matched_indices.contains(&0) || !search.matched_indices.contains(&2));
    }

    #[test]
    fn move_picker_confirm_search_maps_selection() {
        let mut app = App::new(vec![], "INBOX".to_string());
        app.view = make_move_picker_view(3, 0);
        app.start_search();
        app.search_push_char('2'); // match folder2 at index 2
        assert!(app.confirm_search());
        if let View::MoveFolderPicker(state) = &app.view {
            assert_eq!(state.selected, 2);
        } else {
            panic!("expected MoveFolderPicker view");
        }
    }
}
