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

pub async fn run(config_paths: &[PathBuf], all: bool, account: Option<String>) -> Result<()> {
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

    let (toml_account_config, account_config) = config
        .clone()
        .into_account_configs(account.as_deref(), |c: &Config, name| c.account(name).ok())?;

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

    let page_size = account_config.get_envelope_list_page_size();
    let opts = ListEnvelopesOptions {
        page: 0,
        page_size,
        query: None,
    };

    let envelopes = backend.list_envelopes(&folder, opts).await?;
    let mut envelope_data: Vec<EnvelopeData> = envelopes.iter().map(EnvelopeData::from).collect();
    for env in &mut envelope_data {
        env.account = account_name.clone();
    }
    let count = envelope_data.len();

    let _guard = TerminalGuard;
    let mut terminal = ratatui::init();

    let sections = vec![AccountSection {
        name: account_name.clone(),
        start: 0,
        count,
    }];
    let mut app = App::new(envelope_data).with_sections(sections);

    let mut backends = HashMap::new();
    backends.insert(
        account_name.clone(),
        AccountBackend {
            backend,
            account_config,
            source_folder: folder.clone(),
            archive_folder,
            toml_account_config,
        },
    );

    run_event_loop(&mut terminal, &mut app, &backends, &account_name).await
}

async fn run_all_accounts(config: TomlConfig) -> Result<()> {
    let mut account_names: Vec<String> = config.accounts.keys().cloned().collect();
    account_names.sort();

    let mut all_envelopes = Vec::new();
    let mut sections = Vec::new();
    let mut backends = HashMap::new();

    // Spawn all account loads concurrently for parallel I/O
    let mut handles = Vec::new();
    for name in &account_names {
        let config = config.clone();
        let name = name.clone();
        handles.push(tokio::spawn(async move {
            let result: Result<_, color_eyre::eyre::Error> = async {
                let (toml_account_config, account_config) = config
                    .into_account_configs(Some(name.as_str()), |c: &Config, n| c.account(n).ok())?;

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

                let acct_folder = account_config.get_inbox_folder_alias();
                let archive_folder = account_config.get_folder_alias("archive");
                let page_size = account_config.get_envelope_list_page_size();
                let opts = ListEnvelopesOptions {
                    page: 0,
                    page_size,
                    query: None,
                };

                let envelopes = backend.list_envelopes(&acct_folder, opts).await?;

                Ok((
                    backend,
                    account_config,
                    acct_folder,
                    archive_folder,
                    envelopes,
                    toml_account_config,
                ))
            }
            .await;

            // Pair the account name with the result so errors retain context
            (name, result)
        }));
    }

    // Await handles in order to preserve sorted account ordering
    for handle in handles {
        match handle.await {
            Ok((
                name,
                Ok((
                    backend,
                    account_config,
                    acct_folder,
                    archive_folder,
                    envelopes,
                    toml_account_config,
                )),
            )) => {
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

                backends.insert(
                    name,
                    AccountBackend {
                        backend,
                        account_config,
                        source_folder: acct_folder,
                        archive_folder,
                        toml_account_config,
                    },
                );
            }
            Ok((name, Err(e))) => {
                eprintln!("Error loading account {}: {}", name, e);
            }
            Err(e) => {
                eprintln!("Account loading task panicked: {}", e);
            }
        }
    }

    let _guard = TerminalGuard;
    let mut terminal = ratatui::init();

    let mut app = App::new(all_envelopes).with_sections(sections);

    // For multi-account, default_account is empty; we look up per-envelope
    let default_account = String::new();
    run_event_loop(&mut terminal, &mut app, &backends, &default_account).await
}

struct AccountBackend {
    backend: pimalaya_tui::himalaya::backend::Backend,
    account_config: Arc<email::account::config::AccountConfig>,
    source_folder: String,
    archive_folder: String,
    toml_account_config: Arc<crate::account::config::TomlAccountConfig>,
}

