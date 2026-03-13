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
    sort_flags, AccountPickerState, AccountSection, App, EnvelopeData, FolderEntry,
    FolderEnvelopeState, FolderListState, FolderSection, MoveFolderPickerState, Status, View,
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
        saved_envelope_selected: usize,
    },
    FolderEnvelopesLoaded {
        folder_name: String,
        account_key: String,
        envelopes: Vec<EnvelopeData>,
        parent: FolderListState,
    },
    MoveFoldersLoaded {
        folders: Vec<FolderEntry>,
        source_envelope_id: String,
        source_envelope_index: usize,
        source_folder: String,
        account_key: String,
        return_to_folder: bool,
        folder_envelope_state: Option<Box<FolderEnvelopeState>>,
    },

    // Fire-and-forget acknowledgement — clears the "Working…" status
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

/// Extract info about the active envelope, regardless of whether we're in the
/// main list, a folder envelope list, or reading a message.
fn active_envelope_context(app: &App, default_account: &str) -> Option<EnvelopeContext> {
    match &app.view {
        View::FolderEnvelopeList(state) => {
            state
                .envelopes
                .get(state.selected)
                .map(|env| EnvelopeContext {
                    id: env.id.clone(),
                    unseen: env.unseen,
                    flagged: env.flagged,
                    is_draft: env.flags.contains('T'),
                    account_key: state.account_key.clone(),
                    folder: state.folder_name.clone(),
                })
        }
        View::MessageRead {
            folder_context: Some(ctx),
            ..
        } => ctx.envelopes.get(ctx.selected).map(|env| EnvelopeContext {
            id: env.id.clone(),
            unseen: env.unseen,
            flagged: env.flagged,
            is_draft: env.flags.contains('T'),
            account_key: ctx.account_key.clone(),
            folder: ctx.folder_name.clone(),
        }),
        _ => app.envelopes.get(app.selected).map(|env| {
            let account_key = account_key_for(app, default_account);
            EnvelopeContext {
                id: env.id.clone(),
                unseen: env.unseen,
                flagged: env.flagged,
                is_draft: env.flags.contains('T'),
                account_key,
                folder: app.folder.clone(),
            }
        }),
    }
}

