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
};
use pimalaya_tui::{himalaya::backend::BackendBuilder, terminal::config::TomlConfig as _};

use crate::config::TomlConfig;

use self::app::{
    sort_flags, AccountSection, App, EnvelopeData, FolderContext, FolderEntry, FolderListState,
    FolderSection, SavedListState, Status, View,
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

    if backends.is_empty() {
        color_eyre::eyre::bail!("no accounts loaded successfully; cannot start TUI");
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
    #[allow(dead_code)]
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
            account_key,
            folder,
        }
    })
}

/// Get a mutable reference to the active envelope in the current context.
fn active_envelope_mut(app: &mut App) -> Option<&mut EnvelopeData> {
    app.envelopes.get_mut(app.selected)
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

async fn fetch_folder_list(
    backends: &BackendMap,
    account_filter: Option<&str>,
) -> Result<(Vec<FolderEntry>, Vec<FolderSection>), String> {
    let mut folders = Vec::new();
    let mut sections = Vec::new();

    let keys: Vec<String> = match account_filter {
        Some(key) => vec![key.to_string()],
        None => {
            let mut keys: Vec<String> = backends.keys().cloned().collect();
            keys.sort();
            keys
        }
    };

    for key in &keys {
        if let Some(ab) = backends.get(key) {
            match ab.backend.list_folders().await {
                Ok(account_folders) => {
                    let start = folders.len();
                    let account_folders: Vec<email::folder::Folder> = account_folders.into();
                    let mut count = 0;
                    for f in account_folders {
                        folders.push(FolderEntry {
                            name: f.name,
                            account: key.clone(),
                        });
                        count += 1;
                    }
                    if count > 0 {
                        sections.push(FolderSection {
                            name: key.clone(),
                            start,
                            count,
                        });
                    }
                }
                Err(e) => {
                    return Err(format!("Error loading folders for {key}: {e}"));
                }
            }
        }
    }

    Ok((folders, sections))
}

async fn run_event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    backends: &BackendMap,
    default_account: &str,
) -> Result<()> {
    loop {
        terminal.draw(|frame| ui::render(frame, app))?;

        let action = handle_event(&app.view, &app.folder_context, app.search.is_some())?;

        // Clear transient status on any keypress
        if !matches!(action, Action::None) && app.status.is_some() {
            app.status = None;
        }

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
                            continue;
                        }
                    } else {
                        continue;
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
                                        match email.to_read_tpl(&ab.account_config, |tpl| tpl).await
                                        {
                                            Ok(tpl) => body.push_str(&tpl),
                                            Err(e) => body
                                                .push_str(&format!("Error reading message: {e}")),
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

                    // Mark as read on server (best-effort; log failures)
                    if ctx.unseen {
                        if let Some(ab) = backends.get(&ctx.account_key) {
                            if let Ok(id) = ctx.id.parse::<usize>() {
                                let seen = Flags::from_iter([Flag::Seen]);
                                if let Err(e) =
                                    ab.backend.add_flags(&ctx.folder, &[id], &seen).await
                                {
                                    tracing::warn!(
                                        account = ctx.account_key,
                                        id,
                                        "failed to mark message as read on server: {e}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
            Action::BackToList => {
                app.view = View::MessageList;
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
                    if let Some(ab) = backends.get(&ctx.account_key) {
                        match ctx.id.parse::<usize>() {
                            Ok(id) => {
                                app.status = Some(Status::Working("Deleting…".into()));
                                terminal.draw(|frame| ui::render(frame, app))?;
                                match ab.backend.delete_messages(&ctx.folder, &[id]).await {
                                    Ok(_) => {
                                        app.remove_envelope(app.selected);
                                        app.view = View::MessageList;
                                        app.status = Some(Status::Info("Deleted".into()));
                                    }
                                    Err(e) => {
                                        app.status =
                                            Some(Status::Error(format!("Delete failed: {e}")));
                                    }
                                }
                            }
                            Err(e) => {
                                app.status = Some(Status::Error(format!("Delete failed: {e}")));
                            }
                        }
                    }
                }
            }
            Action::ArchiveMessage => {
                if let Some(ctx) = active_envelope_context(app, default_account) {
                    if let Some(ab) = backends.get(&ctx.account_key) {
                        let archive_folder = ab.archive_folder.clone();
                        match ctx.id.parse::<usize>() {
                            Ok(id) => {
                                app.status = Some(Status::Working("Archiving…".into()));
                                terminal.draw(|frame| ui::render(frame, app))?;
                                match ab
                                    .backend
                                    .move_messages(&ctx.folder, &archive_folder, &[id])
                                    .await
                                {
                                    Ok(_) => {
                                        app.remove_envelope(app.selected);
                                        app.view = View::MessageList;
                                        app.status = Some(Status::Info(format!(
                                            "Archived to {archive_folder}"
                                        )));
                                    }
                                    Err(e) => {
                                        app.status =
                                            Some(Status::Error(format!("Archive failed: {e}")));
                                    }
                                }
                            }
                            Err(e) => {
                                app.status = Some(Status::Error(format!("Archive failed: {e}")));
                            }
                        }
                    }
                }
            }
            Action::ToggleRead => {
                if let Some(ctx) = active_envelope_context(app, default_account) {
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
                                        let env = &mut app.envelopes[app.selected];
                                        env.unseen = !ctx.unseen;
                                        if ctx.unseen {
                                            if !env.flags.contains('S') {
                                                env.flags = sort_flags(&format!("S{}", env.flags));
                                            }
                                            app.status =
                                                Some(Status::Info("Marked as read".into()));
                                        } else {
                                            env.flags = env.flags.replace('S', "");
                                            app.status =
                                                Some(Status::Info("Marked as unread".into()));
                                        }
                                    }
                                    Err(e) => {
                                        app.status =
                                            Some(Status::Error(format!("Toggle read failed: {e}")));
                                    }
                                }
                            }
                            Err(e) => {
                                app.status =
                                    Some(Status::Error(format!("Toggle read failed: {e}")));
                            }
                        }
                    }
                }
            }
            Action::ToggleFlag => {
                if let Some(ctx) = active_envelope_context(app, default_account) {
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
                                        let env = &mut app.envelopes[app.selected];
                                        env.flagged = !ctx.flagged;
                                        if ctx.flagged {
                                            env.flags = env.flags.replace('F', "");
                                            app.status = Some(Status::Info("Unflagged".into()));
                                        } else {
                                            if !env.flags.contains('F') {
                                                env.flags = sort_flags(&format!("F{}", env.flags));
                                            }
                                            app.status = Some(Status::Info("Flagged".into()));
                                        }
                                    }
                                    Err(e) => {
                                        app.status =
                                            Some(Status::Error(format!("Toggle flag failed: {e}")));
                                    }
                                }
                            }
                            Err(e) => {
                                app.status =
                                    Some(Status::Error(format!("Toggle flag failed: {e}")));
                            }
                        }
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

                match fetch_folder_list(backends, None).await {
                    Ok((folders, sections)) => {
                        app.status = None;
                        app.view = View::FolderList(FolderListState {
                            folders,
                            sections,
                            selected: 0,
                        });
                    }
                    Err(err) => {
                        app.status = Some(Status::Error(err));
                    }
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
                                envelope_data = envelopes.iter().map(EnvelopeData::from).collect();
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
                app.confirm_search();
                app.cancel_search();
            }
            Action::SearchCancel => {
                app.cancel_search();
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}
