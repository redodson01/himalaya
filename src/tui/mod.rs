mod app;
mod event;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;

use color_eyre::Result;
use email::{
    backend::feature::BackendFeatureSource, config::Config, envelope::list::ListEnvelopesOptions,
};
use pimalaya_tui::{himalaya::backend::BackendBuilder, terminal::config::TomlConfig as _};

use crate::config::TomlConfig;

use self::app::{App, EnvelopeData, View};
use self::event::{handle_event, Action};

/// Drop guard that restores the terminal on exit (including panics).
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

pub async fn run(config_paths: &[PathBuf], _all: bool, _account: Option<String>) -> Result<()> {
    let config = TomlConfig::from_paths_or_default(config_paths).await?;

    let (toml_account_config, account_config) = config
        .clone()
        .into_account_configs(None::<&str>, |c: &Config, name| c.account(name).ok())?;

    let toml_account_config = Arc::new(toml_account_config);
    let account_config = Arc::new(account_config);

    let backend = BackendBuilder::new(
        toml_account_config.clone(),
        account_config.clone(),
        |builder| {
            builder
                .without_features()
                .with_list_envelopes(BackendFeatureSource::Context)
                .with_get_messages(BackendFeatureSource::Context)
        },
    )
    .without_sending_backend()
    .build()
    .await?;

    let folder = account_config.get_inbox_folder_alias();

    let page_size = account_config.get_envelope_list_page_size();
    let opts = ListEnvelopesOptions {
        page: 0,
        page_size,
        query: None,
    };

    let envelopes = backend.list_envelopes(&folder, opts).await?;
    let envelope_data: Vec<EnvelopeData> = envelopes.iter().map(EnvelopeData::from).collect();

    let _guard = TerminalGuard;
    let mut terminal = ratatui::init();

    let mut app = App::new(envelope_data, folder.clone());

    loop {
        terminal.draw(|frame| ui::render(frame, &app))?;

        let action = handle_event(&app.view)?;

        match action {
            Action::None => {}
            Action::Quit => {
                app.should_quit = true;
            }
            Action::SelectNext => app.select_next(),
            Action::SelectPrev => app.select_prev(),
            Action::ReadMessage => {
                if let Some(env) = app.envelopes.get(app.selected) {
                    let id_str = env.id.clone();
                    let content = match id_str.parse::<usize>() {
                        Ok(id) => match backend.get_messages(&folder, &[id]).await {
                            Ok(emails) => {
                                let mut body = String::new();
                                for email in emails.to_vec() {
                                    match email.to_read_tpl(&account_config, |tpl| tpl).await {
                                        Ok(tpl) => body.push_str(&tpl),
                                        Err(e) => {
                                            body.push_str(&format!("Error reading message: {e}"))
                                        }
                                    }
                                }
                                body
                            }
                            Err(e) => format!("Error fetching message: {e}"),
                        },
                        Err(_) => format!("Invalid envelope ID: {id_str}"),
                    };
                    app.view = View::MessageRead { content, scroll: 0 };
                }
            }
            Action::BackToList => {
                app.view = View::EnvelopeList;
            }
            Action::ScrollDown => {
                if let View::MessageRead { scroll, .. } = &mut app.view {
                    *scroll = scroll.saturating_add(1);
                }
            }
            Action::ScrollUp => {
                if let View::MessageRead { scroll, .. } = &mut app.view {
                    *scroll = scroll.saturating_sub(1);
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}
