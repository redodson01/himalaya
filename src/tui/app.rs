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
        }
    }
}

pub enum View {
    EnvelopeList,
    MessageRead { content: String, scroll: u16 },
}

pub enum Status {
    Working(String),
    Error(String),
}

pub struct App {
    pub envelopes: Vec<EnvelopeData>,
    pub selected: usize,
    pub view: View,
    pub folder: String,
    pub should_quit: bool,
    pub status: Option<Status>,
}

impl App {
    pub fn new(envelopes: Vec<EnvelopeData>, folder: String) -> Self {
        Self {
            envelopes,
            selected: 0,
            view: View::EnvelopeList,
            folder,
            should_quit: false,
            status: None,
        }
    }

    pub fn select_next(&mut self) {
        if !self.envelopes.is_empty() {
            self.selected = (self.selected + 1).min(self.envelopes.len() - 1);
        }
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// Remove the envelope at the given index.
    /// Returns the removed envelope data, or `None` if the index is out of bounds.
    pub fn remove_envelope(&mut self, index: usize) -> Option<EnvelopeData> {
        if index >= self.envelopes.len() {
            return None;
        }

        let removed = self.envelopes.remove(index);

        // Clamp selected index
        if !self.envelopes.is_empty() {
            self.selected = self.selected.min(self.envelopes.len() - 1);
        } else {
            self.selected = 0;
        }

        Some(removed)
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
    fn sort_flags_sfadt_order() {
        assert_eq!(sort_flags("FS"), "SF");
        assert_eq!(sort_flags("DATSF"), "SFADT");
        assert_eq!(sort_flags("S"), "S");
        assert_eq!(sort_flags(""), "");
        assert_eq!(sort_flags("*F"), "F*");
    }
}