type BackendMap = HashMap<String, AccountBackend>;

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
/// main list or reading a message.
fn active_envelope_context(app: &App, default_account: &str) -> Option<EnvelopeContext> {
    app.envelopes.get(app.selected).map(|env| {
        let (account_key, folder) = match &app.folder_context {
            FolderContext::SingleFolder {
                account_key,
                folder_name,
            } => (account_key.clone(), folder_name.clone()),
            FolderContext::AllInboxes => (
                account_key_for(app, default_account),
                app.folder_display_name(),
            ),
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

/// Get a mutable reference to the active envelope in the current context.
fn active_envelope_mut(app: &mut App) -> Option<&mut EnvelopeData> {
    app.envelopes.get_mut(app.selected)
}

/// The kind of message operation to execute.
enum MessageOp {
    Delete {
        folder: String,
    },
    Archive {
        source_folder: String,
        target_folder: String,
    },
    Move {
        source_folder: String,
        target_folder: String,
    },
}

impl MessageOp {
    fn working_status(&self) -> String {
        match self {
            MessageOp::Delete { .. } => "Deleting\u{2026}".to_string(),
            MessageOp::Archive { .. } => "Archiving\u{2026}".to_string(),
            MessageOp::Move { target_folder, .. } => format!("Moving to {target_folder}\u{2026}"),
        }
    }

    fn success_status(&self) -> String {
        match self {
            MessageOp::Delete { .. } => "Deleted".to_string(),
            MessageOp::Archive { .. } => "Archived".to_string(),
            MessageOp::Move { target_folder, .. } => format!("Moved to {target_folder}"),
        }
    }

    fn error_prefix(&self) -> &'static str {
        match self {
            MessageOp::Delete { .. } => "Delete failed",
            MessageOp::Archive { .. } => "Archive failed",
            MessageOp::Move { .. } => "Move failed",
        }
    }
}

async fn execute_message_op(
    app: &mut App,
    terminal: &mut ratatui::DefaultTerminal,
    backends: &BackendMap,
    account_key: &str,
    id_str: &str,
    envelope_index: usize,
    op: MessageOp,
) -> Result<()> {
    app.status = Some(Status::Working(op.working_status()));
    terminal.draw(|frame| ui::render(frame, app))?;

    let mut error: Option<String> = None;
    if let Some(ab) = backends.get(account_key) {
        match id_str.parse::<usize>() {
            Ok(id) => {
                let result = match &op {
                    MessageOp::Delete { folder } => ab.backend.delete_messages(folder, &[id]).await,
                    MessageOp::Archive {
                        source_folder,
                        target_folder,
                    }
                    | MessageOp::Move {
                        source_folder,
                        target_folder,
                    } => {
                        ab.backend
                            .move_messages(source_folder, target_folder, &[id])
                            .await
                    }
                };
                match result {
                    Ok(_) => {
                        app.remove_envelope(envelope_index);
                        app.needs_refresh = true;
                        if !matches!(app.view, View::MessageList) {
                            app.view = View::MessageList;
                        }
                        app.status = Some(Status::Info(op.success_status()));
                    }
                    Err(e) => error = Some(format!("{}: {e}", op.error_prefix())),
                }
            }
            Err(e) => error = Some(format!("{}: {e}", op.error_prefix())),
        }
    }
    if let Some(err) = error {
        app.status = Some(Status::Error(err));
    }
    Ok(())
}

/// Re-fetch the envelope list from backends, context-aware.
async fn refresh_envelope_list(
    app: &mut App,
    backends: &BackendMap,
    terminal: &mut ratatui::DefaultTerminal,
) {
    app.status = Some(Status::Working("Refreshing…".to_string()));
    terminal.draw(|frame| ui::render(frame, app)).ok();

    match &app.folder_context {
        FolderContext::AllInboxes => {
            let mut all_envelopes = Vec::new();
            let mut sections = Vec::new();

            let mut keys: Vec<String> = backends.keys().cloned().collect();
            keys.sort();

            for key in &keys {
                if let Some(ab) = backends.get(key) {
                    let page_size = ab.account_config.get_envelope_list_page_size();
                    let opts = ListEnvelopesOptions {
                        page: 0,
                        page_size,
                        query: None,
                    };
                    match ab.backend.list_envelopes(&ab.source_folder, opts).await {
                        Ok(envelopes) => {
                            let start = all_envelopes.len();
                            let mut envelope_data: Vec<EnvelopeData> =
                                envelopes.iter().map(EnvelopeData::from).collect();
                            for env in &mut envelope_data {
                                env.account = key.clone();
                            }
                            let count = envelope_data.len();
                            all_envelopes.extend(envelope_data);
                            sections.push(AccountSection {
                                name: key.clone(),
                                start,
                                count,
                            });
                        }
                        Err(e) => {
                            app.status = Some(Status::Error(format!("Refresh failed: {e}")));
                            return;
                        }
                    }
                }
            }

            app.envelopes = all_envelopes;
            app.sections = sections;
        }
        FolderContext::SingleFolder {
            folder_name,
            account_key,
        } => {
            let folder_name = folder_name.clone();
            let account_key = account_key.clone();
            if let Some(ab) = backends.get(&account_key) {
                let page_size = ab.account_config.get_envelope_list_page_size();
                let opts = ListEnvelopesOptions {
                    page: 0,
                    page_size,
                    query: None,
                };
                match ab.backend.list_envelopes(&folder_name, opts).await {
                    Ok(envelopes) => {
                        let mut envelope_data: Vec<EnvelopeData> =
                            envelopes.iter().map(EnvelopeData::from).collect();
                        for env in &mut envelope_data {
                            env.account = account_key.clone();
                        }
                        app.sections = vec![AccountSection {
                            name: account_key.clone(),
                            start: 0,
                            count: envelope_data.len(),
                        }];
                        app.envelopes = envelope_data;
                    }
                    Err(e) => {
                        app.status = Some(Status::Error(format!("Refresh failed: {e}")));
                        return;
                    }
                }
            }
        }
    }

    if !app.envelopes.is_empty() {
        app.selected = app.selected.min(app.envelopes.len() - 1);
    } else {
        app.selected = 0;
    }
    app.status = None;
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
) -> Result<()> {
    // Determine account key and envelope context
    let (account_key, envelope_id, folder) = match kind {
        ComposeKind::New => {
            let mut key = if let Some(ovr) = account_override {
                ovr.to_string()
            } else {
                account_key_for(app, default_account)
            };
            // If key doesn't match a backend (e.g. empty list in multi-account),
            // fall back to the first available account.
            if !backends.contains_key(&key) {
                key = backends.keys().min().cloned().unwrap_or_default();
            }
            let folder = backends
                .get(&key)
                .map(|ab| ab.source_folder.clone())
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

    let Some(ab) = backends.get(&account_key) else {
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
    let compose_backend =
        match build_compose_backend(&ab.toml_account_config, &ab.account_config).await {
            Ok(b) => b,
            Err(e) => {
                app.status = Some(Status::Error(format!("Compose setup failed: {e}")));
                return Ok(());
            }
        };

    // Build the template
    let tpl = match kind {
        ComposeKind::New => {
            Message::new_tpl_builder(ab.account_config.clone())
                .build()
                .await
        }
        ComposeKind::Reply | ComposeKind::ReplyAll => {
            let id = envelope_id.ok_or_else(|| color_eyre::eyre::eyre!("Invalid envelope ID"))?;
            let msgs = ab.backend.get_messages(&folder, &[id]).await?;
            let msg = msgs
                .first()
                .ok_or_else(|| color_eyre::eyre::eyre!("Cannot find message {id}"))?;
            msg.to_reply_tpl_builder(ab.account_config.clone())
                .with_reply_all(matches!(kind, ComposeKind::ReplyAll))
                .build()
                .await
        }
        ComposeKind::Forward => {
            let id = envelope_id.ok_or_else(|| color_eyre::eyre::eyre!("Invalid envelope ID"))?;
            let msgs = ab.backend.get_messages(&folder, &[id]).await?;
            let msg = msgs
                .first()
                .ok_or_else(|| color_eyre::eyre::eyre!("Cannot find message {id}"))?;
            msg.to_forward_tpl_builder(ab.account_config.clone())
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

    if std::env::var("EDITOR").is_err() {
        // SAFETY: Called while the TUI event loop is paused (single
        // thread of control). The `unused_unsafe` allow keeps this
        // compiling on edition 2021 while being forward-compatible
        // with edition 2024 where `set_var` becomes unsafe.
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var("EDITOR", "vi");
        }
    }

    // Suspend TUI
    ratatui::restore();

    // Run editor flow
    let mut printer = StdoutPrinter::default();
    let editor_result = editor::edit_tpl_with_editor(
        ab.account_config.clone(),
        &mut printer,
        &compose_backend,
        tpl,
    )
    .await;

    // Restore TUI
    *terminal = ratatui::init();

    match editor_result {
        Ok(()) => {
            // For reply/reply-all, add Answered flag
            if matches!(kind, ComposeKind::Reply | ComposeKind::ReplyAll) {
                if let Some(id) = envelope_id {
                    let _ = ab.backend.add_flag(&folder, &[id], Flag::Answered).await;
                }
            }
            // Refresh envelope list
            refresh_envelope_list(app, backends, terminal).await;
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

    let Some(ab) = backends.get(&account_key) else {
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
    let tpl = match ab.backend.get_messages(&folder, &[id]).await {
        Ok(msgs) => match msgs.first() {
            Some(msg) => msg.to_read_tpl(&ab.account_config, |tpl| tpl).await,
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
    let compose_backend =
        match build_compose_backend(&ab.toml_account_config, &ab.account_config).await {
            Ok(b) => b,
            Err(e) => {
                app.status = Some(Status::Error(format!("Compose setup failed: {e}")));
                return Ok(());
            }
        };

    if std::env::var("EDITOR").is_err() {
        // SAFETY: Called while the TUI event loop is paused (single
        // thread of control). The `unused_unsafe` allow keeps this
        // compiling on edition 2021 while being forward-compatible
        // with edition 2024 where `set_var` becomes unsafe.
        #[allow(unused_unsafe)]
        unsafe {
            std::env::set_var("EDITOR", "vi");
        }
    }

    // Suspend TUI
    ratatui::restore();

    // Run editor flow
    let mut printer = StdoutPrinter::default();
    let editor_result = editor::edit_tpl_with_editor(
        ab.account_config.clone(),
        &mut printer,
        &compose_backend,
        tpl,
    )
    .await;

    // Restore TUI
    *terminal = ratatui::init();

    match editor_result {
        Ok(()) => {
            if is_draft {
                let _ = ab.backend.delete_messages(&folder, &[id]).await;
            }
            refresh_envelope_list(app, backends, terminal).await;
        }
        Err(e) => {
            app.status = Some(Status::Error(format!("Editor error: {e}")));
        }
    }

    Ok(())
}

async fn run_event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    backends: &BackendMap,
    default_account: &str,
) -> Result<()> {
    loop {
        terminal.draw(|frame| ui::render(frame, app))?;

        let mut action = handle_event(&app.view, &app.folder_context, app.search.is_some())?;

        // Clear transient status on any keypress
        if !matches!(action, Action::None) && app.status.is_some() {
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
                    // Advance selection for NextMessage, with "no more" feedback
                    if matches!(action, Action::NextMessage) {
                        if matches!(app.view, View::MessageRead { .. }) {
                            let prev = app.selected;
                            app.select_next();
                            if app.selected == prev {
                                app.status = Some(Status::Info("No more messages".to_string()));
                                break;
                            }
                        } else {
                            break;
                        }
                    }

                    if let Some(ctx) = active_envelope_context(app, default_account) {
                        app.status = Some(Status::Working("Loading…".to_string()));
                        terminal.draw(|frame| ui::render(frame, app))?;

                        let content = if let Some(ab) = backends.get(&ctx.account_key) {
                            match ctx.id.parse::<usize>() {
                                Ok(id) => match ab.backend.get_messages(&ctx.folder, &[id]).await {
                                    Ok(emails) => {
                                        let mut body = String::new();
                                        for email in emails.to_vec() {
                                            match email
                                                .to_read_tpl(&ab.account_config, |tpl| tpl)
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
                                },
                                Err(_) => format!("Invalid envelope ID: {}", ctx.id),
                            }
                        } else {
                            format!("No backend for account: {}", ctx.account_key)
                        };

                        // Mark as seen locally
                        if ctx.unseen {
                            if let Some(env) = active_envelope_mut(app) {
                                env.unseen = false;
                                if !env.flags.contains('S') {
                                    env.flags = sort_flags(&format!("S{}", env.flags));
                                }
                            }
                        }

                        // Transition to MessageRead
                        app.status = None;
                        app.view = View::MessageRead { content, scroll: 0 };
                        terminal.draw(|frame| ui::render(frame, app))?;

                        // Mark as read on server in background
                        if ctx.unseen {
                            if let Some(ab) = backends.get(&ctx.account_key) {
                                if let Ok(id) = ctx.id.parse::<usize>() {
                                    let seen = Flags::from_iter([Flag::Seen]);
                                    let _ = ab.backend.add_flags(&ctx.folder, &[id], &seen).await;
                                }
                            }
                        }
                    }
                }
                Action::BackToList => {
                    app.view = View::MessageList;
                    if app.needs_refresh {
                        app.needs_refresh = false;
                        refresh_envelope_list(app, backends, terminal).await;
                    }
                }
                Action::BackFromFolder => {
                    // Restore previous list state from saved_list_state if available
                    if let Some(saved) = app.saved_list_state.take() {
                        app.envelopes = saved.envelopes;
                        app.sections = saved.sections;
                        app.selected = saved.selected;
                        app.folder_context = saved.folder_context;
                    } else {
                        app.folder_context = FolderContext::AllInboxes;
                    }
                    app.view = View::MessageList;
                    app.search = None;
                    refresh_envelope_list(app, backends, terminal).await;
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
                        execute_message_op(
                            app,
                            terminal,
                            backends,
                            &ctx.account_key,
                            &ctx.id,
                            app.selected,
                            MessageOp::Delete { folder: ctx.folder },
                        )
                        .await?;
                    }
                }
                Action::ToggleRead => {
                    if let Some(ctx) = active_envelope_context(app, default_account) {
                        let label = if ctx.unseen {
                            "Marking read…"
                        } else {
                            "Marking unread…"
                        };
                        app.status = Some(Status::Working(label.to_string()));
                        terminal.draw(|frame| ui::render(frame, app))?;

                        let mut error: Option<String> = None;
                        if let Some(ab) = backends.get(&ctx.account_key) {
                            match ctx.id.parse::<usize>() {
                                Ok(id) => {
                                    let seen = Flags::from_iter([Flag::Seen]);
                                    let result = if ctx.unseen {
                                        ab.backend.add_flags(&ctx.folder, &[id], &seen).await
                                    } else {
                                        ab.backend.remove_flags(&ctx.folder, &[id], &seen).await
                                    };
                                    match result {
                                        Ok(_) => {
                                            if let Some(env) = active_envelope_mut(app) {
                                                if ctx.unseen {
                                                    env.unseen = false;
                                                    if !env.flags.contains('S') {
                                                        env.flags =
                                                            sort_flags(&format!("S{}", env.flags));
                                                    }
                                                } else {
                                                    env.unseen = true;
                                                    env.flags = env.flags.replace('S', "");
                                                }
                                            }
                                            // If in MessageRead, go back to list
                                            if !matches!(app.view, View::MessageList) {
                                                app.view = View::MessageList;
                                            }
                                            let msg = if ctx.unseen {
                                                "Marked read"
                                            } else {
                                                "Marked unread"
                                            };
                                            app.status = Some(Status::Info(msg.to_string()));
                                        }
                                        Err(e) => error = Some(format!("Toggle read failed: {e}")),
                                    }
                                }
                                Err(e) => error = Some(format!("Toggle read failed: {e}")),
                            }
                        }
                        if let Some(err) = error {
                            app.status = Some(Status::Error(err));
                        }
                    }
                }
                Action::ToggleFlag => {
                    if let Some(ctx) = active_envelope_context(app, default_account) {
                        let label = if ctx.flagged {
                            "Unflagging…"
                        } else {
                            "Flagging…"
                        };
                        app.status = Some(Status::Working(label.to_string()));
                        terminal.draw(|frame| ui::render(frame, app))?;

                        let mut error: Option<String> = None;
                        if let Some(ab) = backends.get(&ctx.account_key) {
                            match ctx.id.parse::<usize>() {
                                Ok(id) => {
                                    let flagged = Flags::from_iter([Flag::Flagged]);
                                    let result = if ctx.flagged {
                                        ab.backend.remove_flags(&ctx.folder, &[id], &flagged).await
                                    } else {
                                        ab.backend.add_flags(&ctx.folder, &[id], &flagged).await
                                    };
                                    match result {
                                        Ok(_) => {
                                            if let Some(env) = active_envelope_mut(app) {
                                                env.flagged = !ctx.flagged;
                                                if ctx.flagged {
                                                    env.flags = env.flags.replace('F', "");
                                                } else if !env.flags.contains('F') {
                                                    env.flags =
                                                        sort_flags(&format!("F{}", env.flags));
                                                }
                                            }
                                            if !matches!(app.view, View::MessageList) {
                                                app.view = View::MessageList;
                                            }
                                            let msg =
                                                if ctx.flagged { "Unflagged" } else { "Flagged" };
                                            app.status = Some(Status::Info(msg.to_string()));
                                        }
                                        Err(e) => error = Some(format!("Flag toggle failed: {e}")),
                                    }
                                }
                                Err(e) => error = Some(format!("Flag toggle failed: {e}")),
                            }
                        }
                        if let Some(err) = error {
                            app.status = Some(Status::Error(err));
                        }
                    }
                }
                Action::OpenFolderList => {
                    app.status = Some(Status::Working("Loading folders…".to_string()));
                    terminal.draw(|frame| ui::render(frame, app))?;

                    // Save current state before transitioning
                    app.saved_list_state = Some(SavedListState {
                        folder_context: app.folder_context.clone(),
                        envelopes: app.envelopes.clone(),
                        sections: app.sections.clone(),
                        selected: app.selected,
                    });
                    app.search = None;

                    let mut folders = Vec::new();
                    let mut sections = Vec::new();
                    let mut error: Option<String> = None;

                    let mut keys: Vec<String> = backends.keys().cloned().collect();
                    keys.sort();
                    for key in &keys {
                        if let Some(ab) = backends.get(key) {
                            match ab.backend.list_folders().await {
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
                                    error = Some(format!("Error loading folders for {key}: {e}"));
                                    break;
                                }
                            }
                        }
                    }

                    if let Some(err) = error {
                        app.status = Some(Status::Error(err));
                    } else {
                        app.status = None;
                        app.view = View::FolderList(FolderListState {
                            folders,
                            sections,
                            selected: 0,
                        });
                    }
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
                Action::BackFromFolderPicker => {
                    // Restore saved state
                    if let Some(saved) = app.saved_list_state.take() {
                        app.envelopes = saved.envelopes;
                        app.sections = saved.sections;
                        app.selected = saved.selected;
                        app.folder_context = saved.folder_context;
                    }
                    app.view = View::MessageList;
                    app.search = None;
                    refresh_envelope_list(app, backends, terminal).await;
                }
                Action::CancelMove => {
                    app.view = View::MessageList;
                    app.search = None;
                }
                Action::SelectFolder => {
                    // Extract folder info from the FolderList state
                    let folder_info = if let View::FolderList(state) = &app.view {
                        state
                            .folders
                            .get(state.selected)
                            .map(|f| (f.name.clone(), f.account.clone()))
                    } else {
                        None
                    };

                    if let Some((folder_name, account_key)) = folder_info {
                        app.status = Some(Status::Working(format!("Loading {folder_name}…")));
                        terminal.draw(|frame| ui::render(frame, app))?;

                        // Resolve the backend key
                        let key = if !account_key.is_empty() {
                            account_key.clone()
                        } else if backends.contains_key(default_account) {
                            default_account.to_string()
                        } else {
                            backends.keys().next().cloned().unwrap_or_default()
                        };

                        let mut error: Option<String> = None;
                        let mut envelope_data = Vec::new();

                        if let Some(ab) = backends.get(&key) {
                            let page_size = ab.account_config.get_envelope_list_page_size();
                            let opts = ListEnvelopesOptions {
                                page: 0,
                                page_size,
                                query: None,
                            };
                            match ab.backend.list_envelopes(&folder_name, opts).await {
                                Ok(envelopes) => {
                                    envelope_data =
                                        envelopes.iter().map(EnvelopeData::from).collect();
                                    for env in &mut envelope_data {
                                        env.account = key.clone();
                                    }
                                }
                                Err(e) => {
                                    error = Some(format!("Error loading envelopes: {e}"));
                                }
                            }
                        }

                        if let Some(err) = error {
                            app.status = Some(Status::Error(err));
                        } else {
                            app.status = None;
                            app.folder_context = FolderContext::SingleFolder {
                                folder_name,
                                account_key: key.clone(),
                            };
                            app.sections = vec![AccountSection {
                                name: key,
                                start: 0,
                                count: envelope_data.len(),
                            }];
                            app.envelopes = envelope_data;
                            app.selected = 0;
                            app.view = View::MessageList;
                            app.search = None;
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
                    // Clear search if no follow-up (e.g. empty results already cleared)
                    app.cancel_search();
                }
                Action::SearchCancel => {
                    app.cancel_search();
                }
                Action::ArchiveMessage => {
                    if let Some(ctx) = active_envelope_context(app, default_account) {
                        if let Some(ab) = backends.get(&ctx.account_key) {
                            let archive_folder = ab.archive_folder.clone();
                            execute_message_op(
                                app,
                                terminal,
                                backends,
                                &ctx.account_key,
                                &ctx.id,
                                app.selected,
                                MessageOp::Archive {
                                    source_folder: ctx.folder,
                                    target_folder: archive_folder,
                                },
                            )
                            .await?;
                        }
                    }
                }
                Action::MoveMessage => {
                    if let Some(ctx) = active_envelope_context(app, default_account) {
                        let (id_str, account_key, source_folder) =
                            (ctx.id.clone(), ctx.account_key.clone(), ctx.folder.clone());
                        let env_index = app.selected;

                        app.status = Some(Status::Working("Loading folders…".to_string()));
                        terminal.draw(|frame| ui::render(frame, app))?;

                        let mut folders = Vec::new();
                        let mut error: Option<String> = None;

                        if let Some(ab) = backends.get(&account_key) {
                            match ab.backend.list_folders().await {
                                Ok(account_folders) => {
                                    let account_folders: Vec<email::folder::Folder> =
                                        account_folders.into();
                                    for f in account_folders {
                                        if f.name != source_folder {
                                            folders.push(FolderEntry {
                                                name: f.name,
                                                account: account_key.clone(),
                                            });
                                        }
                                    }
                                }
                                Err(e) => {
                                    error = Some(format!("Error loading folders: {e}"));
                                }
                            }
                        }

                        if let Some(err) = error {
                            app.status = Some(Status::Error(err));
                        } else if folders.is_empty() {
                            app.status =
                                Some(Status::Error("No other folders available".to_string()));
                        } else {
                            app.status = None;
                            // If in MessageRead, go back to list first
                            if matches!(app.view, View::MessageRead { .. }) {
                                app.view = View::MessageList;
                            }

                            app.view = View::MoveFolderPicker(MoveFolderPickerState {
                                folders,
                                selected: 0,
                                source_envelope_id: id_str,
                                source_envelope_index: env_index,
                                source_folder,
                                account_key,
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
                            // Look up by ID in case the list changed since the picker opened.
                            let envelope_index = app
                                .envelopes
                                .iter()
                                .position(|e| e.id == id_str)
                                .unwrap_or(state.source_envelope_index);
                            execute_message_op(
                                app,
                                terminal,
                                backends,
                                &account_key,
                                &id_str,
                                envelope_index,
                                MessageOp::Move {
                                    source_folder,
                                    target_folder: target_name,
                                },
                            )
                            .await?;
                        }
                    }
                }
                Action::EditMessage => {
                    handle_edit_message(app, terminal, backends, default_account)
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
