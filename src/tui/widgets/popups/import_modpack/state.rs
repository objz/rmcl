// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// state machine for the modpack import wizard.
// browses modpacks from configured providers and keeps the archive import flow
// for modrinth URLs, project slugs, version IDs, and local pack archives.

use super::super::LoadState;
use crate::instance::import::{ImportInput, ImportSummary, parse_import_input};
use crate::net::modrinth::{self, VersionInfo};
use crate::tui::widgets::instances;
use crate::tui::widgets::search::SearchState;
use crossterm::event::{KeyCode, KeyEvent};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::Level;

pub(super) static IMPORT_STATE: LazyLock<Arc<Mutex<ImportWizardState>>> =
    LazyLock::new(|| Arc::new(Mutex::new(ImportWizardState::default())));
pub(super) static IMPORT_RESULT: LazyLock<Arc<Mutex<Option<ImportResult>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(None)));
pub(super) static DISCOVERY_STATE: LazyLock<Mutex<crate::tui::widgets::content::DiscoveryState>> =
    LazyLock::new(|| Mutex::new(crate::tui::widgets::content::DiscoveryState::new_modpacks()));
static NEXT_IMPORT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct ImportResult {
    pub summary: ImportSummary,
}

#[derive(Debug, Default, Clone, PartialEq)]
pub enum ImportStep {
    #[default]
    Discover,
    Input,
    Fetching,
    Version,
    Confirm,
}

#[derive(Debug, Clone)]
pub struct ImportWizardState {
    pub step: ImportStep,
    pub input: String,
    pub project_title: Option<String>,
    pub versions: LoadState<Vec<VersionInfo>>,
    pub version_idx: usize,
    pub version_search: SearchState,
    pub summary: Option<ImportSummary>,
    pub from_discovery: bool,
    request_id: u64,
}

impl Default for ImportWizardState {
    fn default() -> Self {
        Self {
            step: ImportStep::Discover,
            input: String::new(),
            project_title: None,
            versions: LoadState::Idle,
            version_idx: 0,
            version_search: SearchState::default(),
            summary: None,
            from_discovery: false,
            request_id: 0,
        }
    }
}

impl ImportWizardState {
    pub fn reset(&mut self) {
        *self = ImportWizardState::default();
    }
}

pub fn handle_key(key_event: &KeyEvent, instances_state: &mut instances::State) {
    let mut state = match IMPORT_STATE.lock() {
        Ok(state) => state,
        Err(e) => {
            tracing::error!("Import state lock poisoned: {}", e);
            instances_state.show_import_popup = false;
            return;
        }
    };
    if state.step == ImportStep::Discover {
        drop(state);
        handle_discovery_key(key_event, instances_state);
        return;
    }

    match state.step {
        ImportStep::Discover => unreachable!(),
        ImportStep::Input => handle_input_key(&mut state, key_event, instances_state),
        ImportStep::Fetching => handle_fetching_key(&mut state, key_event, instances_state),
        ImportStep::Version => handle_version_key(&mut state, key_event, instances_state),
        ImportStep::Confirm => handle_confirm_key(&mut state, key_event, instances_state),
    }
}

pub fn open() {
    if let Ok(mut wizard) = IMPORT_STATE.lock() {
        wizard.reset();
    }
    #[cfg(not(test))]
    start_discovery_search();
}

pub fn drain(picker: &ratatui_image::picker::Picker) {
    if let Ok(mut state) = DISCOVERY_STATE.lock() {
        state.drain_pending();
        state.list.drain_pending();
        state.list.request_image_loads(picker);
        state.list.drain_image_loads(picker);
        if state.search_due() {
            drop(state);
            start_discovery_search();
        }
    }
}

pub fn has_version_popup() -> bool {
    DISCOVERY_STATE
        .lock()
        .is_ok_and(|state| state.version_popup.is_some())
}

