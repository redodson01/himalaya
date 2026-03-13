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
    folder::list::ListFolders,
    message::Message,
};
use pimalaya_tui::{
    himalaya::{backend::BackendBuilder, editor},
    terminal::{cli::printer::StdoutPrinter, config::TomlConfig as _},
};
use tokio::sync::mpsc;

use crate::config::TomlConfig;

use self::app::{
    sort_flags, AccountPickerState, AccountSection, App, EnvelopeData, FolderContext, FolderEntry,
    FolderListState, FolderSection, MoveFolderPickerState, SavedListState, Status, View,
};
use self::event::{handle_event, Action};

/// Drop guard that restores the terminal on exit (including panics).
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

type Backend = pimalaya_tui::himalaya::backend::Backend;

type BackendMap = HashMap<
    String,
    (
        Arc<Backend>,
        Arc<email::account::config::AccountConfig>,
        String, // source folder
        String, // archive folder
        Arc<crate::account::config::TomlAccountConfig>,
    ),
>;

/// Results from background backend operations, sent via mpsc channel.
enum BackendResult {
    // Backend constructed and ready — arrives during startup
    BackendReady {
        account: String,
        backend: Arc<Backend>,
        account_config: Arc<email::account::config::AccountConfig>,
        folder: String,
        archive_folder: String,
        toml_account_config: Arc<crate::account::config::TomlAccountConfig>,
    },

    // Data loads
    EnvelopesLoaded {
        account: String,
        envelopes: Vec<EnvelopeData>,
    },
    MessageLoaded {
        content: String,
        envelope_id: String,
        was_unseen: bool,
        account_key: String,
        folder: String,
    },
    FoldersLoaded {
        folders: Vec<FolderEntry>,
        sections: Vec<FolderSection>,
    },
    FolderEnvelopesLoaded {
        folder_name: String,
        account_key: String,
        envelopes: Vec<EnvelopeData>,
    },
    MoveFoldersLoaded {
        folders: Vec<FolderEntry>,
        source_envelope_id: String,
        source_envelope_index: usize,
        source_folder: String,
        account_key: String,
    },

    // Mutation succeeded — clears the "Working…" status.
    MutationDone,

    // Errors
    Error {
        message: String,
        /// When true, this error came from a mutation (delete/archive/move/flag)
        /// that was optimistically applied — the UI should auto-refresh to recover.
        needs_refresh: bool,
    },
}

/// Build a backend for an account and send the result through the channel.
fn spawn_backend_connect(
    config: TomlConfig,
    account_name: String,
    account_filter: Option<String>,
    tx: mpsc::UnboundedSender<BackendResult>,
) {
    tokio::spawn(async move {
        let result: Result<_, color_eyre::eyre::Error> = async {
            let (toml_account_config, account_config) = config
                .into_account_configs(account_filter.as_deref(), |c: &Config, name| {
                    c.account(name).ok()
                })?;

            let toml_account_config = Arc::new(toml_account_config);
            let account_config = Arc::new(account_config);

            let backend = BackendBuilder::new(
                toml_account_config.clone(),
                account_config.clone(),
                |builder| {
                    builder
                        .without_features()
                        .with_list_envelopes(BackendFeatureSource::Context)
                        .with_list_folders(BackendFeatureSource::Context)
                        .with_get_messages(BackendFeatureSource::Context)
                        .with_add_flags(BackendFeatureSource::Context)
                        .with_remove_flags(BackendFeatureSource::Context)
                        .with_delete_messages(BackendFeatureSource::Context)
                        .with_move_messages(BackendFeatureSource::Context)
                },
            )
            .without_sending_backend()
            .build()
            .await?;

            let folder = account_config.get_inbox_folder_alias();
            let archive_folder = account_config.get_folder_alias("archive");

            Ok((
                Arc::new(backend),
                account_config,
                folder,
                archive_folder,
                toml_account_config,
            ))
        }
        .await;

        match result {
            Ok((backend, account_config, folder, archive_folder, toml_account_config)) => {
                let _ = tx.send(BackendResult::BackendReady {
                    account: account_name,
                    backend,
                    account_config,
                    folder,
                    archive_folder,
                    toml_account_config,
                });
            }
            Err(e) => {
                let _ = tx.send(BackendResult::Error {
                    message: format!("Failed to connect {account_name}: {e}"),
                    needs_refresh: false,
                });
            }
        }
    });
}

pub async fn run(config_paths: &[PathBuf], all: bool, account: Option<String>) -> Result<()> {
    // Set EDITOR before spawning any tasks, so we don't call set_var in a
    // multi-threaded context (which is unsafe in the 2024 edition).
    if std::env::var("EDITOR").is_err() {
        std::env::set_var("EDITOR", "vi");
    }

    let config = TomlConfig::from_paths_or_default(config_paths).await?;

    if all {
        run_all_accounts(config).await
    } else {
        run_single_account(config, account).await
    }
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
            .find_map(|(name, acct)| acct.default.filter(|&d| d).map(|_| name.clone()))
            .or_else(|| config.accounts.keys().next().cloned())
            .unwrap_or_default()
    };

    // Show TUI immediately with loading status
    let _guard = TerminalGuard;
    let mut terminal = ratatui::init();

    let mut app = App::new(Vec::new(), "INBOX".to_string());
    app.status = Some(Status::Working("Connecting…".to_string()));
    app.pending_refreshes = 1;

    let mut backends = HashMap::new();

    // Create channel and spawn backend construction + envelope load
    let (tx, rx) = mpsc::unbounded_channel();
    spawn_backend_connect(config, account_name.clone(), account, tx.clone());

    run_event_loop(
        &mut terminal,
        &mut app,
        &mut backends,
        &account_name,
        tx,
        rx,
    )
    .await
}

async fn run_all_accounts(config: TomlConfig) -> Result<()> {
    let mut account_names: Vec<String> = config.accounts.keys().cloned().collect();
    account_names.sort();

    // Show TUI immediately with loading status
    let _guard = TerminalGuard;
    let mut terminal = ratatui::init();

    let mut app = App::new(Vec::new(), "INBOX".to_string());
    app.status = Some(Status::Working("Connecting…".to_string()));
    app.pending_refreshes = account_names.len();

    let mut backends = HashMap::new();

    // Create channel and spawn backend construction for each account
    let (tx, rx) = mpsc::unbounded_channel();
    for name in &account_names {
        spawn_backend_connect(config.clone(), name.clone(), Some(name.clone()), tx.clone());
    }

    // For multi-account, default_account is empty; we look up per-envelope
    let default_account = String::new();
    run_event_loop(
        &mut terminal,
        &mut app,
        &mut backends,
        &default_account,
        tx,
        rx,
    )
    .await
}

/// Resolve the account key for the currently selected envelope.
fn account_key_for(app: &App, default_account: &str) -> String {
    app.envelopes
        .get(app.selected)
        .map(|env| {
            if env.account.is_empty() {
                default_account.to_string()
            } else {
                env.account.clone()
            }
        })
        .unwrap_or_else(|| default_account.to_string())
}

/// Envelope identity and context needed by most action handlers.
struct EnvelopeContext {
    id: String,
    unseen: bool,
    flagged: bool,
    is_draft: bool,
    account_key: String,
    folder: String,
}

