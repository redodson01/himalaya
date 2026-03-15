mod app;
mod event;
mod ui;

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

use self::app::{sort_flags, AccountSection, App, EnvelopeData, Status, View};
use self::event::{handle_event, Action};

/// Drop guard that restores the terminal on exit (including panics).
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

pub async fn run(config_paths: &[PathBuf], all: bool, account: Option<String>) -> Result<()> {
    let config = TomlConfig::from_paths_or_default(config_paths).await?;

    if all {
        color_eyre::eyre::bail!("TUI does not support --all yet");
    }

    run_single_account(config, account).await
}

async fn run_single_account(config: TomlConfig, account: Option<String>) -> Result<()> {
    // Determine the account name: use --account flag if provided, otherwise
    // find the account with `default = true`, matching into_account_configs(None) logic.
    let account_name = if let Some(ref name) = account {
        name.clone()
    } else {
        config
            .accounts
            .iter()
            .find(|(_, v)| v.default.unwrap_or(false))
            .map(|(k, _)| k.clone())
            .unwrap_or_else(|| config.accounts.keys().next().cloned().unwrap_or_default())
    };

    let (toml_account_config, account_config) = config
        .clone()
        .into_account_configs(Some(&account_name), |c: &Config, name| c.account(name).ok())?;

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
                .with_remove_flags(BackendFeatureSource::Context)
                .with_delete_messages(BackendFeatureSource::Context)
                .with_copy_messages(BackendFeatureSource::Context)
        },
    )
    .without_sending_backend()
    .build()
    .await?;

    let folder = "INBOX";
    let envelopes = backend
        .list_envelopes(
            folder,
            ListEnvelopesOptions {
                page: 0,
                page_size: 50,
                query: None,
            },
        )
        .await?;

    let envelope_data: Vec<EnvelopeData> = envelopes
        .iter()
        .map(|e| {
            let mut data = EnvelopeData::from(e);
            data.account = account_name.clone();
            data
        })
        .collect();

    let sections = vec![AccountSection {
        name: account_name.clone(),
        start: 0,
        count: envelope_data.len(),
    }];

    let mut app = App::new(envelope_data).with_sections(sections);

    let mut terminal = ratatui::init();
    let _guard = TerminalGuard;

    let archive_folder = account_config.get_folder_alias("archive");

    // Inline event loop
    loop {
        terminal.draw(|frame| ui::render(frame, &app))?;

        let action = handle_event(&app.view, false)?;

        // Clear status on any non-None action
        if action != Action::None {
            app.status = None;
        }

        match action {
            Action::None => {}
            Action::Quit => {
                app.should_quit = true;
            }
            Action::SelectNext => app.select_next(),
            Action::SelectPrev => app.select_prev(),
            Action::ReadMessage => {
                if app.envelopes.is_empty() {
                    continue;
                }
                let env = &app.envelopes[app.selected];
                let id_num: usize = env.id.parse().unwrap_or(0);
                app.status = Some(Status::Working("Loading message…".into()));
                terminal.draw(|frame| ui::render(frame, &app))?;
                app.status = None;

                match backend.get_messages(folder, &[id_num]).await {
                    Ok(msgs) => {
                        if let Some(msg) = msgs.first() {
                            let tpl = msg
                                .to_read_tpl(&account_config, |tpl| {
                                    tpl.with_show_only_headers(
                                        account_config.get_message_read_headers(),
                                    )
                                })
                                .await;

                            let content = match tpl {
                                Ok(t) => t.to_string(),
                                Err(e) => format!("(failed to render: {e})"),
                            };

                            app.view = View::MessageRead { content, scroll: 0 };

                            // Mark as read locally
                            let env = &mut app.envelopes[app.selected];
                            if env.unseen {
                                env.unseen = false;
                                if !env.flags.contains('S') {
                                    env.flags = sort_flags(&format!("{}S", env.flags));
                                }
                            }
                            // Mark as read on server (best-effort)
                            let seen = Flags::from_iter([Flag::Seen]);
                            if let Err(e) = backend.add_flags(folder, &[id_num], &seen).await {
                                tracing::warn!("failed to mark message as read: {e}");
                            }
                        }
                    }
                    Err(e) => {
                        app.status = Some(Status::Error(format!("Failed to load: {e}")));
                    }
                }
            }
            Action::BackToList => {
                app.view = View::MessageList;
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
            Action::NextMessage => {
                if app.selected + 1 < app.envelopes.len() {
                    app.selected += 1;
                    let env = &app.envelopes[app.selected];
                    let id_num: usize = env.id.parse().unwrap_or(0);
                    app.status = Some(Status::Working("Loading message…".into()));
                    terminal.draw(|frame| ui::render(frame, &app))?;
                    app.status = None;

                    match backend.get_messages(folder, &[id_num]).await {
                        Ok(msgs) => {
                            if let Some(msg) = msgs.first() {
                                let tpl = msg
                                    .to_read_tpl(&account_config, |tpl| {
                                        tpl.with_show_only_headers(
                                            account_config.get_message_read_headers(),
                                        )
                                    })
                                    .await;

                                let content = match tpl {
                                    Ok(t) => t.to_string(),
                                    Err(e) => format!("(failed to render: {e})"),
                                };

                                app.view = View::MessageRead { content, scroll: 0 };

                                let env = &mut app.envelopes[app.selected];
                                if env.unseen {
                                    env.unseen = false;
                                    if !env.flags.contains('S') {
                                        env.flags = sort_flags(&format!("{}S", env.flags));
                                    }
                                }
                                let seen = Flags::from_iter([Flag::Seen]);
                                if let Err(e) = backend.add_flags(folder, &[id_num], &seen).await {
                                    tracing::warn!("failed to mark message as read: {e}");
                                }
                            }
                        }
                        Err(e) => {
                            app.status = Some(Status::Error(format!("Failed to load: {e}")));
                        }
                    }
                }
            }
            Action::DeleteMessage => {
                if app.envelopes.is_empty() {
                    continue;
                }
                let env = &app.envelopes[app.selected];
                let id_num: usize = env.id.parse().unwrap_or(0);

                app.status = Some(Status::Working("Deleting…".into()));
                terminal.draw(|frame| ui::render(frame, &app))?;

                match backend.delete_messages(folder, &[id_num]).await {
                    Ok(_) => {
                        app.remove_envelope(app.selected);
                        app.view = View::MessageList;
                        app.status = Some(Status::Info("Deleted".into()));
                    }
                    Err(e) => {
                        app.status = Some(Status::Error(format!("Delete failed: {e}")));
                    }
                }
            }
            Action::ArchiveMessage => {
                if app.envelopes.is_empty() {
                    continue;
                }
                let env = &app.envelopes[app.selected];
                let id_num: usize = env.id.parse().unwrap_or(0);

                app.status = Some(Status::Working("Archiving…".into()));
                terminal.draw(|frame| ui::render(frame, &app))?;

                match backend
                    .copy_messages(folder, &archive_folder, &[id_num])
                    .await
                {
                    Ok(_) => match backend.delete_messages(folder, &[id_num]).await {
                        Ok(_) => {
                            app.remove_envelope(app.selected);
                            app.view = View::MessageList;
                            app.status =
                                Some(Status::Info(format!("Archived to {archive_folder}")));
                        }
                        Err(e) => {
                            app.status =
                                Some(Status::Error(format!("Copied but delete failed: {e}")));
                        }
                    },
                    Err(e) => {
                        app.status = Some(Status::Error(format!("Archive failed: {e}")));
                    }
                }
            }
            Action::ToggleRead => {
                if app.envelopes.is_empty() {
                    continue;
                }
                let env = &app.envelopes[app.selected];
                let id_num: usize = env.id.parse().unwrap_or(0);
                let was_unseen = env.unseen;
                let seen = Flags::from_iter([Flag::Seen]);

                let result = if was_unseen {
                    backend.add_flags(folder, &[id_num], &seen).await
                } else {
                    backend.remove_flags(folder, &[id_num], &seen).await
                };

                match result {
                    Ok(_) => {
                        let env = &mut app.envelopes[app.selected];
                        env.unseen = !was_unseen;
                        if was_unseen {
                            if !env.flags.contains('S') {
                                env.flags = sort_flags(&format!("{}S", env.flags));
                            }
                            app.status = Some(Status::Info("Marked as read".into()));
                        } else {
                            env.flags = env.flags.replace('S', "");
                            app.status = Some(Status::Info("Marked as unread".into()));
                        }
                    }
                    Err(e) => {
                        app.status = Some(Status::Error(format!("Toggle read failed: {e}")));
                    }
                }
            }
            Action::ToggleFlag => {
                if app.envelopes.is_empty() {
                    continue;
                }
                let env = &app.envelopes[app.selected];
                let id_num: usize = env.id.parse().unwrap_or(0);
                let was_flagged = env.flagged;
                let flagged = Flags::from_iter([Flag::Flagged]);

                let result = if was_flagged {
                    backend.remove_flags(folder, &[id_num], &flagged).await
                } else {
                    backend.add_flags(folder, &[id_num], &flagged).await
                };

                match result {
                    Ok(_) => {
                        let env = &mut app.envelopes[app.selected];
                        env.flagged = !was_flagged;
                        if was_flagged {
                            env.flags = env.flags.replace('F', "");
                            app.status = Some(Status::Info("Unflagged".into()));
                        } else {
                            if !env.flags.contains('F') {
                                env.flags = sort_flags(&format!("{}F", env.flags));
                            }
                            app.status = Some(Status::Info("Flagged".into()));
                        }
                    }
                    Err(e) => {
                        app.status = Some(Status::Error(format!("Toggle flag failed: {e}")));
                    }
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}