pub fn handle_discovery_click(x: u16, y: u16) -> bool {
    if !IMPORT_STATE
        .lock()
        .is_ok_and(|state| state.step == ImportStep::Discover)
    {
        return false;
    }
    let (handled, request) = match DISCOVERY_STATE.lock() {
        Ok(mut discovery) => {
            if discovery.search.active
                || discovery.project_page_open()
                || discovery.version_popup.is_some()
            {
                return false;
            }
            let handled = discovery.list.click_page(x, y);
            let request = handled.then(|| discovery.begin_next_page()).flatten();
            (handled, request)
        }
        Err(_) => return false,
    };
    if let Some(request) = request {
        spawn_discovery_request(request);
    }
    handled
}

fn handle_discovery_key(key_event: &KeyEvent, instances_state: &mut instances::State) {
    let mut discovery = match DISCOVERY_STATE.lock() {
        Ok(state) => state,
        Err(_) => return,
    };
    let search_active = discovery.search.active;
    let popup_open = discovery.version_popup.is_some();
    let project_page_open = discovery.project_page_open();
    match key_event.code {
        KeyCode::Esc if !search_active && !popup_open && !project_page_open => {
            drop(discovery);
            if let Ok(mut state) = IMPORT_STATE.lock() {
                close_popup(&mut state, instances_state);
            }
        }
        KeyCode::Char('i') if !search_active && !popup_open && !project_page_open => {
            drop(discovery);
            if let Ok(mut state) = IMPORT_STATE.lock() {
                state.step = ImportStep::Input;
            }
        }
        KeyCode::Enter if !search_active && !popup_open && !project_page_open => {
            let request = discovery.begin_project_page();
            drop(discovery);
            if let Some(request) = request {
                crate::tui::widgets::content::discovery::spawn_project_page(request);
            }
        }
        KeyCode::Char('v') if !search_active && !popup_open => {
            let request = discovery.begin_versions();
            drop(discovery);
            if let Some(request) = request {
                spawn_versions(request);
            }
        }
        KeyCode::Tab if popup_open => {
            let request = discovery.switch_version_source();
            drop(discovery);
            if let Some(request) = request {
                spawn_versions(request);
            }
        }
        KeyCode::Enter if popup_open => {
            if discovery.select_minecraft_version() {
                return;
            }
            let request = take_discovered_version(&mut discovery);
            drop(discovery);
            if let Some(request) = request {
                start_discovered_download(request);
            }
        }
        _ => {
            let navigate =
                matches!(
                    key_event.code,
                    KeyCode::Char('j') | KeyCode::Char('k') | KeyCode::Down | KeyCode::Up
                ) || crate::tui::widgets::content::discovery::page_key_direction(key_event)
                    .is_some();
            crate::tui::widgets::content::discovery::handle_key(key_event, &mut discovery);
            let request = navigate.then(|| discovery.begin_next_page()).flatten();
            drop(discovery);
            if let Some(request) = request {
                spawn_discovery_request(request);
            }
        }
    }
}

fn take_discovered_version(
    discovery: &mut crate::tui::widgets::content::DiscoveryState,
) -> Option<crate::tui::widgets::content::discovery::InstallRequest> {
    discovery
        .begin_confirmation()
        .then(|| discovery.begin_install())?
}

pub fn take_result() -> Option<ImportResult> {
    match IMPORT_RESULT.lock() {
        Ok(mut r) => r.take(),
        Err(_) => None,
    }
}

fn handle_input_key(
    state: &mut ImportWizardState,
    key_event: &KeyEvent,
    _instances_state: &mut instances::State,
) {
    match key_event.code {
        KeyCode::Esc => state.step = ImportStep::Discover,
        KeyCode::Backspace => {
            crate::tui::widgets::search::backspace(&mut state.input, key_event.modifiers);
        }
        KeyCode::Enter => {
            if state.input.trim().is_empty() {
                return;
            }
            start_resolve(state);
        }
        KeyCode::Char(c) => {
            state.input.push(c);
        }
        _ => {}
    }
}