/// Extract info about the active envelope — always reads from app.envelopes.
fn active_envelope_context(app: &App, default_account: &str) -> Option<EnvelopeContext> {
    app.envelopes.get(app.selected).map(|env| {
        let account_key = match &app.folder_context {
            FolderContext::SingleFolder { account_key, .. } => account_key.clone(),
            FolderContext::AllInboxes => account_key_for(app, default_account),
        };
        let folder = match &app.folder_context {
            FolderContext::SingleFolder { folder_name, .. } => folder_name.clone(),
            FolderContext::AllInboxes => {
                // Find the inbox folder for this account's backend
                // (we don't have direct access to backends here, so use the folder display name)
                app.folder_display_name()
            }
        };
        EnvelopeContext {
            id: env.id.clone(),
            unseen: env.unseen,
            flagged: env.flagged,
            is_draft: env.flags.contains('T'),
            account_key,
            folder,
        }
    })
}

/// Get a mutable reference to the active envelope.
fn active_envelope_mut(app: &mut App) -> Option<&mut EnvelopeData> {
    app.envelopes.get_mut(app.selected)
}

/// Find an envelope by ID in app.envelopes and apply a mutation to it.
/// Returns true if found.
#[allow(dead_code)]
fn mutate_envelope_by_id(app: &mut App, id: &str, f: impl Fn(&mut EnvelopeData)) -> bool {
    if let Some(env) = app.envelopes.iter_mut().find(|e| e.id == id) {
        f(env);
        true
    } else {
        false
    }
}

/// Spawn background tasks to refresh the main envelope list from all backends.
fn spawn_refresh_envelope_list(backends: &BackendMap, tx: &mpsc::UnboundedSender<BackendResult>) {
    for (key, (backend, account_config, folder, _, _)) in backends {
        let tx = tx.clone();
        let backend = backend.clone();
        let account_config = account_config.clone();
        let folder = folder.clone();
        let key = key.clone();
        tokio::spawn(async move {
            let page_size = account_config.get_envelope_list_page_size();
            let opts = ListEnvelopesOptions {
                page: 0,
                page_size,
                query: None,
            };
            match backend.list_envelopes(&folder, opts).await {
                Ok(envelopes) => {
                    let mut envelope_data: Vec<EnvelopeData> =
                        envelopes.iter().map(EnvelopeData::from).collect();
                    for env in &mut envelope_data {
                        env.account = key.clone();
                    }
                    let _ = tx.send(BackendResult::EnvelopesLoaded {
                        account: key,
                        envelopes: envelope_data,
                    });
                }
                Err(e) => {
                    let _ = tx.send(BackendResult::Error {
                        message: format!("Refresh failed: {e}"),
                        needs_refresh: false,
                    });
                }
            }
        });
    }
}

