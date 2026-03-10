use pimalaya_tui::himalaya::config::Envelope;

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

        let flags: Vec<&str> = env
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

        Self {
            id: env.id.clone(),
            subject: env.subject.clone(),
            from,
            date: env.date.clone(),
            flags: flags.join(""),
            unseen,
            flagged,
        }
    }
}

pub enum View {
    EnvelopeList,
    MessageRead { content: String, scroll: u16 },
}

pub struct App {
    pub envelopes: Vec<EnvelopeData>,
    pub selected: usize,
    pub view: View,
    pub folder: String,
    pub should_quit: bool,
}

impl App {
    pub fn new(envelopes: Vec<EnvelopeData>, folder: String) -> Self {
        Self {
            envelopes,
            selected: 0,
            view: View::EnvelopeList,
            folder,
            should_quit: false,
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
}