fn handle_fetching_key(
    state: &mut ImportWizardState,
    key_event: &KeyEvent,
    instances_state: &mut instances::State,
) {
    if key_event.code == KeyCode::Esc {
        close_popup(state, instances_state);
    }
}

fn handle_version_key(
    state: &mut ImportWizardState,
    key_event: &KeyEvent,
    instances_state: &mut instances::State,
) {
    if state.version_search.active {
        match key_event.code {
            KeyCode::Esc => {
                state.version_search.deactivate();
                clamp_version_index(state);
                return;
            }
            KeyCode::Backspace => {
                state.version_search.backspace(key_event.modifiers);
                clamp_version_index(state);
                return;
            }
            KeyCode::Char('j') | KeyCode::Down => {}
            KeyCode::Char('k') | KeyCode::Up => {}
            KeyCode::Enter => {
                state.version_search.active = false;
                return;
            }
            KeyCode::Char(c) => {
                state.version_search.push(c);
                state.version_idx = 0;
                return;
            }
            _ => {}
        }
    }

    let visible_count = visible_versions(state).len();

    match key_event.code {
        KeyCode::Esc => close_popup(state, instances_state),
        KeyCode::Left | KeyCode::Char('h') if !state.version_search.active => {
            state.step = ImportStep::Input;
            state.versions = LoadState::Idle;
            state.version_idx = 0;
            state.version_search.deactivate();
        }
        KeyCode::Char('j') | KeyCode::Down if visible_count > 0 => {
            state.version_idx = (state.version_idx + 1).min(visible_count.saturating_sub(1));
        }
        KeyCode::Char('k') | KeyCode::Up => {
            state.version_idx = state.version_idx.saturating_sub(1);
        }
        KeyCode::Char('/') if !state.version_search.active => {
            state.version_search.activate();
            state.version_idx = 0;
        }
        KeyCode::Enter if !state.version_search.active => {
            let selected = selected_version(state);
            if selected.is_none() {
                return;
            }
            start_version_download(state);
        }
        _ => {}
    }
}

fn handle_confirm_key(
    state: &mut ImportWizardState,
    key_event: &KeyEvent,
    instances_state: &mut instances::State,
) {
    match key_event.code {
        KeyCode::Esc => close_popup(state, instances_state),
        // if it came from a local file, there's no version list to go back to
        KeyCode::Left | KeyCode::Char('h') => {
            if matches!(state.versions, LoadState::Loaded(_)) {
                state.step = ImportStep::Version;
            } else if state.from_discovery {
                state.step = ImportStep::Discover;
            } else {
                state.step = ImportStep::Input;
            }
        }
        KeyCode::Enter => {
            let summary = match state.summary.take() {
                Some(s) => s,
                None => return,
            };

            match IMPORT_RESULT.lock() {
                Ok(mut result) => {
                    *result = Some(ImportResult { summary });
                }
                Err(e) => {
                    tracing::error!("Import result lock poisoned: {}", e);
                }
            }

            close_popup(state, instances_state);
        }
        _ => {}
    }
}

// pushes an error toast and rewinds the wizard to a previous step
fn update_current_request(
    state_arc: &Arc<Mutex<ImportWizardState>>,
    request_id: u64,
    update: impl FnOnce(&mut ImportWizardState),
) -> bool {
    if let Ok(mut state) = state_arc.lock()
        && state.request_id == request_id
    {
        update(&mut state);
        return true;
    }
    false
}

fn set_error_and_back(
    state_arc: &Arc<Mutex<ImportWizardState>>,
    request_id: u64,
    msg: String,
    step: ImportStep,
) {
    if update_current_request(state_arc, request_id, |state| state.step = step) {
        push_import_error(msg);
    }
}

fn begin_import_request(state: &mut ImportWizardState) -> u64 {
    let request_id = NEXT_IMPORT_REQUEST_ID.fetch_add(1, Ordering::Relaxed);
    state.request_id = request_id;
    request_id
}