/// Apply a backend result to the app state.
fn apply_backend_result(
    app: &mut App,
    result: BackendResult,
    backends: &mut BackendMap,
    tx: &mpsc::UnboundedSender<BackendResult>,
) {
    match result {
        BackendResult::BackendReady {
            account,
            backend,
            account_config,
            folder,
            archive_folder,
            toml_account_config,
        } => {
            // Spawn envelope load now that the backend is ready
            let tx = tx.clone();
            let backend_clone = backend.clone();
            let account_config_clone = account_config.clone();
            let folder_clone = folder.clone();
            let account_clone = account.clone();
            tokio::spawn(async move {
                let page_size = account_config_clone.get_envelope_list_page_size();
                let opts = ListEnvelopesOptions {
                    page: 0,
                    page_size,
                    query: None,
                };
                match backend_clone.list_envelopes(&folder_clone, opts).await {
                    Ok(envelopes) => {
                        let mut envelope_data: Vec<EnvelopeData> =
                            envelopes.iter().map(EnvelopeData::from).collect();
                        for env in &mut envelope_data {
                            env.account = account_clone.clone();
                        }
                        let _ = tx.send(BackendResult::EnvelopesLoaded {
                            account: account_clone,
                            envelopes: envelope_data,
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(BackendResult::Error {
                            message: format!("Failed to load envelopes for {account_clone}: {e}"),
                            needs_refresh: false,
                        });
                    }
                }
            });

            backends.insert(
                account,
                (
                    backend,
                    account_config,
                    folder,
                    archive_folder,
                    toml_account_config,
                ),
            );
        }
        BackendResult::EnvelopesLoaded { account, envelopes } => {
            // Only apply to the main list when viewing AllInboxes
            if matches!(app.folder_context, FolderContext::AllInboxes) {
                if let Some(si) = app.sections.iter().position(|s| s.name == account) {
                    let section = &app.sections[si];
                    let old_start = section.start;
                    let old_count = section.count;
                    let new_count = envelopes.len();

                    app.envelopes
                        .splice(old_start..old_start + old_count, envelopes.clone());

                    app.sections[si].count = new_count;

                    let diff = new_count as isize - old_count as isize;
                    for s in &mut app.sections {
                        if s.start > old_start {
                            s.start = (s.start as isize + diff) as usize;
                        }
                    }
                } else {
                    let count = envelopes.len();
                    let start = app.envelopes.len();
                    app.envelopes.extend(envelopes.clone());
                    app.sections.push(AccountSection {
                        name: account.clone(),
                        start,
                        count,
                    });
                    resort_sections(app);
                }

                if !app.envelopes.is_empty() {
                    app.selected = app.selected.min(app.envelopes.len() - 1);
                } else {
                    app.selected = 0;
                }

                // Also update the all_inboxes cache
                app.cache.all_inboxes = Some((app.envelopes.clone(), app.sections.clone()));
            } else {
                // Not viewing AllInboxes — update the cache only
                // We need to update the saved cache for when we return
                if let Some((ref mut cached_envs, ref mut cached_sections)) = app.cache.all_inboxes
                {
                    if let Some(si) = cached_sections.iter().position(|s| s.name == account) {
                        let old_start = cached_sections[si].start;
                        let old_count = cached_sections[si].count;
                        let new_count = envelopes.len();
                        cached_envs.splice(old_start..old_start + old_count, envelopes);
                        cached_sections[si].count = new_count;
                        let diff = new_count as isize - old_count as isize;
                        for s in cached_sections.iter_mut() {
                            if s.start > old_start {
                                s.start = (s.start as isize + diff) as usize;
                            }
                        }
                    }
                }
            }

            app.pending_refreshes = app.pending_refreshes.saturating_sub(1);
            if app.pending_refreshes == 0 {
                app.status = None;
            }
        }
        BackendResult::MessageLoaded {
            content,
            envelope_id,
            was_unseen,
            account_key,
            folder,
        } => {
            app.cache
                .messages
                .insert(envelope_id.clone(), content.clone());

            if let View::MessageRead {
                content: view_content,
                ..
            } = &mut app.view
            {
                *view_content = content;
            }
            app.status = None;

            app.last_read_context = if was_unseen {
                Some((account_key, folder, envelope_id))
            } else {
                None
            };
        }
        BackendResult::FoldersLoaded { folders, sections } => {
            app.cache.folders = Some((folders.clone(), sections.clone()));

            if matches!(app.view, View::FolderList(_) | View::MessageList) {
                app.status = None;
                let selected = if let View::FolderList(state) = &app.view {
                    state.selected
                } else {
                    0
                };
                app.view = View::FolderList(FolderListState {
                    folders,
                    sections,
                    selected,
                });
            }
        }
        BackendResult::FolderEnvelopesLoaded {
            folder_name,
            account_key,
            envelopes,
        } => {
            // Cache folder envelopes
            app.cache.folder_envelopes.insert(
                (account_key.clone(), folder_name.clone()),
                envelopes.clone(),
            );

            // If currently viewing this folder, replace envelopes
            let viewing_this_folder = matches!(
                &app.folder_context,
                FolderContext::SingleFolder { folder_name: f, account_key: a }
                    if *f == folder_name && *a == account_key
            );

            if viewing_this_folder && matches!(app.view, View::MessageList) {
                let old_selected = app.selected;
                app.envelopes = envelopes;
                app.sections = vec![AccountSection {
                    name: account_key,
                    start: 0,
                    count: app.envelopes.len(),
                }];
                if !app.envelopes.is_empty() {
                    app.selected = old_selected.min(app.envelopes.len() - 1);
                } else {
                    app.selected = 0;
                }
                app.status = None;
            }
        }
        BackendResult::MoveFoldersLoaded {
            folders,
            source_envelope_id,
            source_envelope_index,
            source_folder,
            account_key,
        } => {
            if folders.is_empty() {
                app.status = Some(Status::Error("No other folders available".to_string()));
            } else {
                app.status = None;
                app.view = View::MoveFolderPicker(MoveFolderPickerState {
                    folders,
                    selected: 0,
                    source_envelope_id,
                    source_envelope_index,
                    source_folder,
                    account_key,
                });
            }
        }
        BackendResult::MutationDone => {
            if matches!(app.status, Some(Status::Working(_))) {
                app.status = None;
            }
            if app.envelopes_stale {
                start_full_refresh(app, backends, tx);
            }
        }
        BackendResult::Error {
            message,
            needs_refresh,
        } => {
            app.status = Some(Status::Error(message));
            app.pending_refreshes = app.pending_refreshes.saturating_sub(1);
            if needs_refresh {
                start_full_refresh(app, backends, tx);
            }
        }
    }
}

/// Re-sort envelope sections by account name and rebuild envelope order.
fn resort_sections(app: &mut App) {
    if app.sections.len() <= 1 {
        return;
    }
    let mut groups: Vec<(String, Vec<EnvelopeData>)> = Vec::new();
    let sections = std::mem::take(&mut app.sections);
    for section in sections.iter().rev() {
        let end = (section.start + section.count).min(app.envelopes.len());
        let envs: Vec<EnvelopeData> = app.envelopes.drain(section.start..end).collect();
        groups.push((section.name.clone(), envs));
    }
    groups.sort_by(|a, b| a.0.cmp(&b.0));
    app.envelopes.clear();
    for (name, envs) in groups {
        let start = app.envelopes.len();
        let count = envs.len();
        app.envelopes.extend(envs);
        app.sections.push(AccountSection { name, start, count });
    }
    if !app.envelopes.is_empty() {
        app.selected = app.selected.min(app.envelopes.len() - 1);
    } else {
        app.selected = 0;
    }
}

enum ComposeKind {
    New,
    Reply,
    ReplyAll,
    Forward,
}

async fn build_compose_backend(
    toml_account_config: &Arc<crate::account::config::TomlAccountConfig>,
    account_config: &Arc<email::account::config::AccountConfig>,
) -> Result<pimalaya_tui::himalaya::backend::Backend> {
    BackendBuilder::new(
        toml_account_config.clone(),
        account_config.clone(),
        |builder| {
            builder
                .without_features()
                .with_add_message(BackendFeatureSource::Context)
                .with_send_message(BackendFeatureSource::Context)
        },
    )
    .build()
    .await
}

async fn handle_compose(
    app: &mut App,
    terminal: &mut ratatui::DefaultTerminal,
    backends: &BackendMap,
    default_account: &str,
    kind: ComposeKind,
    account_override: Option<&str>,
    tx: &mpsc::UnboundedSender<BackendResult>,
) -> Result<()> {
    let (account_key, envelope_id, folder) = match kind {
        ComposeKind::New => {
            let mut key = if let Some(ovr) = account_override {
                ovr.to_string()
            } else {
                account_key_for(app, default_account)
            };
            if !backends.contains_key(&key) {
                key = backends.keys().min().cloned().unwrap_or_default();
            }
            let folder = backends
                .get(&key)
                .map(|(_, _, f, _, _)| f.clone())
                .unwrap_or_default();
            (key, None, folder)
        }
        _ => {
            if let Some(ctx) = active_envelope_context(app, default_account) {
                let id: Option<usize> = ctx.id.parse().ok();
                (ctx.account_key, id, ctx.folder)
            } else {
                app.status = Some(Status::Error("No message selected".to_string()));
                return Ok(());
            }
        }
    };

    let Some((backend, account_config, _, _, toml_account_config)) = backends.get(&account_key)
    else {
        app.status = Some(Status::Error(format!(
            "No backend for account: {account_key}"
        )));
        return Ok(());
    };

    let status_msg = match kind {
        ComposeKind::New => "Composing…",
        ComposeKind::Reply => "Preparing reply…",
        ComposeKind::ReplyAll => "Preparing reply all…",
        ComposeKind::Forward => "Preparing forward…",
    };
    app.status = Some(Status::Working(status_msg.to_string()));
    terminal.draw(|frame| ui::render(frame, app))?;

    let compose_backend = match build_compose_backend(toml_account_config, account_config).await {
        Ok(b) => b,
        Err(e) => {
            app.status = Some(Status::Error(format!("Compose setup failed: {e}")));
            return Ok(());
        }
    };

    let tpl = match kind {
        ComposeKind::New => {
            Message::new_tpl_builder(account_config.clone())
                .build()
                .await
        }
        ComposeKind::Reply | ComposeKind::ReplyAll => {
            let id = envelope_id.ok_or_else(|| color_eyre::eyre::eyre!("Invalid envelope ID"))?;
            let msgs = backend.get_messages(&folder, &[id]).await?;
            let msg = msgs
                .first()
                .ok_or_else(|| color_eyre::eyre::eyre!("Cannot find message {id}"))?;
            msg.to_reply_tpl_builder(account_config.clone())
                .with_reply_all(matches!(kind, ComposeKind::ReplyAll))
                .build()
                .await
        }
        ComposeKind::Forward => {
            let id = envelope_id.ok_or_else(|| color_eyre::eyre::eyre!("Invalid envelope ID"))?;
            let msgs = backend.get_messages(&folder, &[id]).await?;
            let msg = msgs
                .first()
                .ok_or_else(|| color_eyre::eyre::eyre!("Cannot find message {id}"))?;
            msg.to_forward_tpl_builder(account_config.clone())
                .build()
                .await
        }
    };

    let tpl = match tpl {
        Ok(t) => t,
        Err(e) => {
            app.status = Some(Status::Error(format!("Template error: {e}")));
            return Ok(());
        }
    };

    ratatui::restore();

    let mut printer = StdoutPrinter::default();
    let editor_result =
        editor::edit_tpl_with_editor(account_config.clone(), &mut printer, &compose_backend, tpl)
            .await;

    *terminal = ratatui::init();

    match editor_result {
        Ok(()) => {
            if matches!(kind, ComposeKind::Reply | ComposeKind::ReplyAll) {
                if let Some(id) = envelope_id {
                    let backend = backend.clone();
                    let folder = folder.clone();
                    tokio::spawn(async move {
                        let _ = backend.add_flag(&folder, &[id], Flag::Answered).await;
                    });
                }
            }
            app.status = Some(Status::Working("Refreshing…".to_string()));
            start_full_refresh(app, backends, tx);
        }
        Err(e) => {
            app.status = Some(Status::Error(format!("Editor error: {e}")));
        }
    }

    Ok(())
}

async fn handle_edit_message(
    app: &mut App,
    terminal: &mut ratatui::DefaultTerminal,
    backends: &BackendMap,
    default_account: &str,
    tx: &mpsc::UnboundedSender<BackendResult>,
) -> Result<()> {
    let ctx = match active_envelope_context(app, default_account) {
        Some(ctx) => ctx,
        None => {
            app.status = Some(Status::Error("No message selected".to_string()));
            return Ok(());
        }
    };

    let (id_str, account_key, folder, is_draft) =
        (ctx.id, ctx.account_key, ctx.folder, ctx.is_draft);

    let Some((backend, account_config, _, _, toml_account_config)) = backends.get(&account_key)
    else {
        app.status = Some(Status::Error(format!(
            "No backend for account: {account_key}"
        )));
        return Ok(());
    };

    let id: usize = match id_str.parse() {
        Ok(id) => id,
        Err(_) => {
            app.status = Some(Status::Error(format!("Invalid envelope ID: {id_str}")));
            return Ok(());
        }
    };

    app.status = Some(Status::Working("Editing…".to_string()));
    terminal.draw(|frame| ui::render(frame, app))?;

    let tpl = match backend.get_messages(&folder, &[id]).await {
        Ok(msgs) => match msgs.first() {
            Some(msg) => msg.to_read_tpl(account_config, |tpl| tpl).await,
            None => {
                app.status = Some(Status::Error(format!("Cannot find message {id}")));
                return Ok(());
            }
        },
        Err(e) => {
            app.status = Some(Status::Error(format!("Fetch failed: {e}")));
            return Ok(());
        }
    };

    let tpl = match tpl {
        Ok(t) => t,
        Err(e) => {
            app.status = Some(Status::Error(format!("Template error: {e}")));
            return Ok(());
        }
    };

    let compose_backend = match build_compose_backend(toml_account_config, account_config).await {
        Ok(b) => b,
        Err(e) => {
            app.status = Some(Status::Error(format!("Compose setup failed: {e}")));
            return Ok(());
        }
    };

    ratatui::restore();

    let mut printer = StdoutPrinter::default();
    let editor_result =
        editor::edit_tpl_with_editor(account_config.clone(), &mut printer, &compose_backend, tpl)
            .await;

    *terminal = ratatui::init();

    match editor_result {
        Ok(()) => {
            if is_draft {
                // Optimistically remove the draft
                if let Some(pos) = app.envelopes.iter().position(|e| e.id == id_str) {
                    app.remove_envelope(pos);
                }
                // Delete on server in background
                let backend = backend.clone();
                let folder = folder.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    if let Err(e) = backend.delete_messages(&folder, &[id]).await {
                        let _ = tx.send(BackendResult::Error {
                            message: format!("Draft cleanup failed: {e}"),
                            needs_refresh: false,
                        });
                    } else {
                        let _ = tx.send(BackendResult::MutationDone);
                    }
                });
            }
            app.status = Some(Status::Working("Refreshing…".to_string()));
            start_full_refresh(app, backends, tx);
        }
        Err(e) => {
            app.status = Some(Status::Error(format!("Editor error: {e}")));
        }
    }

    Ok(())
}