/// Get a mutable reference to the active envelope in the current context.
fn active_envelope_mut(app: &mut App) -> Option<&mut EnvelopeData> {
    match &mut app.view {
        View::FolderEnvelopeList(state) => state.envelopes.get_mut(state.selected),
        View::MessageRead {
            folder_context: Some(ctx),
            ..
        } => ctx.envelopes.get_mut(ctx.selected),
        _ => app.envelopes.get_mut(app.selected),
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

            if app.folder == "INBOX" && !folder.is_empty() {
                app.folder = folder.clone();
            }

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
            // Replace envelopes for this account if a section already exists,
            // otherwise append. This avoids clearing the entire list on refresh.
            if let Some(si) = app.sections.iter().position(|s| s.name == account) {
                let section = &app.sections[si];
                let old_start = section.start;
                let old_count = section.count;
                let new_count = envelopes.len();

                // Remove old envelopes for this account
                app.envelopes
                    .splice(old_start..old_start + old_count, envelopes);

                // Update this section's count
                app.sections[si].count = new_count;

                // Adjust start offsets for sections that come after this one
                let diff = new_count as isize - old_count as isize;
                for s in &mut app.sections {
                    if s.start > old_start {
                        s.start = (s.start as isize + diff) as usize;
                    }
                }
            } else {
                let count = envelopes.len();
                let start = app.envelopes.len();
                app.envelopes.extend(envelopes);
                app.sections.push(AccountSection {
                    name: account,
                    start,
                    count,
                });
                // Re-sort sections by account name for consistent ordering
                resort_sections(app);
            }

            // Clamp selection
            if !app.envelopes.is_empty() {
                app.selected = app.selected.min(app.envelopes.len() - 1);
            } else {
                app.selected = 0;
            }

            // Clear loading status when all accounts have loaded
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
            // Cache the message content for instant re-display
            app.cache
                .messages
                .insert(envelope_id.clone(), content.clone());

            // View already transitioned optimistically — just fill in the content
            if let View::MessageRead {
                content: view_content,
                ..
            } = &mut app.view
            {
                *view_content = content;
            }
            app.status = None;

            // Store context for background seen-marking
            app.last_read_context = if was_unseen {
                Some((account_key, folder, envelope_id))
            } else {
                None
            };
        }
        BackendResult::FoldersLoaded {
            folders,
            sections,
            saved_envelope_selected,
        } => {
            // Cache the folder list for instant display next time
            app.cache.folders = Some((folders.clone(), sections.clone()));

            // Update the view if the user is still waiting for folders (EnvelopeList
            // on first load) or already viewing them (FolderList on background refresh)
            if matches!(app.view, View::FolderList(_) | View::EnvelopeList) {
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
                    saved_envelope_selected,
                });
            }
        }
        BackendResult::FolderEnvelopesLoaded {
            folder_name,
            account_key,
            envelopes,
            parent,
        } => {
            // Cache folder envelopes for instant display next time
            app.cache.folder_envelopes.insert(
                (account_key.clone(), folder_name.clone()),
                envelopes.clone(),
            );

            // Update the view if the user is still waiting for this folder (FolderList
            // on first load, EnvelopeList on legacy path) or already viewing it
            // (FolderEnvelopeList on background refresh / cached view)
            let should_update = matches!(app.view, View::EnvelopeList | View::FolderList(_))
                || matches!(
                    &app.view,
                    View::FolderEnvelopeList(state)
                        if state.account_key == account_key && state.folder_name == folder_name
                );
            if should_update {
                app.status = None;
                let selected = if let View::FolderEnvelopeList(state) = &app.view {
                    state.selected
                } else {
                    0
                };
                app.view = View::FolderEnvelopeList(FolderEnvelopeState {
                    envelopes,
                    selected,
                    folder_name,
                    account_key,
                    parent,
                });
            }
        }
        BackendResult::MoveFoldersLoaded {
            folders,
            source_envelope_id,
            source_envelope_index,
            source_folder,
            account_key,
            return_to_folder,
            folder_envelope_state,
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
                    return_to_folder,
                    folder_envelope_state,
                });
            }
        }
        BackendResult::MutationDone => {
            // Clear the "Working…" status unless another operation set it
            if matches!(app.status, Some(Status::Working(_))) {
                app.status = None;
            }
        }
        BackendResult::Error {
            message,
            needs_refresh,
        } => {
            app.status = Some(Status::Error(message));
            app.pending_refreshes = app.pending_refreshes.saturating_sub(1);
            if needs_refresh {
                refresh_folder_view_if_needed(app, backends, tx);
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
    // Pair each section with its envelope slice, sort by name, rebuild
    let mut groups: Vec<(String, Vec<EnvelopeData>)> = Vec::new();
    // Drain sections in reverse order so earlier indices stay valid
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
    // Determine account key and envelope context
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

    // Build a send-capable backend
    let compose_backend = match build_compose_backend(toml_account_config, account_config).await {
        Ok(b) => b,
        Err(e) => {
            app.status = Some(Status::Error(format!("Compose setup failed: {e}")));
            return Ok(());
        }
    };

    // Build the template
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

    // Suspend TUI
    ratatui::restore();

    // Run editor flow
    let mut printer = StdoutPrinter::default();
    let editor_result =
        editor::edit_tpl_with_editor(account_config.clone(), &mut printer, &compose_backend, tpl)
            .await;

    // Restore TUI
    *terminal = ratatui::init();

    match editor_result {
        Ok(()) => {
            // For reply/reply-all, add Answered flag in background
            if matches!(kind, ComposeKind::Reply | ComposeKind::ReplyAll) {
                if let Some(id) = envelope_id {
                    let backend = backend.clone();
                    let folder = folder.clone();
                    tokio::spawn(async move {
                        let _ = backend.add_flag(&folder, &[id], Flag::Answered).await;
                    });
                }
            }
            // Refresh envelope list (async) and folder view if in one
            app.status = Some(Status::Working("Refreshing…".to_string()));
            refresh_folder_view_if_needed(app, backends, tx);
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

    // Fetch message and build editable template
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

    // Build a send-capable backend
    let compose_backend = match build_compose_backend(toml_account_config, account_config).await {
        Ok(b) => b,
        Err(e) => {
            app.status = Some(Status::Error(format!("Compose setup failed: {e}")));
            return Ok(());
        }
    };

    // Suspend TUI
    ratatui::restore();

    // Run editor flow
    let mut printer = StdoutPrinter::default();
    let editor_result =
        editor::edit_tpl_with_editor(account_config.clone(), &mut printer, &compose_backend, tpl)
            .await;

    // Restore TUI
    *terminal = ratatui::init();

    match editor_result {
        Ok(()) => {
            if is_draft {
                // Optimistically remove the draft from the folder view
                if let View::FolderEnvelopeList(state) = &mut app.view {
                    if let Some(pos) = state.envelopes.iter().position(|e| e.id == id_str) {
                        state.remove_envelope(pos);
                    }
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
            refresh_folder_view_if_needed(app, backends, tx);
            start_full_refresh(app, backends, tx);
        }
        Err(e) => {
            app.status = Some(Status::Error(format!("Editor error: {e}")));
        }
    }

    Ok(())
}

/// Start a full refresh: spawn envelope loads for all accounts.
fn start_full_refresh(
    app: &mut App,
    backends: &BackendMap,
    tx: &mpsc::UnboundedSender<BackendResult>,
) {
    app.pending_refreshes = backends.len();
    app.envelopes_stale = false;
    spawn_refresh_envelope_list(backends, tx);
}

/// Sync the folder envelope cache from the current FolderEnvelopeList view state.
/// Call after any optimistic mutation that modifies folder-specific envelopes.
fn sync_folder_envelope_cache(app: &mut App) {
    if let View::FolderEnvelopeList(state) = &app.view {
        app.cache.folder_envelopes.insert(
            (state.account_key.clone(), state.folder_name.clone()),
            state.envelopes.clone(),
        );
    }
}

/// If the current view is a folder-specific view (FolderEnvelopeList or MessageRead
/// with folder_context), spawn a background re-fetch of that folder's envelopes so
/// the view reflects any changes (e.g. draft deleted after send/discard).
fn refresh_folder_view_if_needed(
    app: &App,
    backends: &BackendMap,
    tx: &mpsc::UnboundedSender<BackendResult>,
) {
    let (folder_name, account_key, parent) = match &app.view {
        View::FolderEnvelopeList(state) => (
            state.folder_name.clone(),
            state.account_key.clone(),
            state.parent.clone(),
        ),
        View::MessageRead {
            folder_context: Some(ctx),
            ..
        } => (
            ctx.folder_name.clone(),
            ctx.account_key.clone(),
            ctx.parent.clone(),
        ),
        _ => return,
    };

    let Some((backend, account_config, _, _, _)) = backends.get(&account_key) else {
        return;
    };

    let tx = tx.clone();
    let backend = backend.clone();
    let account_config = account_config.clone();
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
                    parent,
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

            // If we got a message loaded with a seen context, fire the server mark
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

        // 2. Check for user input (100ms poll — same as today)
        let mut action = handle_event(&app.view, app.search.is_some())?;

        // Clear transient status on any keypress
        if !matches!(action, Action::None) && app.status.is_some() {
            // Don't clear "Loading…" status during pending refreshes
            if app.pending_refreshes == 0 || !matches!(app.status, Some(Status::Working(_))) {
                app.status = None;
            }
        }

        // This loop allows SearchConfirm to set a follow-up action and re-enter.
        let mut clear_search_after = false;
        loop {
            let in_folder_context = matches!(
                app.view,
                View::FolderEnvelopeList(_)
                    | View::MessageRead {
                        folder_context: Some(_),
                        ..
                    }
            );

            match action {
                Action::None => {}
                Action::Quit => {
                    app.should_quit = true;
                }
                Action::SelectNext => {
                    if app.search.is_some() {
                        app.search_select_next();
                    } else if let View::FolderEnvelopeList(state) = &mut app.view {
                        if !state.envelopes.is_empty() {
                            state.selected = (state.selected + 1).min(state.envelopes.len() - 1);
                        }
                    } else {
                        app.select_next();
                    }
                }
                Action::SelectPrev => {
                    if app.search.is_some() {
                        app.search_select_prev();
                    } else if let View::FolderEnvelopeList(state) = &mut app.view {
                        state.selected = state.selected.saturating_sub(1);
                    } else {
                        app.select_prev();
                    }
                }
                Action::ReadMessage | Action::NextMessage => {
                    // Advance selection for NextMessage, with "no more" feedback
                    if matches!(action, Action::NextMessage) {
                        let advanced = match &mut app.view {
                            View::MessageRead {
                                folder_context: Some(ctx),
                                ..
                            } if !ctx.envelopes.is_empty() => {
                                let prev = ctx.selected;
                                ctx.selected = (ctx.selected + 1).min(ctx.envelopes.len() - 1);
                                ctx.selected != prev
                            }
                            View::MessageRead {
                                folder_context: None,
                                ..
                            } => {
                                let prev = app.selected;
                                app.select_next();
                                app.selected != prev
                            }
                            _ => false,
                        };
                        if !advanced {
                            app.status = Some(Status::Working("No more messages".to_string()));
                            continue;
                        }
                    }

                    if let Some(ctx) = active_envelope_context(app, default_account) {
                        app.status = Some(Status::Working("Loading…".to_string()));

                        // Optimistic: mark as seen locally and transition to
                        // MessageRead immediately with placeholder content
                        if ctx.unseen {
                            if let Some(env) = active_envelope_mut(app) {
                                env.unseen = false;
                                if !env.flags.contains('S') {
                                    env.flags = sort_flags(&format!("S{}", env.flags));
                                }
                            }
                        }

                        // Use cached message content if available
                        let cached_content =
                            app.cache.messages.get(&ctx.id).cloned().unwrap_or_default();

                        if in_folder_context {
                            let old_view = std::mem::replace(&mut app.view, View::EnvelopeList);
                            let folder_state = match old_view {
                                View::FolderEnvelopeList(state) => state,
                                View::MessageRead {
                                    folder_context: Some(ctx),
                                    ..
                                } => *ctx,
                                other => {
                                    app.view = other;
                                    break;
                                }
                            };
                            app.view = View::MessageRead {
                                content: cached_content,
                                scroll: 0,
                                folder_context: Some(Box::new(folder_state)),
                            };
                        } else {
                            app.view = View::MessageRead {
                                content: cached_content,
                                scroll: 0,
                                folder_context: None,
                            };
                        }

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
                    let old_view = std::mem::replace(&mut app.view, View::EnvelopeList);
                    if let View::MessageRead {
                        folder_context: Some(ctx),
                        ..
                    } = old_view
                    {
                        app.view = View::FolderEnvelopeList(*ctx);
                    } else if app.envelopes_stale {
                        // A mutation happened — refresh to sync with the server
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
                        if in_folder_context {
                            let old_view = std::mem::replace(&mut app.view, View::EnvelopeList);
                            let mut state = match old_view {
                                View::FolderEnvelopeList(s) => s,
                                View::MessageRead {
                                    folder_context: Some(ctx),
                                    ..
                                } => *ctx,
                                other => {
                                    app.view = other;
                                    app.status = None;
                                    break;
                                }
                            };
                            state.remove_envelope(state.selected);
                            app.view = View::FolderEnvelopeList(state);
                            sync_folder_envelope_cache(app);
                        } else {
                            app.remove_envelope(app.selected);
                            if !matches!(app.view, View::EnvelopeList) {
                                app.view = View::EnvelopeList;
                            }
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
                        // Optimistic local update
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
                        // Return to the list view (main or folder-specific)
                        if in_folder_context {
                            if let View::MessageRead {
                                folder_context: Some(_),
                                ..
                            } = &app.view
                            {
                                let old = std::mem::replace(&mut app.view, View::EnvelopeList);
                                if let View::MessageRead {
                                    folder_context: Some(ctx),
                                    ..
                                } = old
                                {
                                    app.view = View::FolderEnvelopeList(*ctx);
                                }
                            }
                            sync_folder_envelope_cache(app);
                        } else {
                            if !matches!(app.view, View::EnvelopeList) {
                                app.view = View::EnvelopeList;
                            }
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
                        // Optimistic local update
                        if let Some(env) = active_envelope_mut(app) {
                            env.flagged = !ctx.flagged;
                            if ctx.flagged {
                                env.flags = env.flags.replace('F', "");
                            } else if !env.flags.contains('F') {
                                env.flags = sort_flags(&format!("F{}", env.flags));
                            }
                        }
                        // Return to the list view (main or folder-specific)
                        if in_folder_context {
                            if let View::MessageRead {
                                folder_context: Some(_),
                                ..
                            } = &app.view
                            {
                                let old = std::mem::replace(&mut app.view, View::EnvelopeList);
                                if let View::MessageRead {
                                    folder_context: Some(ctx),
                                    ..
                                } = old
                                {
                                    app.view = View::FolderEnvelopeList(*ctx);
                                }
                            }
                            sync_folder_envelope_cache(app);
                        } else {
                            if !matches!(app.view, View::EnvelopeList) {
                                app.view = View::EnvelopeList;
                            }
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
                    let saved_envelope_selected = app.selected;

                    // Show cached folders immediately if available
                    if let Some((folders, sections)) = &app.cache.folders {
                        app.view = View::FolderList(FolderListState {
                            folders: folders.clone(),
                            sections: sections.clone(),
                            selected: 0,
                            saved_envelope_selected,
                        });
                        app.status = Some(Status::Working("Refreshing folders…".to_string()));
                    } else {
                        app.status = Some(Status::Working("Loading folders…".to_string()));
                    }
                    let tx = tx.clone();

                    // Spawn folder loading for all backends
                    let mut keys: Vec<String> = backends.keys().cloned().collect();
                    keys.sort();

                    // We need to collect all folders from all backends, so we spawn
                    // a single task that iterates them sequentially to maintain ordering.
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

                        let _ = tx.send(BackendResult::FoldersLoaded {
                            folders,
                            sections,
                            saved_envelope_selected,
                        });
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
                    if let View::FolderList(state) = &app.view {
                        app.selected = state.saved_envelope_selected;
                    }
                    app.view = View::EnvelopeList;
                    if app.envelopes_stale {
                        app.status = Some(Status::Working("Refreshing…".to_string()));
                        start_full_refresh(app, backends, &tx);
                    }
                }
                Action::CancelMove => {
                    let old_view = std::mem::replace(&mut app.view, View::EnvelopeList);
                    if let View::MoveFolderPicker(picker) = old_view {
                        if let Some(fe_state) = picker.folder_envelope_state {
                            app.view = View::FolderEnvelopeList(*fe_state);
                        } else if app.envelopes_stale {
                            app.status = Some(Status::Working("Refreshing…".to_string()));
                            start_full_refresh(app, backends, &tx);
                        }
                    }
                }
                Action::BackFromFolderEnvelopes => {
                    let old_view = std::mem::replace(&mut app.view, View::EnvelopeList);
                    if let View::FolderEnvelopeList(state) = old_view {
                        app.view = View::FolderList(state.parent);
                    }
                }
                Action::SelectFolder => {
                    // Extract folder info and take the FolderList state
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
                            // Clone the FolderListState to use as parent (don't
                            // change the view yet — keep folder list visible during load)
                            let parent = if let View::FolderList(state) = &app.view {
                                state.clone()
                            } else {
                                unreachable!()
                            };

                            // Show cached folder envelopes immediately if available
                            if let Some(cached) = app
                                .cache
                                .folder_envelopes
                                .get(&(key.clone(), folder_name.clone()))
                            {
                                app.view = View::FolderEnvelopeList(FolderEnvelopeState {
                                    envelopes: cached.clone(),
                                    selected: 0,
                                    folder_name: folder_name.clone(),
                                    account_key: key.clone(),
                                    parent: parent.clone(),
                                });
                                app.status =
                                    Some(Status::Working(format!("Refreshing {folder_name}…")));
                            } else {
                                app.status =
                                    Some(Status::Working(format!("Loading {folder_name}…")));
                            }

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
                                            parent,
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
                        View::EnvelopeList | View::FolderEnvelopeList(_) => {
                            Some(Action::ReadMessage)
                        }
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
                        if in_folder_context {
                            let old_view = std::mem::replace(&mut app.view, View::EnvelopeList);
                            let mut state = match old_view {
                                View::FolderEnvelopeList(s) => s,
                                View::MessageRead {
                                    folder_context: Some(ctx),
                                    ..
                                } => *ctx,
                                other => {
                                    app.view = other;
                                    app.status = None;
                                    break;
                                }
                            };
                            state.remove_envelope(state.selected);
                            app.view = View::FolderEnvelopeList(state);
                            sync_folder_envelope_cache(app);
                        } else {
                            app.remove_envelope(app.selected);
                            if !matches!(app.view, View::EnvelopeList) {
                                app.view = View::EnvelopeList;
                            }
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
                            {
                                match &app.view {
                                    View::FolderEnvelopeList(state) => state.selected,
                                    View::MessageRead {
                                        folder_context: Some(fctx),
                                        ..
                                    } => fctx.selected,
                                    _ => app.selected,
                                }
                            },
                        );

                        app.status = Some(Status::Working("Loading folders…".to_string()));

                        // Clone FolderEnvelopeState if in folder context (don't
                        // change the view yet — MoveFoldersLoaded will replace it)
                        let fe_state = if in_folder_context {
                            match &app.view {
                                View::FolderEnvelopeList(s) => Some(Box::new(s.clone())),
                                View::MessageRead {
                                    folder_context: Some(ctx),
                                    ..
                                } => Some(ctx.clone()),
                                _ => None,
                            }
                        } else {
                            if matches!(app.view, View::MessageRead { .. }) {
                                app.view = View::EnvelopeList;
                            }
                            None
                        };

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
                                            return_to_folder: in_folder_context,
                                            folder_envelope_state: fe_state,
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
                            let return_to_folder = state.return_to_folder;
                            let source_envelope_index = state.source_envelope_index;

                            // Optimistic local update
                            let old_view = std::mem::replace(&mut app.view, View::EnvelopeList);
                            if let View::MoveFolderPicker(picker) = old_view {
                                if return_to_folder {
                                    if let Some(mut fe_state) = picker.folder_envelope_state {
                                        fe_state.remove_envelope(source_envelope_index);
                                        app.view = View::FolderEnvelopeList(*fe_state);
                                        sync_folder_envelope_cache(app);
                                    }
                                } else {
                                    app.remove_envelope(source_envelope_index);
                                    // already set to EnvelopeList
                                }
                            }
                            app.status = Some(Status::Working(format!("Moved to {target_name}")));
                            app.envelopes_stale = true;

                            if let Some((backend, _, _, _, _)) = backends.get(&account_key) {
                                if let Ok(id) = id_str.parse::<usize>() {
                                    let tx = tx.clone();
                                    let backend = backend.clone();
                                    let target_name_clone = target_name.clone();
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
                        let previous_view = std::mem::replace(&mut app.view, View::EnvelopeList);
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
                        std::mem::replace(&mut app.view, View::EnvelopeList)
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
                        std::mem::replace(&mut app.view, View::EnvelopeList)
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