// parses user input to figure out what they gave us, then dispatches
// to the appropriate resolve path (slug lookup, direct version, or local file)
fn start_resolve(state: &mut ImportWizardState) {
    let input_text = state.input.clone();
    state.step = ImportStep::Fetching;
    state.from_discovery = false;
    state.project_title = None;
    state.versions = LoadState::Idle;
    state.version_idx = 0;
    state.version_search.deactivate();
    state.summary = None;
    let request_id = begin_import_request(state);

    let state_arc = IMPORT_STATE.clone();

    tokio::spawn(async move {
        let client = crate::net::HttpClient::new();
        let parsed = parse_import_input(&input_text);

        match parsed {
            ImportInput::ProjectSlug(slug) => {
                resolve_project_slug(state_arc, request_id, &client, &slug).await;
            }
            ImportInput::VersionId {
                slug: _,
                version_id,
            } => {
                resolve_version_id(state_arc, request_id, &client, &version_id).await;
            }
            ImportInput::LocalFile(path) => {
                resolve_local_file(state_arc, request_id, &path);
            }
        }
    });
}

async fn resolve_project_slug(
    state_arc: Arc<Mutex<ImportWizardState>>,
    request_id: u64,
    client: &crate::net::HttpClient,
    slug: &str,
) {
    match modrinth::fetch_project(client, slug).await {
        Ok(project) => match modrinth::fetch_versions(client, slug).await {
            Ok(versions) => {
                update_current_request(&state_arc, request_id, |state| {
                    state.project_title = Some(project.title);
                    state.versions = LoadState::Loaded(versions);
                    state.version_idx = 0;
                    state.version_search.deactivate();
                    state.step = ImportStep::Version;
                });
            }
            Err(e) => set_error_and_back(
                &state_arc,
                request_id,
                format!("Failed to fetch versions: {}", e),
                ImportStep::Input,
            ),
        },
        Err(e) => set_error_and_back(
            &state_arc,
            request_id,
            format!("Failed to fetch project: {}", e),
            ImportStep::Input,
        ),
    }
}

async fn resolve_version_id(
    state_arc: Arc<Mutex<ImportWizardState>>,
    request_id: u64,
    client: &crate::net::HttpClient,
    version_id: &str,
) {
    match modrinth::fetch_version(client, version_id).await {
        Ok(version) => {
            let meta_dir = crate::config::SETTINGS.read().paths.resolve_meta_dir();
            let tmp_dir = crate::storage::MetadataPaths::new(&meta_dir).temporary();
            if let Err(e) = tokio::fs::create_dir_all(&tmp_dir).await {
                set_error_and_back(
                    &state_arc,
                    request_id,
                    format!("Failed to create tmp dir: {}", e),
                    ImportStep::Input,
                );
                return;
            }

            match modrinth::download_mrpack(client, &version, &tmp_dir).await {
                Ok(mrpack_path) => match crate::instance::import::build_summary(&mrpack_path) {
                    Ok(summary) => {
                        update_current_request(&state_arc, request_id, |state| {
                            state.summary = Some(summary);
                            state.step = ImportStep::Confirm;
                        });
                    }
                    Err(e) => set_error_and_back(
                        &state_arc,
                        request_id,
                        format!("Failed to build summary: {}", e),
                        ImportStep::Input,
                    ),
                },
                Err(e) => set_error_and_back(
                    &state_arc,
                    request_id,
                    format!("Failed to download mrpack: {}", e),
                    ImportStep::Input,
                ),
            }
        }
        Err(e) => set_error_and_back(
            &state_arc,
            request_id,
            format!("Failed to fetch version: {}", e),
            ImportStep::Input,
        ),
    }
}

fn resolve_local_file(state_arc: Arc<Mutex<ImportWizardState>>, request_id: u64, path: &str) {
    let resolved = crate::config::settings::resolve_path(path);

    match crate::instance::import::build_summary(&resolved) {
        Ok(summary) => {
            update_current_request(&state_arc, request_id, |state| {
                state.summary = Some(summary);
                state.step = ImportStep::Confirm;
            });
        }
        Err(e) => set_error_and_back(
            &state_arc,
            request_id,
            format!("Failed to parse pack: {}", e),
            ImportStep::Input,
        ),
    }
}