/// Start a full refresh: context-aware (AllInboxes vs SingleFolder).
fn start_full_refresh(
    app: &mut App,
    backends: &BackendMap,
    tx: &mpsc::UnboundedSender<BackendResult>,
) {
    app.envelopes_stale = false;
    match &app.folder_context {
        FolderContext::AllInboxes => {
            app.pending_refreshes = backends.len();
            spawn_refresh_envelope_list(backends, tx);
        }
        FolderContext::SingleFolder {
            folder_name,
            account_key,
        } => {
            app.pending_refreshes = 1;
            if let Some((backend, account_config, _, _, _)) = backends.get(account_key) {
                let tx = tx.clone();
                let backend = backend.clone();
                let account_config = account_config.clone();
                let folder_name = folder_name.clone();
                let account_key = account_key.clone();
                tokio::spawn(async move {
                    let page_size = account_config.get_envelope_list_page_size();
                    let opts = ListEnvelopesOptions {
                        page: 0,
                        page_size,
                        query: None,
                    };
                    match backend.list_envelopes(&folder_name, opts).await {
                        Ok(envelopes) => {
                            let envelope_data: Vec<EnvelopeData> =
                                envelopes.iter().map(EnvelopeData::from).collect();
                            let _ = tx.send(BackendResult::FolderEnvelopesLoaded {
                                folder_name,
                                account_key,
                                envelopes: envelope_data,
                            });
                        }
                        Err(e) => {
                            let _ = tx.send(BackendResult::Error {
                                message: format!("Error refreshing folder: {e}"),
                                needs_refresh: false,
                            });
                        }
                    }
                });
            }
        }
    }
}

