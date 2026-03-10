mod app;
mod event;
mod ui;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use color_eyre::Result;
use email::{
    backend::feature::BackendFeatureSource,
    config::Config,
    envelope::list::ListEnvelopesOptions,
    flag::{Flag, Flags},
};
use pimalaya_tui::{himalaya::backend::BackendBuilder, terminal::config::TomlConfig as _};

use crate::config::TomlConfig;

use self::app::{sort_flags, AccountSection, App, EnvelopeData, View};
use self::event::{handle_event, Action};

/// Drop guard that restores the terminal on exit (including panics).
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

pub async fn run(config_paths: &[PathBuf], all: bool) -> Result<()> {
    let config = TomlConfig::from_paths_or_default(config_paths).await?;

    if all {
        run_all_accounts(config).await
    } else {
        run_single_account(config).await
    }
}

async fn run_single_account(config: TomlConfig) -> Result<()> {
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
                .with_add_flags(BackendFeatureSource::Context)
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

    // Store backends keyed by account name for message reading
    let mut backends = HashMap::new();
    let default_account = String::new();
    backends.insert(default_account.clone(), (backend, account_config));

    run_event_loop(
        &mut terminal,
        &mut app,
        &backends,
        &folder,
        &default_account,
    )
    .await
}

async fn run_all_accounts(config: TomlConfig) -> Result<()> {
    let mut account_names: Vec<String> = config.accounts.keys().cloned().collect();
    account_names.sort();

    let mut all_envelopes = Vec::new();
    let mut sections = Vec::new();
    let mut backends = HashMap::new();
    let mut folder = String::from("INBOX");

    for name in &account_names {
        let result = async {
            let (toml_account_config, account_config) = config
                .clone()
                .into_account_configs(Some(name.as_str()), |c: &Config, n| c.account(n).ok())?;

            let toml_account_config = Arc::new(toml_account_config);
            let account_config = Arc::new(account_config);

            let backend =
                BackendBuilder::new(toml_account_config, account_config.clone(), |builder| {
                    builder
                        .without_features()
                        .with_list_envelopes(BackendFeatureSource::Context)
                        .with_get_messages(BackendFeatureSource::Context)
                        .with_add_flags(BackendFeatureSource::Context)
                })
                .without_sending_backend()
                .build()
                .await?;

            let acct_folder = account_config.get_inbox_folder_alias();
            let page_size = account_config.get_envelope_list_page_size();
            let opts = ListEnvelopesOptions {
                page: 0,
                page_size,
                query: None,
            };

            let envelopes = backend.list_envelopes(&acct_folder, opts).await?;

            Ok::<_, color_eyre::eyre::Error>((backend, account_config, acct_folder, envelopes))
        }
        .await;

        match result {
            Ok((backend, account_config, acct_folder, envelopes)) => {
                if folder == "INBOX" && !acct_folder.is_empty() {
                    folder = acct_folder;
                }

                let start = all_envelopes.len();
                let mut envelope_data: Vec<EnvelopeData> =
                    envelopes.iter().map(EnvelopeData::from).collect();
                for env in &mut envelope_data {
                    env.account = name.clone();
                }
                let count = envelope_data.len();
                all_envelopes.extend(envelope_data);

                sections.push(AccountSection {
                    name: name.clone(),
                    start,
                    count,
                });

                backends.insert(name.clone(), (backend, account_config));
            }
            Err(e) => {
                eprintln!("Error loading account {}: {}", name, e);
            }
        }
    }

    let _guard = TerminalGuard;
    let mut terminal = ratatui::init();

    let mut app = App::new(all_envelopes, folder.clone()).with_sections(sections);

    // For multi-account, default_account is empty; we look up per-envelope
    let default_account = String::new();
    run_event_loop(
        &mut terminal,
        &mut app,
        &backends,
        &folder,
        &default_account,
    )
    .await
}

type BackendMap = HashMap<
    String,
    (
        pimalaya_tui::himalaya::backend::Backend,
        Arc<email::account::config::AccountConfig>,
    ),
>;

async fn run_event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    backends: &BackendMap,
    folder: &str,
    default_account: &str,
) -> Result<()> {
    loop {
        terminal.draw(|frame| ui::render(frame, app))?;

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
                    let was_unseen = env.unseen;
                    let account_key = if env.account.is_empty() {
                        default_account
                    } else {
                        &env.account
                    };

                    let content = if let Some((backend, account_config)) = backends.get(account_key)
                    {
                        match id_str.parse::<usize>() {
                            Ok(id) => match backend.get_messages(folder, &[id]).await {
                                Ok(emails) => {
                                    // Mark as seen on the server if previously unseen
                                    if was_unseen {
                                        let seen = Flags::from_iter([Flag::Seen]);
                                        let _ = backend.add_flags(folder, &[id], &seen).await;
                                    }

                                    let mut body = String::new();
                                    for email in emails.to_vec() {
                                        match email.to_read_tpl(account_config, |tpl| tpl).await {
                                            Ok(tpl) => body.push_str(&tpl),
                                            Err(e) => body
                                                .push_str(&format!("Error reading message: {e}")),
                                        }
                                    }
                                    body
                                }
                                Err(e) => format!("Error fetching message: {e}"),
                            },
                            Err(_) => format!("Invalid envelope ID: {id_str}"),
                        }
                    } else {
                        format!("No backend for account: {account_key}")
                    };

                    // Update local state to reflect the message is now seen
                    if was_unseen {
                        if let Some(env) = app.envelopes.get_mut(app.selected) {
                            env.unseen = false;
                            if !env.flags.contains('S') {
                                env.flags = sort_flags(&format!("S{}", env.flags));
                            }
                        }
                    }

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