// user picked a version from the list. download the .mrpack,
// build a summary, and move to confirm.
fn start_version_download(state: &mut ImportWizardState) {
    let version = match selected_version(state) {
        Some(v) => v.clone(),
        None => return,
    };

    state.step = ImportStep::Fetching;
    let request_id = begin_import_request(state);

    let state_arc = IMPORT_STATE.clone();

    tokio::spawn(async move {
        let client = crate::net::HttpClient::new();
        let meta_dir = crate::config::SETTINGS.read().paths.resolve_meta_dir();
        let tmp_dir = crate::storage::MetadataPaths::new(&meta_dir).temporary();
        if let Err(e) = tokio::fs::create_dir_all(&tmp_dir).await {
            set_error_and_back(
                &state_arc,
                request_id,
                format!("Failed to create tmp dir: {}", e),
                ImportStep::Version,
            );
            return;
        }

        match modrinth::download_mrpack(&client, &version, &tmp_dir).await {
            Ok(mrpack_path) => match crate::instance::import::build_summary(&mrpack_path) {
                Ok(summary) => {
                    update_current_request(&state_arc, request_id, |state| {
                        state.summary = Some(summary);
                        state.step = ImportStep::Confirm;
                    });
                }
                Err(e) => set_error_and_back(
                    &state_arc,
                    request_id,
                    format!("Failed to build summary: {}", e),
                    ImportStep::Version,
                ),
            },
            Err(e) => set_error_and_back(
                &state_arc,
                request_id,
                format!("Failed to download mrpack: {}", e),
                ImportStep::Version,
            ),
        }
    });
}

fn start_discovery_search() {
    let (query, request) = match DISCOVERY_STATE.lock() {
        Ok(mut state) => (state.search.query.clone(), state.begin_modpack_search()),
        Err(_) => return,
    };
    spawn_discovery_request_with_query(query, request);
}

fn spawn_discovery_request(request: crate::tui::widgets::content::discovery::DiscoveryRequest) {
    let query = DISCOVERY_STATE
        .lock()
        .map(|state| state.search.query.clone())
        .unwrap_or_default();
    spawn_discovery_request_with_query(query, request);
}

fn spawn_discovery_request_with_query(
    query: String,
    request: crate::tui::widgets::content::discovery::DiscoveryRequest,
) {
    crate::tui::widgets::content::discovery::spawn_provider_search(
        query,
        crate::tui::widgets::content::discovery::DiscoveryTarget::Modpacks,
        crate::config::SETTINGS.read().paths.resolve_meta_dir(),
        request,
    );
}

fn spawn_versions(request: crate::tui::widgets::content::discovery::VersionsRequest) {
    tokio::spawn(async move {
        let client = crate::net::HttpClient::new();
        let result = match request.provider.as_str() {
            "curseforge" => match crate::net::curseforge::api_key() {
                Some(api_key) => {
                    crate::net::curseforge::fetch_versions(
                        &client,
                        api_key,
                        &request.project_id,
                        "",
                        None,
                    )
                    .await
                }
                None => Err(crate::net::NetError::Parse(
                    "CurseForge API key is not configured".to_owned(),
                )),
            },
            _ => crate::net::modrinth::fetch_versions(&client, &request.project_id).await,
        }
        .map_err(|error| error.to_string());
        crate::tui::widgets::content::DiscoveryState::push_action_result(
            &request.pending,
            crate::tui::widgets::content::discovery::DiscoveryActionResult::Versions {
                request_id: request.request_id,
                project_id: request.project_id,
                result,
            },
        );
    });
}