async fn run_event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    backends: &mut BackendMap,
    default_account: &str,
    tx: mpsc::UnboundedSender<BackendResult>,
    mut rx: mpsc::UnboundedReceiver<BackendResult>,
) -> Result<()> {
    loop {
        terminal.draw(|frame| ui::render(frame, app))?;

        // 1. Process all pending backend results (non-blocking)
        while let Ok(result) = rx.try_recv() {
            apply_backend_result(app, result, backends, &tx);

            if let Some((ref account_key, ref folder, ref envelope_id)) = app.last_read_context {
                if let Some((backend, _, _, _, _)) = backends.get(account_key) {
                    if let Ok(id) = envelope_id.parse::<usize>() {
                        let tx = tx.clone();
                        let backend = backend.clone();
                        let folder = folder.clone();
                        tokio::spawn(async move {
                            let seen = Flags::from_iter([Flag::Seen]);
                            let _ = backend.add_flags(&folder, &[id], &seen).await;
                            let _ = tx.send(BackendResult::MutationDone);
                        });
                    }
                }
                app.last_read_context = None;
            }
        }

        // 2. Check for user input (100ms poll)
        let mut action = handle_event(&app.view, &app.folder_context, app.search.is_some())?;

        // Clear transient status on any keypress
        if !matches!(action, Action::None)
            && app.status.is_some()
            && (app.pending_refreshes == 0 || !matches!(app.status, Some(Status::Working(_))))
        {
            app.status = None;
        }

        // This loop allows SearchConfirm to set a follow-up action and re-enter.
        let mut clear_search_after = false;
        loop {
            match action {
                Action::None => {}
                Action::Quit => {
                    app.should_quit = true;
                }
                Action::BackToAllInboxes => {
                    // Return from SingleFolder to AllInboxes
                    // Cache current folder envelopes
                    if let FolderContext::SingleFolder {
                        ref folder_name,
                        ref account_key,
                    } = app.folder_context
                    {
                        app.cache.folder_envelopes.insert(
                            (account_key.clone(), folder_name.clone()),
                            app.envelopes.clone(),
                        );
                    }
                    // Restore from saved_list_state or cache
                    if let Some(saved) = app.saved_list_state.take() {
                        app.envelopes = saved.envelopes;
                        app.sections = saved.sections;
                        app.selected = saved.selected;
                        app.folder_context = saved.folder_context;
                    } else if let Some((envs, secs)) = app.cache.all_inboxes.clone() {
                        app.envelopes = envs;
                        app.sections = secs;
                        app.selected = 0;
                        app.folder_context = FolderContext::AllInboxes;
                    } else {
                        app.envelopes.clear();
                        app.sections.clear();
                        app.selected = 0;
                        app.folder_context = FolderContext::AllInboxes;
                    }
                    app.view = View::MessageList;
                    app.search = None;
                    // Always refresh: the all-inboxes data may be stale if the
                    // user mutated messages while viewing the folder.
                    app.status = Some(Status::Working("Refreshing…".to_string()));
                    start_full_refresh(app, backends, &tx);
                }
                Action::SelectNext => {
                    if app.search.is_some() {
                        app.search_select_next();
                    } else {
                        app.select_next();
                    }
                }
                Action::SelectPrev => {
                    if app.search.is_some() {
                        app.search_select_prev();
                    } else {
                        app.select_prev();
                    }
                }
                Action::ReadMessage | Action::NextMessage => {
                    if matches!(action, Action::NextMessage) {
                        let prev = app.selected;
                        app.select_next();
                        if app.selected == prev {
                            app.status = Some(Status::Working("No more messages".to_string()));
                            continue;
                        }
                    }

                    if let Some(ctx) = active_envelope_context(app, default_account) {
                        app.status = Some(Status::Working("Loading…".to_string()));

                        if ctx.unseen {
                            if let Some(env) = active_envelope_mut(app) {
                                env.unseen = false;
                                if !env.flags.contains('S') {
                                    env.flags = sort_flags(&format!("S{}", env.flags));
                                }
                            }
                        }

                        let cached_content =
                            app.cache.messages.get(&ctx.id).cloned().unwrap_or_default();

                        app.view = View::MessageRead {
                            content: cached_content,
                            scroll: 0,
                        };

                        let tx = tx.clone();
                        let envelope_id = ctx.id.clone();
                        let was_unseen = ctx.unseen;
                        let account_key = ctx.account_key.clone();
                        let folder = ctx.folder.clone();

                        if let Some((backend, account_config, _, _, _)) =
                            backends.get(&ctx.account_key)
                        {
                            let backend = backend.clone();
                            let account_config = account_config.clone();
                            let ctx_folder = ctx.folder.clone();
                            let ctx_id = ctx.id.clone();
                            tokio::spawn(async move {
                                let content = match ctx_id.parse::<usize>() {
                                    Ok(id) => {
                                        match backend.get_messages(&ctx_folder, &[id]).await {
                                            Ok(emails) => {
                                                let mut body = String::new();
                                                for email in emails.to_vec() {
                                                    match email
                                                        .to_read_tpl(&account_config, |tpl| tpl)
                                                        .await
                                                    {
                                                        Ok(tpl) => body.push_str(&tpl),
                                                        Err(e) => body.push_str(&format!(
                                                            "Error reading message: {e}"
                                                        )),
                                                    }
                                                }
                                                body
                                            }
                                            Err(e) => format!("Error fetching message: {e}"),
                                        }
                                    }
                                    Err(_) => format!("Invalid envelope ID: {ctx_id}"),
                                };

                                let _ = tx.send(BackendResult::MessageLoaded {
                                    content,
                                    envelope_id,
                                    was_unseen,
                                    account_key,
                                    folder,
                                });
                            });
                        }
                    }
                }
                Action::BackToList => {
                    app.view = View::MessageList;
                    if app.envelopes_stale {
                        app.status = Some(Status::Working("Refreshing…".to_string()));
                        start_full_refresh(app, backends, &tx);
                    }
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
                Action::DeleteMessage => {
                    if let Some(ctx) = active_envelope_context(app, default_account) {
                        let (id_str, account_key, folder) = (ctx.id, ctx.account_key, ctx.folder);

                        // Optimistic local update
                        app.remove_envelope(app.selected);
                        if !matches!(app.view, View::MessageList) {
                            app.view = View::MessageList;
                        }

                        app.status = Some(Status::Working("Deleting…".to_string()));
                        app.envelopes_stale = true;

                        if let Some((backend, _, _, _, _)) = backends.get(&account_key) {
                            if let Ok(id) = id_str.parse::<usize>() {
                                let tx = tx.clone();
                                let backend = backend.clone();
                                let folder = folder.clone();
                                tokio::spawn(async move {
                                    match backend.delete_messages(&folder, &[id]).await {
                                        Ok(_) => {
                                            let _ = tx.send(BackendResult::MutationDone);
                                        }
                                        Err(e) => {
                                            let _ = tx.send(BackendResult::Error {
                                                message: format!("Delete failed: {e}"),
                                                needs_refresh: true,
                                            });
                                        }
                                    }
                                });
                            }
                        }
                    }
                }
                Action::ToggleRead => {
                    if let Some(ctx) = active_envelope_context(app, default_account) {
                        if let Some(env) = active_envelope_mut(app) {
                            if ctx.unseen {
                                env.unseen = false;
                                if !env.flags.contains('S') {
                                    env.flags = sort_flags(&format!("S{}", env.flags));
                                }
                            } else {
                                env.unseen = true;
                                env.flags = env.flags.replace('S', "");
                            }
                        }
                        if !matches!(app.view, View::MessageList) {
                            app.view = View::MessageList;
                        }
                        app.status = Some(Status::Working("Updating…".to_string()));

                        if let Some((backend, _, _, _, _)) = backends.get(&ctx.account_key) {
                            if let Ok(id) = ctx.id.parse::<usize>() {
                                let tx = tx.clone();
                                let backend = backend.clone();
                                let folder = ctx.folder.clone();
                                let was_unseen = ctx.unseen;
                                tokio::spawn(async move {
                                    let seen = Flags::from_iter([Flag::Seen]);
                                    let result = if was_unseen {
                                        backend.add_flags(&folder, &[id], &seen).await
                                    } else {
                                        backend.remove_flags(&folder, &[id], &seen).await
                                    };
                                    match result {
                                        Ok(_) => {
                                            let _ = tx.send(BackendResult::MutationDone);
                                        }
                                        Err(e) => {
                                            let _ = tx.send(BackendResult::Error {
                                                message: format!("Toggle read failed: {e}"),
                                                needs_refresh: true,
                                            });
                                        }
                                    }
                                });
                            }
                        }
                    }
                }
                Action::ToggleFlag => {
                    if let Some(ctx) = active_envelope_context(app, default_account) {
                        if let Some(env) = active_envelope_mut(app) {
                            env.flagged = !ctx.flagged;
                            if ctx.flagged {
                                env.flags = env.flags.replace('F', "");
                            } else if !env.flags.contains('F') {
                                env.flags = sort_flags(&format!("F{}", env.flags));
                            }
                        }
                        if !matches!(app.view, View::MessageList) {
                            app.view = View::MessageList;
                        }
                        app.status = Some(Status::Working("Updating…".to_string()));

                        if let Some((backend, _, _, _, _)) = backends.get(&ctx.account_key) {
                            if let Ok(id) = ctx.id.parse::<usize>() {
                                let tx = tx.clone();
                                let backend = backend.clone();
                                let folder = ctx.folder.clone();
                                let was_flagged = ctx.flagged;
                                tokio::spawn(async move {
                                    let flagged = Flags::from_iter([Flag::Flagged]);
                                    let result = if was_flagged {
                                        backend.remove_flags(&folder, &[id], &flagged).await
                                    } else {
                                        backend.add_flags(&folder, &[id], &flagged).await
                                    };
                                    match result {
                                        Ok(_) => {
                                            let _ = tx.send(BackendResult::MutationDone);
                                        }
                                        Err(e) => {
                                            let _ = tx.send(BackendResult::Error {
                                                message: format!("Flag toggle failed: {e}"),
                                                needs_refresh: true,
                                            });
                                        }
                                    }
                                });
                            }
                        }
                    }
                }
                Action::OpenFolderList => {
                    // Save current state for restoration
                    app.saved_list_state = Some(SavedListState {
                        folder_context: app.folder_context.clone(),
                        envelopes: app.envelopes.clone(),
                        sections: app.sections.clone(),
                        selected: app.selected,
                    });

                    // Also cache AllInboxes state if that's what we're viewing
                    if matches!(app.folder_context, FolderContext::AllInboxes) {
                        app.cache.all_inboxes = Some((app.envelopes.clone(), app.sections.clone()));
                    }

                    app.search = None;

                    // Show cached folders immediately if available
                    if let Some((folders, sections)) = &app.cache.folders {
                        app.view = View::FolderList(FolderListState {
                            folders: folders.clone(),
                            sections: sections.clone(),
                            selected: 0,
                        });
                        app.status = Some(Status::Working("Refreshing folders…".to_string()));
                    } else {
                        app.status = Some(Status::Working("Loading folders…".to_string()));
                    }
                    let tx = tx.clone();

                    let mut keys: Vec<String> = backends.keys().cloned().collect();
                    keys.sort();

                    let backend_entries: Vec<_> = keys
                        .iter()
                        .filter_map(|key| {
                            backends
                                .get(key)
                                .map(|(b, _, _, _, _)| (key.clone(), b.clone()))
                        })
                        .collect();

                    tokio::spawn(async move {
                        let mut folders = Vec::new();
                        let mut sections = Vec::new();

                        for (key, backend) in backend_entries {
                            match backend.list_folders().await {
                                Ok(account_folders) => {
                                    let start = folders.len();
                                    let account_folders: Vec<email::folder::Folder> =
                                        account_folders.into();
                                    let count = account_folders.len();
                                    for f in account_folders {
                                        folders.push(FolderEntry {
                                            name: f.name,
                                            account: key.clone(),
                                        });
                                    }
                                    sections.push(FolderSection {
                                        name: key.clone(),
                                        start,
                                        count,
                                    });
                                }
                                Err(e) => {
                                    let _ = tx.send(BackendResult::Error {
                                        message: format!("Error loading folders for {key}: {e}"),
                                        needs_refresh: false,
                                    });
                                    return;
                                }
                            }
                        }

                        let _ = tx.send(BackendResult::FoldersLoaded { folders, sections });
                    });
                }
                Action::FolderSelectNext => {
                    if app.search.is_some() {
                        app.search_select_next();
                    } else {
                        app.folder_select_next();
                    }
                }
                Action::FolderSelectPrev => {
                    if app.search.is_some() {
                        app.search_select_prev();
                    } else {
                        app.folder_select_prev();
                    }
                }
                Action::BackFromFolders => {
                    // Restore saved state
                    if let Some(saved) = app.saved_list_state.take() {
                        app.envelopes = saved.envelopes;
                        app.sections = saved.sections;
                        app.selected = saved.selected;
                        app.folder_context = saved.folder_context;
                    }
                    app.view = View::MessageList;
                    app.search = None;
                    if app.envelopes_stale {
                        app.status = Some(Status::Working("Refreshing…".to_string()));
                        start_full_refresh(app, backends, &tx);
                    }
                }
                Action::CancelMove => {
                    app.view = View::MessageList;
                    app.search = None;
                }
                Action::SelectFolder => {
                    let folder_info = if let View::FolderList(state) = &app.view {
                        state
                            .folders
                            .get(state.selected)
                            .map(|f| (f.name.clone(), f.account.clone()))
                    } else {
                        None
                    };

                    if let Some((folder_name, account_key)) = folder_info {
                        let key = if !account_key.is_empty() {
                            account_key.clone()
                        } else if backends.contains_key(default_account) {
                            default_account.to_string()
                        } else {
                            backends.keys().next().cloned().unwrap_or_default()
                        };

                        if let Some((backend, account_config, _, _, _)) = backends.get(&key) {
                            // Set folder context to the selected folder
                            app.folder_context = FolderContext::SingleFolder {
                                folder_name: folder_name.clone(),
                                account_key: key.clone(),
                            };

                            // Show cached folder envelopes immediately if available
                            if let Some(cached) = app
                                .cache
                                .folder_envelopes
                                .get(&(key.clone(), folder_name.clone()))
                            {
                                app.envelopes = cached.clone();
                                app.sections = vec![AccountSection {
                                    name: key.clone(),
                                    start: 0,
                                    count: app.envelopes.len(),
                                }];
                                app.selected = 0;
                                app.view = View::MessageList;
                                app.status =
                                    Some(Status::Working(format!("Refreshing {folder_name}…")));
                            } else {
                                app.envelopes.clear();
                                app.sections.clear();
                                app.selected = 0;
                                app.view = View::MessageList;
                                app.status =
                                    Some(Status::Working(format!("Loading {folder_name}…")));
                            }

                            app.search = None;

                            let tx = tx.clone();
                            let backend = backend.clone();
                            let account_config = account_config.clone();
                            let folder_name_clone = folder_name.clone();
                            let key_clone = key.clone();
                            tokio::spawn(async move {
                                let page_size = account_config.get_envelope_list_page_size();
                                let opts = ListEnvelopesOptions {
                                    page: 0,
                                    page_size,
                                    query: None,
                                };
                                match backend.list_envelopes(&folder_name_clone, opts).await {
                                    Ok(envelopes) => {
                                        let envelope_data: Vec<EnvelopeData> =
                                            envelopes.iter().map(EnvelopeData::from).collect();
                                        let _ = tx.send(BackendResult::FolderEnvelopesLoaded {
                                            folder_name: folder_name_clone,
                                            account_key: key_clone,
                                            envelopes: envelope_data,
                                        });
                                    }
                                    Err(e) => {
                                        let _ = tx.send(BackendResult::Error {
                                            message: format!("Error loading envelopes: {e}"),
                                            needs_refresh: false,
                                        });
                                    }
                                }
                            });
                        }
                    }
                }
                Action::StartSearch => {
                    app.start_search();
                }
                Action::SearchChar(c) => {
                    app.search_push_char(c);
                }
                Action::SearchBackspace => {
                    app.search_pop_char();
                }
                Action::SearchConfirm => {
                    let follow_up = match &app.view {
                        View::MessageList => Some(Action::ReadMessage),
                        View::FolderList(_) => Some(Action::SelectFolder),
                        View::MoveFolderPicker(_) => Some(Action::ConfirmMove),
                        _ => None,
                    };
                    if app.confirm_search() {
                        if let Some(next_action) = follow_up {
                            action = next_action;
                            clear_search_after = true;
                            continue;
                        }
                    }
                    app.cancel_search();
                }
                Action::SearchCancel => {
                    app.cancel_search();
                }
                Action::ArchiveMessage => {
                    if let Some(ctx) = active_envelope_context(app, default_account) {
                        let (id_str, account_key, source_folder) =
                            (ctx.id, ctx.account_key, ctx.folder);

                        // Optimistic local update
                        app.remove_envelope(app.selected);
                        if !matches!(app.view, View::MessageList) {
                            app.view = View::MessageList;
                        }
                        app.status = Some(Status::Working("Archiving…".to_string()));
                        app.envelopes_stale = true;

                        if let Some((backend, _, _, archive_folder, _)) = backends.get(&account_key)
                        {
                            if let Ok(id) = id_str.parse::<usize>() {
                                let tx = tx.clone();
                                let backend = backend.clone();
                                let source_folder = source_folder.clone();
                                let archive_folder = archive_folder.clone();
                                tokio::spawn(async move {
                                    match backend
                                        .move_messages(&source_folder, &archive_folder, &[id])
                                        .await
                                    {
                                        Ok(_) => {
                                            let _ = tx.send(BackendResult::MutationDone);
                                        }
                                        Err(e) => {
                                            let _ = tx.send(BackendResult::Error {
                                                message: format!("Archive failed: {e}"),
                                                needs_refresh: true,
                                            });
                                        }
                                    }
                                });
                            }
                        }
                    }
                }
                Action::MoveMessage => {
                    if let Some(ctx) = active_envelope_context(app, default_account) {
                        let (id_str, account_key, source_folder, env_index) = (
                            ctx.id.clone(),
                            ctx.account_key.clone(),
                            ctx.folder.clone(),
                            app.selected,
                        );

                        app.status = Some(Status::Working("Loading folders…".to_string()));

                        if matches!(app.view, View::MessageRead { .. }) {
                            app.view = View::MessageList;
                        }

                        if let Some((backend, _, _, _, _)) = backends.get(&account_key) {
                            let tx = tx.clone();
                            let backend = backend.clone();
                            let source_folder_clone = source_folder.clone();
                            let account_key_clone = account_key.clone();
                            tokio::spawn(async move {
                                match backend.list_folders().await {
                                    Ok(account_folders) => {
                                        let account_folders: Vec<email::folder::Folder> =
                                            account_folders.into();
                                        let folders: Vec<FolderEntry> = account_folders
                                            .into_iter()
                                            .filter(|f| f.name != source_folder_clone)
                                            .map(|f| FolderEntry {
                                                name: f.name,
                                                account: account_key_clone.clone(),
                                            })
                                            .collect();
                                        let _ = tx.send(BackendResult::MoveFoldersLoaded {
                                            folders,
                                            source_envelope_id: id_str,
                                            source_envelope_index: env_index,
                                            source_folder: source_folder_clone,
                                            account_key: account_key_clone,
                                        });
                                    }
                                    Err(e) => {
                                        let _ = tx.send(BackendResult::Error {
                                            message: format!("Error loading folders: {e}"),
                                            needs_refresh: false,
                                        });
                                    }
                                }
                            });
                        }
                    }
                }
                Action::ConfirmMove => {
                    if let View::MoveFolderPicker(ref state) = app.view {
                        if let Some(target) = state.folders.get(state.selected) {
                            let target_name = target.name.clone();
                            let source_folder = state.source_folder.clone();
                            let id_str = state.source_envelope_id.clone();
                            let account_key = state.account_key.clone();
                            let source_envelope_index = state.source_envelope_index;

                            // Optimistic local update
                            app.view = View::MessageList;
                            app.remove_envelope(source_envelope_index);

                            app.status = Some(Status::Working(format!("Moved to {target_name}")));
                            app.envelopes_stale = true;

                            if let Some((backend, _, _, _, _)) = backends.get(&account_key) {
                                if let Ok(id) = id_str.parse::<usize>() {
                                    let tx = tx.clone();
                                    let backend = backend.clone();
                                    let target_name_clone = target_name.clone();
                                    let source_folder = source_folder.clone();
                                    tokio::spawn(async move {
                                        match backend
                                            .move_messages(
                                                &source_folder,
                                                &target_name_clone,
                                                &[id],
                                            )
                                            .await
                                        {
                                            Ok(_) => {
                                                let _ = tx.send(BackendResult::MutationDone);
                                            }
                                            Err(e) => {
                                                let _ = tx.send(BackendResult::Error {
                                                    message: format!("Move failed: {e}"),
                                                    needs_refresh: true,
                                                });
                                            }
                                        }
                                    });
                                }
                            }
                        }
                    }
                }
                Action::EditMessage => {
                    handle_edit_message(app, terminal, backends, default_account, &tx)
                        .await
                        .ok();
                }
                Action::ComposeMessage => {
                    if backends.len() > 1 {
                        let mut accounts: Vec<String> = backends.keys().cloned().collect();
                        accounts.sort();
                        let previous_view = std::mem::replace(&mut app.view, View::MessageList);
                        app.view = View::AccountPicker(AccountPickerState {
                            accounts,
                            selected: 0,
                            previous_view: Box::new(previous_view),
                        });
                    } else {
                        handle_compose(
                            app,
                            terminal,
                            backends,
                            default_account,
                            ComposeKind::New,
                            None,
                            &tx,
                        )
                        .await
                        .ok();
                    }
                }
                Action::ConfirmAccountPicker => {
                    if let View::AccountPicker(state) =
                        std::mem::replace(&mut app.view, View::MessageList)
                    {
                        if let Some(account_key) = state.accounts.get(state.selected) {
                            let key = account_key.clone();
                            app.view = *state.previous_view;
                            handle_compose(
                                app,
                                terminal,
                                backends,
                                default_account,
                                ComposeKind::New,
                                Some(&key),
                                &tx,
                            )
                            .await
                            .ok();
                        }
                    }
                }
                Action::CancelAccountPicker => {
                    if let View::AccountPicker(state) =
                        std::mem::replace(&mut app.view, View::MessageList)
                    {
                        app.view = *state.previous_view;
                    }
                }
                Action::ReplyMessage => {
                    handle_compose(
                        app,
                        terminal,
                        backends,
                        default_account,
                        ComposeKind::Reply,
                        None,
                        &tx,
                    )
                    .await
                    .ok();
                }
                Action::ReplyAllMessage => {
                    handle_compose(
                        app,
                        terminal,
                        backends,
                        default_account,
                        ComposeKind::ReplyAll,
                        None,
                        &tx,
                    )
                    .await
                    .ok();
                }
                Action::ForwardMessage => {
                    handle_compose(
                        app,
                        terminal,
                        backends,
                        default_account,
                        ComposeKind::Forward,
                        None,
                        &tx,
                    )
                    .await
                    .ok();
                }
            }

            if clear_search_after {
                app.cancel_search();
            }
            break;
        } // end inner action loop

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use self::app::{
        AccountSection, App, EnvelopeData, FolderContext, FolderEntry, FolderSection, Status, View,
    };
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

    fn make_envelope_for(id: &str, subject: &str, account: &str) -> EnvelopeData {
        let mut env = make_envelope(id, subject);
        env.account = account.to_string();
        env
    }

    fn test_fixtures() -> (
        BackendMap,
        mpsc::UnboundedSender<BackendResult>,
        mpsc::UnboundedReceiver<BackendResult>,
    ) {
        let backends = BackendMap::new();
        let (tx, rx) = mpsc::unbounded_channel();
        (backends, tx, rx)
    }

    fn make_folder_entry(name: &str, account: &str) -> FolderEntry {
        FolderEntry {
            name: name.to_string(),
            account: account.to_string(),
        }
    }

    fn make_folder_section(name: &str, start: usize, count: usize) -> FolderSection {
        FolderSection {
            name: name.to_string(),
            start,
            count,
        }
    }

    // ---- EnvelopesLoaded ----

    #[test]
    fn envelopes_loaded_appends_new_account() {
        let (mut backends, tx, _rx) = test_fixtures();
        let mut app = App::new(vec![], "INBOX".to_string());
        app.pending_refreshes = 1;
        app.status = Some(Status::Working("Loading…".to_string()));

        let envelopes = vec![
            make_envelope_for("1", "Hello", "work"),
            make_envelope_for("2", "World", "work"),
        ];

        apply_backend_result(
            &mut app,
            BackendResult::EnvelopesLoaded {
                account: "work".to_string(),
                envelopes,
            },
            &mut backends,
            &tx,
        );

        assert_eq!(app.envelopes.len(), 2);
        assert_eq!(app.sections.len(), 1);
        assert_eq!(app.sections[0].name, "work");
        assert_eq!(app.sections[0].start, 0);
        assert_eq!(app.sections[0].count, 2);
        assert_eq!(app.pending_refreshes, 0);
        assert!(app.status.is_none());
    }

    #[test]
    fn envelopes_loaded_replaces_existing_account() {
        let (mut backends, tx, _rx) = test_fixtures();
        let mut app = App::new(
            vec![make_envelope_for("1", "Old", "work")],
            "INBOX".to_string(),
        );
        app.sections = vec![AccountSection {
            name: "work".to_string(),
            start: 0,
            count: 1,
        }];
        app.pending_refreshes = 1;

        let new_envelopes = vec![
            make_envelope_for("2", "New A", "work"),
            make_envelope_for("3", "New B", "work"),
        ];

        apply_backend_result(
            &mut app,
            BackendResult::EnvelopesLoaded {
                account: "work".to_string(),
                envelopes: new_envelopes,
            },
            &mut backends,
            &tx,
        );

        assert_eq!(app.envelopes.len(), 2);
        assert_eq!(app.envelopes[0].id, "2");
        assert_eq!(app.envelopes[1].id, "3");
        assert_eq!(app.sections[0].count, 2);
    }

    #[test]
    fn envelopes_loaded_multi_account_sorts_sections() {
        let (mut backends, tx, _rx) = test_fixtures();
        let mut app = App::new(vec![], "INBOX".to_string());
        app.pending_refreshes = 2;
        app.status = Some(Status::Working("Loading…".to_string()));

        apply_backend_result(
            &mut app,
            BackendResult::EnvelopesLoaded {
                account: "personal".to_string(),
                envelopes: vec![make_envelope_for("p1", "Personal", "personal")],
            },
            &mut backends,
            &tx,
        );
        assert_eq!(app.pending_refreshes, 1);
        assert!(app.status.is_some());

        apply_backend_result(
            &mut app,
            BackendResult::EnvelopesLoaded {
                account: "business".to_string(),
                envelopes: vec![make_envelope_for("b1", "Business", "business")],
            },
            &mut backends,
            &tx,
        );

        assert_eq!(app.sections.len(), 2);
        assert_eq!(app.sections[0].name, "business");
        assert_eq!(app.sections[1].name, "personal");
        assert_eq!(app.envelopes[0].id, "b1");
        assert_eq!(app.envelopes[1].id, "p1");
        assert_eq!(app.pending_refreshes, 0);
        assert!(app.status.is_none());
    }

    #[test]
    fn envelopes_loaded_clamps_selection() {
        let (mut backends, tx, _rx) = test_fixtures();
        let mut app = App::new(
            vec![
                make_envelope_for("1", "a", "work"),
                make_envelope_for("2", "b", "work"),
                make_envelope_for("3", "c", "work"),
            ],
            "INBOX".to_string(),
        );
        app.sections = vec![AccountSection {
            name: "work".to_string(),
            start: 0,
            count: 3,
        }];
        app.selected = 2;
        app.pending_refreshes = 1;

        apply_backend_result(
            &mut app,
            BackendResult::EnvelopesLoaded {
                account: "work".to_string(),
                envelopes: vec![make_envelope_for("4", "d", "work")],
            },
            &mut backends,
            &tx,
        );

        assert_eq!(app.envelopes.len(), 1);
        assert_eq!(app.selected, 0);
    }

    // ---- FoldersLoaded ----

    #[test]
    fn folders_loaded_transitions_to_folder_list() {
        let (mut backends, tx, _rx) = test_fixtures();
        let mut app = App::new(vec![], "INBOX".to_string());
        app.status = Some(Status::Working("Loading folders…".to_string()));

        let folders = vec![
            make_folder_entry("INBOX", "work"),
            make_folder_entry("Sent", "work"),
        ];
        let sections = vec![make_folder_section("work", 0, 2)];

        apply_backend_result(
            &mut app,
            BackendResult::FoldersLoaded { folders, sections },
            &mut backends,
            &tx,
        );

        if let View::FolderList(state) = &app.view {
            assert_eq!(state.folders.len(), 2);
            assert_eq!(state.sections.len(), 1);
            assert_eq!(state.selected, 0);
        } else {
            panic!("Expected FolderList view");
        }
        assert!(app.status.is_none());
    }

    // ---- FolderContext switching ----

    #[test]
    fn folder_context_switch_saves_and_restores() {
        let mut app = App::new(
            vec![make_envelope("1", "a"), make_envelope("2", "b")],
            "INBOX".to_string(),
        );
        app.sections = vec![AccountSection {
            name: "work".to_string(),
            start: 0,
            count: 2,
        }];
        app.selected = 1;

        // Save state (simulating OpenFolderList)
        app.saved_list_state = Some(SavedListState {
            folder_context: app.folder_context.clone(),
            envelopes: app.envelopes.clone(),
            sections: app.sections.clone(),
            selected: app.selected,
        });

        // Switch to SingleFolder
        app.envelopes = vec![make_envelope("3", "c")];
        app.sections = vec![AccountSection {
            name: "work".to_string(),
            start: 0,
            count: 1,
        }];
        app.selected = 0;
        app.folder_context = FolderContext::SingleFolder {
            folder_name: "Sent".to_string(),
            account_key: "work".to_string(),
        };

        assert_eq!(app.envelopes.len(), 1);
        assert_eq!(app.folder_display_name(), "Sent");

        // Restore (simulating BackFromFolders)
        let saved = app.saved_list_state.take().unwrap();
        app.envelopes = saved.envelopes;
        app.sections = saved.sections;
        app.selected = saved.selected;
        app.folder_context = saved.folder_context;

        assert_eq!(app.envelopes.len(), 2);
        assert_eq!(app.selected, 1);
        assert!(matches!(app.folder_context, FolderContext::AllInboxes));
    }

    // ---- MutationDone ----

    #[test]
    fn mutation_done_clears_status() {
        let (mut backends, tx, _rx) = test_fixtures();
        let mut app = App::new(vec![], "INBOX".to_string());
        app.status = Some(Status::Working("Deleting…".to_string()));

        apply_backend_result(&mut app, BackendResult::MutationDone, &mut backends, &tx);

        assert!(app.status.is_none());
    }
}
