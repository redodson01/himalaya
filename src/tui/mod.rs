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
    sort_flags, AccountSection, App, EnvelopeData, FolderEntry, FolderEnvelopeState,
    FolderListState, FolderSection, MoveFolderPickerState, Status, View,
};
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
    // Determine the account name so single-account mode matches --all visually.
    // Find the account with `default = true`, matching into_account_configs(None) logic.
    let account_name = config
        .accounts
        .iter()
        .find_map(|(name, acct)| acct.default.filter(|&d| d).map(|_| name.clone()))
        .or_else(|| config.accounts.keys().next().cloned())
        .unwrap_or_default();

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
    let mut app = App::new(envelope_data, folder.clone()).with_sections(sections);

    let mut backends = HashMap::new();
    backends.insert(
        account_name.clone(),
        (backend, account_config, folder.clone(), archive_folder),
    );

    run_event_loop(&mut terminal, &mut app, &backends, &account_name).await
}

async fn run_all_accounts(config: TomlConfig) -> Result<()> {
    let mut account_names: Vec<String> = config.accounts.keys().cloned().collect();
    account_names.sort();

    let mut all_envelopes = Vec::new();
    let mut sections = Vec::new();
    let mut backends = HashMap::new();
    let mut folder = String::from("INBOX");

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

                let backend =
                    BackendBuilder::new(toml_account_config, account_config.clone(), |builder| {
                        builder
                            .without_features()
                            .with_list_envelopes(BackendFeatureSource::Context)
                            .with_list_folders(BackendFeatureSource::Context)
                            .with_get_messages(BackendFeatureSource::Context)
                            .with_add_flags(BackendFeatureSource::Context)
                            .with_remove_flags(BackendFeatureSource::Context)
                            .with_delete_messages(BackendFeatureSource::Context)
                            .with_move_messages(BackendFeatureSource::Context)
                    })
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
            Ok((name, Ok((backend, account_config, acct_folder, archive_folder, envelopes)))) => {
                if folder == "INBOX" && !acct_folder.is_empty() {
                    folder = acct_folder.clone();
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

                backends.insert(name, (backend, account_config, acct_folder, archive_folder));
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

    let mut app = App::new(all_envelopes, folder.clone()).with_sections(sections);

    // For multi-account, default_account is empty; we look up per-envelope
    let default_account = String::new();
    run_event_loop(&mut terminal, &mut app, &backends, &default_account).await
}

type BackendMap = HashMap<
    String,
    (
        pimalaya_tui::himalaya::backend::Backend,
        Arc<email::account::config::AccountConfig>,
        String, // source folder
        String, // archive folder
    ),
>;

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
            account_key: ctx.account_key.clone(),
            folder: ctx.folder_name.clone(),
        }),
        _ => app.envelopes.get(app.selected).map(|env| {
            let account_key = account_key_for(app, default_account);
            EnvelopeContext {
                id: env.id.clone(),
                unseen: env.unseen,
                flagged: env.flagged,
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

/// Re-fetch the main envelope list from all backends, updating app state.
async fn refresh_envelope_list(
    app: &mut App,
    backends: &BackendMap,
    terminal: &mut ratatui::DefaultTerminal,
) {
    app.status = Some(Status::Working("Refreshing…".to_string()));
    terminal.draw(|frame| ui::render(frame, app)).ok();

    let mut all_envelopes = Vec::new();
    let mut sections = Vec::new();

    let mut keys: Vec<String> = backends.keys().cloned().collect();
    keys.sort();

    for key in &keys {
        if let Some((backend, account_config, folder, _)) = backends.get(key) {
            let page_size = account_config.get_envelope_list_page_size();
            let opts = ListEnvelopesOptions {
                page: 0,
                page_size,
                query: None,
            };
            match backend.list_envelopes(folder, opts).await {
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
    if !app.envelopes.is_empty() {
        app.selected = app.selected.min(app.envelopes.len() - 1);
    } else {
        app.selected = 0;
    }
    app.status = None;
}

async fn run_event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    backends: &BackendMap,
    default_account: &str,
) -> Result<()> {
    loop {
        terminal.draw(|frame| ui::render(frame, app))?;

        let mut action = handle_event(&app.view, app.search.is_some())?;

        // Clear transient status on any keypress
        if !matches!(action, Action::None) && app.status.is_some() {
            app.status = None;
        }

        // This loop allows SearchConfirm to set a follow-up action and re-enter.
        let mut clear_search_after = false;
        loop {
            // Determine if we're in a folder context (FolderEnvelopeList or
            // MessageRead with folder_context). Extract envelope info accordingly.
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
                        terminal.draw(|frame| ui::render(frame, app))?;

                        let content = if let Some((backend, account_config, _, _)) =
                            backends.get(&ctx.account_key)
                        {
                            match ctx.id.parse::<usize>() {
                                Ok(id) => match backend.get_messages(&ctx.folder, &[id]).await {
                                    Ok(emails) => {
                                        let mut body = String::new();
                                        for email in emails.to_vec() {
                                            match email.to_read_tpl(account_config, |tpl| tpl).await
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
                        if in_folder_context {
                            // Take folder state out of current view, put into MessageRead
                            let old_view = std::mem::replace(&mut app.view, View::EnvelopeList);
                            let folder_state = match old_view {
                                View::FolderEnvelopeList(state) => state,
                                View::MessageRead {
                                    folder_context: Some(ctx),
                                    ..
                                } => *ctx,
                                _ => unreachable!(),
                            };
                            app.view = View::MessageRead {
                                content,
                                scroll: 0,
                                folder_context: Some(Box::new(folder_state)),
                            };
                        } else {
                            app.view = View::MessageRead {
                                content,
                                scroll: 0,
                                folder_context: None,
                            };
                        }
                        terminal.draw(|frame| ui::render(frame, app))?;

                        // Mark as read on server in background
                        if ctx.unseen {
                            if let Some((backend, _, _, _)) = backends.get(&ctx.account_key) {
                                if let Ok(id) = ctx.id.parse::<usize>() {
                                    let seen = Flags::from_iter([Flag::Seen]);
                                    let _ = backend.add_flags(&ctx.folder, &[id], &seen).await;
                                }
                            }
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
                    } else {
                        // Returning to main envelope list — refresh from server
                        refresh_envelope_list(app, backends, terminal).await;
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
                        app.status = Some(Status::Working("Deleting…".to_string()));
                        terminal.draw(|frame| ui::render(frame, app))?;

                        let mut error: Option<String> = None;
                        if let Some((backend, _, _, _)) = backends.get(&account_key) {
                            if let Ok(id) = id_str.parse::<usize>() {
                                match backend.delete_messages(&folder, &[id]).await {
                                    Ok(_) => {
                                        if in_folder_context {
                                            // Remove from folder state and go back to folder envelope list
                                            let old_view = std::mem::replace(
                                                &mut app.view,
                                                View::EnvelopeList,
                                            );
                                            let mut state = match old_view {
                                                View::FolderEnvelopeList(s) => s,
                                                View::MessageRead {
                                                    folder_context: Some(ctx),
                                                    ..
                                                } => *ctx,
                                                _ => unreachable!(),
                                            };
                                            state.remove_envelope(state.selected);
                                            app.view = View::FolderEnvelopeList(state);
                                        } else {
                                            app.remove_envelope(app.selected);
                                            if !matches!(app.view, View::EnvelopeList) {
                                                app.view = View::EnvelopeList;
                                            }
                                        }
                                    }
                                    Err(e) => error = Some(format!("Delete failed: {e}")),
                                }
                            }
                        }
                        app.status = error.map(Status::Error);
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
                        if let Some((backend, _, _, _)) = backends.get(&ctx.account_key) {
                            if let Ok(id) = ctx.id.parse::<usize>() {
                                let seen = Flags::from_iter([Flag::Seen]);
                                let result = if ctx.unseen {
                                    backend.add_flags(&ctx.folder, &[id], &seen).await
                                } else {
                                    backend.remove_flags(&ctx.folder, &[id], &seen).await
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
                                        // If in main MessageRead, go back to list
                                        if !in_folder_context
                                            && !matches!(app.view, View::EnvelopeList)
                                        {
                                            app.view = View::EnvelopeList;
                                        }
                                    }
                                    Err(e) => error = Some(format!("Toggle read failed: {e}")),
                                }
                            }
                        }
                        app.status = error.map(Status::Error);
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
                        if let Some((backend, _, _, _)) = backends.get(&ctx.account_key) {
                            if let Ok(id) = ctx.id.parse::<usize>() {
                                let flagged = Flags::from_iter([Flag::Flagged]);
                                let result = if ctx.flagged {
                                    backend.remove_flags(&ctx.folder, &[id], &flagged).await
                                } else {
                                    backend.add_flags(&ctx.folder, &[id], &flagged).await
                                };
                                match result {
                                    Ok(_) => {
                                        if let Some(env) = active_envelope_mut(app) {
                                            env.flagged = !ctx.flagged;
                                            if ctx.flagged {
                                                env.flags = env.flags.replace('F', "");
                                            } else if !env.flags.contains('F') {
                                                env.flags = sort_flags(&format!("F{}", env.flags));
                                            }
                                        }
                                        if !in_folder_context
                                            && !matches!(app.view, View::EnvelopeList)
                                        {
                                            app.view = View::EnvelopeList;
                                        }
                                    }
                                    Err(e) => error = Some(format!("Flag toggle failed: {e}")),
                                }
                            }
                        }
                        app.status = error.map(Status::Error);
                    }
                }
                Action::OpenFolderList => {
                    app.status = Some(Status::Working("Loading folders…".to_string()));
                    terminal.draw(|frame| ui::render(frame, app))?;

                    let saved_envelope_selected = app.selected;
                    let mut folders = Vec::new();
                    let mut sections = Vec::new();
                    let mut error: Option<String> = None;

                    let mut keys: Vec<String> = backends.keys().cloned().collect();
                    keys.sort();
                    for key in &keys {
                        if let Some((backend, _, _, _)) = backends.get(key) {
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
                            saved_envelope_selected,
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
                Action::BackFromFolders => {
                    if let View::FolderList(state) = &app.view {
                        app.selected = state.saved_envelope_selected;
                    }
                    app.view = View::EnvelopeList;
                    refresh_envelope_list(app, backends, terminal).await;
                }
                Action::CancelMove => {
                    let old_view = std::mem::replace(&mut app.view, View::EnvelopeList);
                    if let View::MoveFolderPicker(picker) = old_view {
                        if let Some(fe_state) = picker.folder_envelope_state {
                            app.view = View::FolderEnvelopeList(*fe_state);
                        } else {
                            // Returning to main envelope list from picker
                            refresh_envelope_list(app, backends, terminal).await;
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

                        if let Some((backend, account_config, _, _)) = backends.get(&key) {
                            let page_size = account_config.get_envelope_list_page_size();
                            let opts = ListEnvelopesOptions {
                                page: 0,
                                page_size,
                                query: None,
                            };
                            match backend.list_envelopes(&folder_name, opts).await {
                                Ok(envelopes) => {
                                    envelope_data =
                                        envelopes.iter().map(EnvelopeData::from).collect();
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
                            // Take the FolderListState out and use it as parent
                            let old_view = std::mem::replace(&mut app.view, View::EnvelopeList);
                            let parent = if let View::FolderList(state) = old_view {
                                state
                            } else {
                                unreachable!()
                            };
                            app.view = View::FolderEnvelopeList(FolderEnvelopeState {
                                envelopes: envelope_data,
                                selected: 0,
                                folder_name,
                                account_key: key,
                                parent,
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
                    // Clear search if no follow-up (e.g. empty results already cleared)
                    app.cancel_search();
                }
                Action::SearchCancel => {
                    app.cancel_search();
                }
                Action::ArchiveMessage => {
                    if let Some(ctx) = active_envelope_context(app, default_account) {
                        let (id_str, account_key, source_folder) =
                            (ctx.id, ctx.account_key, ctx.folder);
                        app.status = Some(Status::Working("Archiving…".to_string()));
                        terminal.draw(|frame| ui::render(frame, app))?;

                        let mut error: Option<String> = None;
                        if let Some((backend, _, _, archive_folder)) = backends.get(&account_key) {
                            if let Ok(id) = id_str.parse::<usize>() {
                                match backend
                                    .move_messages(&source_folder, archive_folder, &[id])
                                    .await
                                {
                                    Ok(_) => {
                                        if in_folder_context {
                                            let old_view = std::mem::replace(
                                                &mut app.view,
                                                View::EnvelopeList,
                                            );
                                            let mut state = match old_view {
                                                View::FolderEnvelopeList(s) => s,
                                                View::MessageRead {
                                                    folder_context: Some(ctx),
                                                    ..
                                                } => *ctx,
                                                _ => unreachable!(),
                                            };
                                            state.remove_envelope(state.selected);
                                            app.view = View::FolderEnvelopeList(state);
                                        } else {
                                            app.remove_envelope(app.selected);
                                            if !matches!(app.view, View::EnvelopeList) {
                                                app.view = View::EnvelopeList;
                                            }
                                        }
                                    }
                                    Err(e) => error = Some(format!("Archive failed: {e}")),
                                }
                            }
                        }
                        app.status = error.map(Status::Error);
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
                        terminal.draw(|frame| ui::render(frame, app))?;

                        let mut folders = Vec::new();
                        let mut error: Option<String> = None;

                        if let Some((backend, _, _, _)) = backends.get(&account_key) {
                            match backend.list_folders().await {
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
                            // Extract FolderEnvelopeState if in folder context
                            let fe_state = if in_folder_context {
                                let old_view = std::mem::replace(&mut app.view, View::EnvelopeList);
                                match old_view {
                                    View::FolderEnvelopeList(s) => Some(Box::new(s)),
                                    View::MessageRead {
                                        folder_context: Some(ctx),
                                        ..
                                    } => Some(ctx),
                                    _ => None,
                                }
                            } else {
                                // If in MessageRead without folder context, go back to list
                                if matches!(app.view, View::MessageRead { .. }) {
                                    app.view = View::EnvelopeList;
                                }
                                None
                            };

                            app.view = View::MoveFolderPicker(MoveFolderPickerState {
                                folders,
                                selected: 0,
                                source_envelope_id: id_str,
                                source_envelope_index: env_index,
                                source_folder,
                                account_key,
                                return_to_folder: in_folder_context,
                                folder_envelope_state: fe_state,
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

                            app.status = Some(Status::Working(format!("Moving to {target_name}…")));
                            terminal.draw(|frame| ui::render(frame, app))?;

                            let mut error: Option<String> = None;
                            if let Some((backend, _, _, _)) = backends.get(&account_key) {
                                if let Ok(id) = id_str.parse::<usize>() {
                                    match backend
                                        .move_messages(&source_folder, &target_name, &[id])
                                        .await
                                    {
                                        Ok(_) => {
                                            // Take the picker state
                                            let old_view = std::mem::replace(
                                                &mut app.view,
                                                View::EnvelopeList,
                                            );
                                            if let View::MoveFolderPicker(picker) = old_view {
                                                if return_to_folder {
                                                    if let Some(mut fe_state) =
                                                        picker.folder_envelope_state
                                                    {
                                                        fe_state.remove_envelope(
                                                            picker.source_envelope_index,
                                                        );
                                                        app.view =
                                                            View::FolderEnvelopeList(*fe_state);
                                                    }
                                                } else {
                                                    app.remove_envelope(
                                                        picker.source_envelope_index,
                                                    );
                                                    // already set to EnvelopeList
                                                }
                                            }
                                            app.status = Some(Status::Working(format!(
                                                "Moved to {target_name}"
                                            )));
                                        }
                                        Err(e) => {
                                            error = Some(format!("Move failed: {e}"));
                                        }
                                    }
                                }
                            }
                            if let Some(err) = error {
                                app.status = Some(Status::Error(err));
                            }
                        }
                    }
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