fn start_discovered_download(request: crate::tui::widgets::content::discovery::InstallRequest) {
    let request_id = if let Ok(mut state) = IMPORT_STATE.lock() {
        state.step = ImportStep::Fetching;
        state.from_discovery = true;
        begin_import_request(&mut state)
    } else {
        return;
    };
    tokio::spawn(async move {
        let client = crate::net::HttpClient::new();
        let registry = crate::instance::content::provider::ProviderRegistry::configured(client);
        let tmp_dir = crate::storage::MetadataPaths::new(
            crate::config::SETTINGS.read().paths.resolve_meta_dir(),
        )
        .temporary();
        let result: Result<crate::instance::import::ImportSummary, crate::net::NetError> = async {
            let source = crate::instance::ProviderProject {
                provider: request.provider.clone(),
                project_id: request.project_id.clone(),
                version_id: request.version.id.clone(),
            };
            tokio::fs::create_dir_all(&tmp_dir).await?;
            let provider = registry.get(&request.provider).ok_or_else(|| {
                crate::net::NetError::Parse(format!(
                    "{} content provider is unavailable",
                    request.provider
                ))
            })?;
            let outcome = provider
                .download_version(&request.version, &tmp_dir, None)
                .await?;
            let path = match outcome {
                crate::net::modrinth::DownloadOutcome::Downloaded(path)
                | crate::net::modrinth::DownloadOutcome::SkippedExisting(path) => path,
            };
            let mut summary = crate::instance::import::build_summary(&path)
                .map_err(crate::net::NetError::Parse)?;
            summary.source = Some(source);
            Ok(summary)
        }
        .await;
        match result {
            Ok(summary) => {
                update_current_request(&IMPORT_STATE, request_id, |state| {
                    state.summary = Some(summary);
                    state.step = ImportStep::Confirm;
                });
            }
            Err(error) => {
                if update_current_request(&IMPORT_STATE, request_id, |state| {
                    state.step = ImportStep::Discover;
                }) {
                    push_import_error(format!("Failed to prepare modpack: {error}"));
                }
            }
        }
        crate::feedback::request_redraw();
    });
}

fn close_popup(state: &mut ImportWizardState, instances_state: &mut instances::State) {
    state.reset();
    instances_state.show_import_popup = false;
}

fn push_import_error(msg: String) {
    crate::feedback::errors::push_error(crate::feedback::errors::ErrorEvent {
        id: 0,
        level: Level::ERROR,
        message: msg,
        pushed_at: Instant::now(),
    });
}

pub(super) fn visible_versions(state: &ImportWizardState) -> Vec<VersionInfo> {
    match &state.versions {
        LoadState::Loaded(versions) => {
            let q = state.version_search.query.to_lowercase();
            versions
                .iter()
                .filter(|v| {
                    q.is_empty()
                        || v.name.to_lowercase().contains(&q)
                        || v.version_number.to_lowercase().contains(&q)
                })
                .cloned()
                .collect()
        }
        _ => Vec::new(),
    }
}

fn visible_versions_ref<'a>(
    versions: &'a [VersionInfo],
    search: &SearchState,
) -> Vec<&'a VersionInfo> {
    let q = search.query.to_lowercase();
    versions
        .iter()
        .filter(|v| {
            q.is_empty()
                || v.name.to_lowercase().contains(&q)
                || v.version_number.to_lowercase().contains(&q)
        })
        .collect()
}

fn selected_version(state: &ImportWizardState) -> Option<&VersionInfo> {
    if let LoadState::Loaded(ref versions) = state.versions {
        let visible: Vec<_> = visible_versions_ref(versions, &state.version_search);
        visible.get(state.version_idx).copied()
    } else {
        None
    }
}

fn clamp_version_index(state: &mut ImportWizardState) {
    let count = visible_versions(state).len();
    if count == 0 {
        state.version_idx = 0;
    } else if state.version_idx >= count {
        state.version_idx = count.saturating_sub(1);
    }
}

#[cfg(test)]
#[path = "../../../tests/widgets/popups/import_modpack/state.rs"]
mod tests;
