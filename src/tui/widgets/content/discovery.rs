// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent};

use crate::instance::content::entry::ContentEntry;
use crate::instance::{ContentKind, InstanceConfig, ModLoader};
use crate::net::modrinth::{DiscoveryProject, DiscoveryResults, VersionInfo};

use super::list::{ContentListState, ContentStream};

pub const PAGE_SIZE: usize = 100;
const PREFETCH_VIEWPORTS: usize = 2;
const MIN_PREFETCH_ITEMS: usize = 10;
const SEARCH_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);
const PAGE_RETRY_BASE_DELAY: std::time::Duration = std::time::Duration::from_millis(500);
const PAGE_RETRY_MAX_DELAY: std::time::Duration = std::time::Duration::from_secs(8);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ContentMode {
    #[default]
    Installed,
    Discover,
}

impl ContentMode {
    pub fn toggle(self) -> Self {
        match self {
            Self::Installed => Self::Discover,
            Self::Discover => Self::Installed,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Installed => "Installed",
            Self::Discover => "Discovery",
        }
    }
}

pub struct PendingDiscoveryResult {
    generation: u64,
    offset: usize,
    result: Result<DiscoveryPageResult, DiscoveryPageError>,
    sources: Vec<(String, crate::instance::ProviderProject)>,
}

pub struct DiscoveryPageError {
    pub message: String,
    pub retryable: bool,
}

pub struct DiscoveryPageResult {
    pub received: usize,
    pub total_hits: usize,
}

pub(crate) struct MergedDiscoveryProject {
    pub stem: String,
    pub provider: String,
    pub project: DiscoveryProject,
}

pub(crate) struct MergedDiscoveryResults {
    pub projects: Vec<MergedDiscoveryProject>,
    pub sources: Vec<(String, crate::instance::ProviderProject)>,
    pub received: usize,
    pub total_hits: usize,
}

pub struct DiscoveryRequest {
    pub generation: u64,
    pub offset: usize,
    pub limit: usize,
    pub pending: PendingDiscovery,
    pub stream: ContentStream,
    pub reconcile: bool,
    pub loaded_icon_stems: std::collections::HashSet<String>,
    pub known_projects: std::collections::HashMap<String, (String, String)>,
}

pub(crate) struct ContentDiscoveryTarget {
    pub instance: InstanceConfig,
    pub kind: ContentKind,
    pub manifest: Option<crate::instance::ContentManifest>,
    pub minecraft_dir: PathBuf,
}

pub(crate) enum DiscoveryTarget {
    Content(Box<ContentDiscoveryTarget>),
    Modpacks,
}

async fn search_provider(
    provider: Option<&dyn crate::instance::content::provider::ContentProvider>,
    enabled: bool,
    target: &DiscoveryTarget,
    query: &str,
    offset: usize,
    limit: usize,
) -> Option<Result<DiscoveryResults, crate::net::NetError>> {
    let provider = provider.filter(|_| enabled)?;
    Some(match target {
        DiscoveryTarget::Content(content) => {
            provider
                .search(content.kind, query, &content.instance, offset, limit)
                .await
        }
        DiscoveryTarget::Modpacks => provider.search_modpacks(query, offset, limit).await,
    })
}

pub(crate) fn spawn_provider_search(
    query: String,
    target: DiscoveryTarget,
    meta_dir: PathBuf,
    request: DiscoveryRequest,
) {
    let DiscoveryRequest {
        generation,
        offset,
        limit,
        pending,
        stream,
        reconcile,
        loaded_icon_stems,
        known_projects,
    } = request;
    tokio::spawn(async move {
        let client = crate::net::HttpClient::new();
        let registry =
            crate::instance::content::provider::ProviderRegistry::configured(client.clone());
        let (modrinth_result, curseforge_result) = tokio::join!(
            search_provider(
                registry.get("modrinth"),
                crate::config::SETTINGS
                    .read()
                    .content
                    .discovery_provider_enabled("modrinth"),
                &target,
                &query,
                offset,
                limit,
            ),
            search_provider(
                registry.get("curseforge"),
                crate::config::SETTINGS
                    .read()
                    .content
                    .discovery_provider_enabled("curseforge"),
                &target,
                &query,
                offset,
                limit,
            )
        );
        let failure = match (&modrinth_result, &curseforge_result) {
            (Some(Err(error)), Some(Err(_))) | (Some(Err(error)), None) => {
                Some((error.to_string(), error.is_retryable()))
            }
            (None, Some(Err(error))) => Some((error.to_string(), error.is_retryable())),
            (None, None) => Some(("No discovery provider is available".to_owned(), false)),
            _ => None,
        };
        let mut merged_sources = Vec::new();
        let result = if let Some((message, retryable)) = failure {
            Err(DiscoveryPageError { message, retryable })
        } else {
            let mut pages = Vec::new();
            if let Some(Ok(results)) = modrinth_result {
                pages.push(("modrinth", results));
            }
            if let Some(Ok(results)) = curseforge_result {
                pages.push(("curseforge", results));
            }
            let mut merged = merge_provider_results(
                pages,
                crate::config::SETTINGS.read().content.preferred_provider(),
                known_projects,
            );
            refresh_source_installed_versions(&mut merged.sources, &target);
            let mut returned = std::collections::HashSet::new();
            let icon_slots = Arc::new(tokio::sync::Semaphore::new(8));
            for merged_project in merged.projects {
                let stem = merged_project.stem;
                let provider = merged_project.provider;
                let mut project = merged_project.project;
                returned.insert(stem.clone());
                let project_id = project.id.clone();
                let installed_path = match &target {
                    DiscoveryTarget::Content(content) => merged
                        .sources
                        .iter()
                        .filter(|(source_stem, _)| source_stem == &stem)
                        .find_map(|(_, source)| {
                            content.manifest.as_ref().and_then(|manifest| {
                                manifest.resolved_project_path(
                                    &source.provider,
                                    &source.project_id,
                                    &content.minecraft_dir,
                                )
                            })
                        }),
                    DiscoveryTarget::Modpacks => None,
                };
                let cached_icon = crate::storage::MetadataPaths::new(&meta_dir)
                    .provider_icons(&provider)
                    .join(format!("{project_id}.img"));
                if !loaded_icon_stems.contains(&stem)
                    && let Ok(bytes) = tokio::fs::read(&cached_icon).await
                    && !bytes.is_empty()
                {
                    project.icon_bytes = Some(bytes);
                }
                let icon_url = (!loaded_icon_stems.contains(&stem) && project.icon_bytes.is_none())
                    .then(|| project.icon_url.clone())
                    .flatten();
                let entry = provider_project_entry(project, &provider, stem, installed_path);
                let icon = icon_url.map(|url| (url, entry.file_stem.clone(), entry.path.clone()));
                if !stream.upsert(entry) {
                    break;
                }
                if let Some((url, file_stem, path)) = icon {
                    let client = client.clone();
                    let stream = stream.clone();
                    let icon_slots = icon_slots.clone();
                    tokio::spawn(async move {
                        let Ok(_permit) = icon_slots.acquire_owned().await else {
                            return;
                        };
                        match client
                            .get_bytes_limited(&url, crate::net::MAX_PROVIDER_ASSET_BYTES)
                            .await
                        {
                            Ok(bytes) if !bytes.is_empty() => {
                                if let Some(parent) = cached_icon.parent() {
                                    let _ = tokio::fs::create_dir_all(parent).await;
                                }
                                let _ = tokio::fs::write(cached_icon, &bytes).await;
                                stream.send_icon(file_stem, path, bytes);
                            }
                            _ => {
                                stream.send_icon_unavailable(file_stem, path);
                            }
                        }
                    });
                }
            }
            if reconcile {
                stream.retain(returned);
            }
            let received = merged.received;
            let total_hits = merged.total_hits;
            merged_sources = merged.sources;
            Ok(DiscoveryPageResult {
                received,
                total_hits,
            })
        };
        DiscoveryState::push_provider_result(&pending, generation, offset, result, merged_sources);
    });
}

pub(crate) fn spawn_project_page(request: ProjectPageRequest) {
    tokio::spawn(async move {
        let client = crate::net::HttpClient::new();
        let mut image_urls = request.image_urls;
        if request.cached_project.is_none() {
            let progress = crate::feedback::progress::ProgressTask::start(format!(
                "Loading {} from {}",
                request.project_title, request.provider
            ));
            let registry =
                crate::instance::content::provider::ProviderRegistry::configured(client.clone());
            match registry.get(&request.provider) {
                Some(provider) => match provider.project(&request.project_id).await {
                    Ok(project) => {
                        image_urls = crate::tui::widgets::markdown::image_urls(
                            &project.title,
                            &project.body,
                        );
                        DiscoveryState::push_action_result(
                            &request.pending,
                            DiscoveryActionResult::ProjectPage {
                                request_id: request.request_id,
                                project_id: request.project_id.clone(),
                                result: Ok(project),
                            },
                        );
                        progress.finish();
                    }
                    Err(error) => {
                        progress.fail(&error);
                        DiscoveryState::push_action_result(
                            &request.pending,
                            DiscoveryActionResult::ProjectPage {
                                request_id: request.request_id,
                                project_id: request.project_id,
                                result: Err(error.to_string()),
                            },
                        );
                        return;
                    }
                },
                None => {
                    let error = format!("{} content provider is unavailable", request.provider);
                    progress.fail(&error);
                    DiscoveryState::push_action_result(
                        &request.pending,
                        DiscoveryActionResult::ProjectPage {
                            request_id: request.request_id,
                            project_id: request.project_id,
                            result: Err(error),
                        },
                    );
                    return;
                }
            }
        }
        image_urls.sort();
        image_urls.dedup();
        let semaphore = Arc::new(tokio::sync::Semaphore::new(4));
        let mut tasks = tokio::task::JoinSet::new();
        for url in image_urls {
            let client = client.clone();
            let semaphore = semaphore.clone();
            tasks.spawn(async move {
                let result = async {
                    let _permit = semaphore
                        .acquire_owned()
                        .await
                        .map_err(|error| error.to_string())?;
                    let bytes = client
                        .get_bytes_limited(&url, crate::net::MAX_PROVIDER_ASSET_BYTES)
                        .await
                        .map_err(|error| error.to_string())?;
                    tokio::task::spawn_blocking(move || {
                        crate::tui::widgets::markdown::decode_image(&bytes)
                    })
                    .await
                    .map_err(|error| error.to_string())?
                }
                .await;
                (url, result)
            });
        }
        while let Some(task) = tasks.join_next().await {
            let (url, result) = match task {
                Ok(result) => result,
                Err(error) => {
                    tracing::debug!("Project image task failed: {error}");
                    continue;
                }
            };
            DiscoveryState::push_action_result(
                &request.pending,
                DiscoveryActionResult::ProjectImage {
                    request_id: request.request_id,
                    project_id: request.project_id.clone(),
                    url,
                    result,
                },
            );
        }
    });
}

pub(crate) type PendingDiscovery = Arc<Mutex<Vec<PendingDiscoveryResult>>>;

pub struct VersionPopupState {
    request_id: u64,
    pub project_id: String,
    pub provider: String,
    pub project_title: String,
    pub sources: Vec<crate::instance::ProviderProject>,
    pub source_index: usize,
    pub installed_path: Option<PathBuf>,
    pub current_version_id: Option<String>,
    pub minecraft_versions: Vec<String>,
    pub selected_minecraft_version: Option<String>,
    pub selecting_minecraft_version: bool,
    pub selecting_world: bool,
    pub worlds: ContentListState,
    pub target_world: Option<(String, PathBuf)>,
    pub versions: Vec<VersionInfo>,
    pub selected: usize,
    pub loading: bool,
    pub resolving_dependencies: bool,
    pub confirming: bool,
    pub installing: bool,
    pub dependency_plan: Option<crate::instance::content::dependencies::DependencyPlan>,
    pub error: Option<String>,
}

impl VersionPopupState {
    pub fn title(&self) -> String {
        let installed = self.installed_path.is_some() || self.current_version_id.is_some();
        if installed
            && self.current_version_id.as_deref().is_some_and(|current| {
                self.selected_version()
                    .is_some_and(|version| version.id == current)
            })
        {
            format!("Reinstall {}", self.project_title)
        } else if installed {
            format!("Change {} version", self.project_title)
        } else {
            format!("Install {}", self.project_title)
        }
    }

    pub fn provider_label(&self) -> &str {
        match self.provider.as_str() {
            "curseforge" => "CurseForge",
            _ => "Modrinth",
        }
    }

    pub fn visible_versions(&self) -> impl Iterator<Item = &VersionInfo> {
        self.versions.iter().filter(|version| {
            self.selected_minecraft_version
                .as_ref()
                .is_none_or(|selected| {
                    version
                        .game_versions
                        .iter()
                        .any(|version| version == selected)
                })
        })
    }

    pub fn selected_version(&self) -> Option<&VersionInfo> {
        self.visible_versions().nth(self.selected)
    }

    fn item_count(&self) -> usize {
        if self.selecting_minecraft_version {
            self.minecraft_versions.len()
        } else if self.selecting_world {
            self.worlds.filtered_indices().len()
        } else {
            self.visible_versions().count()
        }
    }
}

pub struct VersionsRequest {
    pub request_id: u64,
    pub project_id: String,
    pub provider: String,
    pub current_version_id: Option<String>,
    pub pending: PendingActions,
}

pub struct ProjectPageRequest {
    pub request_id: u64,
    pub project_id: String,
    pub project_title: String,
    pub provider: String,
    pub cached_project: Option<crate::net::modrinth::ProjectInfo>,
    pub image_urls: Vec<String>,
    pub pending: PendingActions,
}

pub struct ProjectPageState {
    request_id: u64,
    project_id: String,
    pub title: String,
    pub document: Option<crate::tui::widgets::markdown::Document>,
    pub error: Option<String>,
    pub scroll: usize,
    pub max_scroll: usize,
}

pub struct InstallRequest {
    pub request_id: u64,
    pub generation: u64,
    pub project_id: String,
    pub project_title: String,
    pub provider: String,
    pub version: VersionInfo,
    pub installed_path: Option<PathBuf>,
    pub dependency_plan: Option<crate::instance::content::dependencies::DependencyPlan>,
    pub target_world: Option<(String, PathBuf)>,
    pub pending: PendingActions,
}

pub struct DependencyRequest {
    pub request_id: u64,
    pub project_id: String,
    pub root: crate::instance::content::dependencies::InstallRoot,
    pub pending: PendingActions,
}

pub struct InstallCompletion {
    pub path: PathBuf,
    pub replaced: bool,
    pub skipped: bool,
    pub orphaned_dependencies: Vec<PathBuf>,
}

pub enum DiscoveryActionResult {
    ProjectPage {
        request_id: u64,
        project_id: String,
        result: Result<crate::net::modrinth::ProjectInfo, String>,
    },
    ProjectImage {
        request_id: u64,
        project_id: String,
        url: String,
        result: Result<image::DynamicImage, String>,
    },
    Versions {
        request_id: u64,
        project_id: String,
        result: Result<Vec<VersionInfo>, String>,
    },
    Dependencies {
        request_id: u64,
        project_id: String,
        result: Result<crate::instance::content::dependencies::DependencyPlan, String>,
    },
    Install {
        request_id: u64,
        generation: u64,
        project_id: String,
        project_title: String,
        result: Result<InstallCompletion, String>,
    },
}

pub(crate) type PendingActions = Arc<Mutex<Vec<DiscoveryActionResult>>>;

pub struct DiscoveryState {
    pub kind: ContentKind,
    pub modpacks: bool,
    pub list: ContentListState,
    pub search: crate::tui::widgets::search::SearchState,
    pub total_hits: usize,
    pub error: Option<String>,
    context: Option<String>,
    generation: u64,
    pending: PendingDiscovery,
    pending_actions: PendingActions,
    project_pages: std::collections::HashMap<String, crate::net::modrinth::ProjectInfo>,
    project_images: std::collections::HashMap<(String, String), image::DynamicImage>,
    sources: std::collections::HashMap<String, Vec<crate::instance::ProviderProject>>,
    pub project_page: Option<ProjectPageState>,
    pub version_popup: Option<VersionPopupState>,
    pending_orphan_cleanup: Option<Vec<PathBuf>>,
    next_action_request_id: u64,
    stream: Option<ContentStream>,
    next_offset: usize,
    page_loading: bool,
    exhausted: bool,
    viewport_rows: u16,
    search_changed_at: Option<std::time::Instant>,
    retry_page_at: Option<std::time::Instant>,
    page_retry_attempt: u32,
}

impl DiscoveryState {
    pub fn new(kind: ContentKind) -> Self {
        Self {
            kind,
            modpacks: false,
            list: ContentListState::default(),
            search: crate::tui::widgets::search::SearchState::default(),
            total_hits: 0,
            error: None,
            context: None,
            generation: 0,
            pending: Arc::new(Mutex::new(Vec::new())),
            pending_actions: Arc::new(Mutex::new(Vec::new())),
            project_pages: std::collections::HashMap::new(),
            project_images: std::collections::HashMap::new(),
            sources: std::collections::HashMap::new(),
            project_page: None,
            version_popup: None,
            pending_orphan_cleanup: None,
            next_action_request_id: 0,
            stream: None,
            next_offset: 0,
            page_loading: false,
            exhausted: false,
            viewport_rows: 0,
            search_changed_at: None,
            retry_page_at: None,
            page_retry_attempt: 0,
        }
    }

    pub fn new_modpacks() -> Self {
        let mut state = Self::new(ContentKind::ResourcePack);
        state.modpacks = true;
        state
    }

    pub fn needs_search(&self, instance: &InstanceConfig) -> bool {
        self.context.as_deref() != Some(discovery_context(instance).as_str())
    }

    pub fn unavailable_message(&self, instance: &InstanceConfig) -> Option<&'static str> {
        self.kind.unavailable_message(instance.loader)
    }

    pub fn set_unavailable(&mut self, instance: &InstanceConfig) {
        let context = discovery_context(instance);
        if self.context.as_deref() == Some(&context) && self.list.entries.is_empty() {
            return;
        }
        self.context = None;
        drop(self.begin_search(instance));
        self.stream = None;
        self.page_loading = false;
        self.exhausted = true;
    }

    pub fn begin_search(&mut self, instance: &InstanceConfig) -> DiscoveryRequest {
        self.begin_search_context(discovery_context(instance))
    }

    pub fn begin_modpack_search(&mut self) -> DiscoveryRequest {
        self.begin_search_context("modpacks".to_owned())
    }

    fn begin_search_context(&mut self, context: String) -> DiscoveryRequest {
        self.generation = self.generation.wrapping_add(1);
        self.project_page = None;
        self.version_popup = None;
        self.sources.clear();
        let reconcile =
            self.context.as_deref() == Some(context.as_str()) && !self.list.entries.is_empty();
        self.context = Some(context.clone());
        self.total_hits = 0;
        self.error = None;
        self.next_offset = 0;
        self.page_loading = true;
        self.exhausted = false;
        self.search_changed_at = None;
        self.retry_page_at = None;
        self.page_retry_attempt = 0;
        let loaded_icon_stems = if reconcile {
            self.list
                .entries
                .iter()
                .filter(|entry| entry.icon_bytes.is_some())
                .map(|entry| entry.file_stem.clone())
                .collect()
        } else {
            std::collections::HashSet::new()
        };
        let stream = if reconcile {
            self.list.refresh_source_stream(context)
        } else {
            self.list.start_source_stream(context)
        };
        self.list.search.query.clone_from(&self.search.query);
        self.list.set_search_filtering(false);
        self.stream = Some(stream.clone());
        DiscoveryRequest {
            generation: self.generation,
            offset: 0,
            limit: self.request_limit(),
            pending: self.pending.clone(),
            stream,
            reconcile,
            loaded_icon_stems,
            known_projects: std::collections::HashMap::new(),
        }
    }

    pub fn begin_next_page(&mut self) -> Option<DiscoveryRequest> {
        if !self.should_load_more() {
            return None;
        }
        self.page_loading = true;
        self.retry_page_at = None;
        let loaded_icon_stems = self
            .list
            .entries
            .iter()
            .filter(|entry| entry.icon_bytes.is_some())
            .map(|entry| entry.file_stem.clone())
            .collect();
        let known_projects = self
            .list
            .entries
            .iter()
            .filter_map(|entry| {
                let source = entry.provider_project.as_ref()?;
                let slug = entry.source_slug.as_deref()?;
                Some((
                    project_identity_parts(&entry.name, slug),
                    (entry.file_stem.clone(), source.provider.clone()),
                ))
            })
            .collect();
        Some(DiscoveryRequest {
            generation: self.generation,
            offset: self.next_offset,
            limit: self.request_limit(),
            pending: self.pending.clone(),
            stream: self.stream.clone()?,
            reconcile: false,
            loaded_icon_stems,
            known_projects,
        })
    }

    pub fn set_viewport_rows(&mut self, rows: u16) {
        self.viewport_rows = rows;
    }

    fn request_limit(&self) -> usize {
        let viewport_items = usize::from(self.viewport_rows / 3);
        if viewport_items == 0 {
            PAGE_SIZE
        } else {
            viewport_items.saturating_mul(4).min(PAGE_SIZE)
        }
    }

    pub fn begin_versions(&mut self) -> Option<VersionsRequest> {
        let filtered = self.list.filtered_indices();
        let index = self
            .list
            .list_state
            .selected
            .and_then(|selected| filtered.get(selected))?;
        let entry = self.list.entries.get(*index)?.clone();
        let installed_path = (self.kind != ContentKind::DataPack)
            .then(|| entry.installed_path.clone())
            .flatten();
        let sources = self
            .sources
            .get(&entry.file_stem)
            .cloned()
            .unwrap_or_else(|| entry.provider_project.clone().into_iter().collect());
        self.open_version_popup(&entry.name, sources, installed_path, None)
    }

    pub fn begin_installed_versions(
        &mut self,
        entry: &ContentEntry,
        record: &crate::instance::ContentFileRecord,
        target_world: Option<(String, PathBuf)>,
    ) -> Option<VersionsRequest> {
        let current = record.resolved_project()?.clone();
        let mut sources = vec![current.clone()];
        for alias in &record.provider_aliases {
            if !sources.iter().any(|source| {
                source.provider == alias.provider && source.project_id == alias.project_id
            }) {
                sources.push(alias.clone());
            }
        }
        if let Some(discovered) = self.sources.values().find(|discovered| {
            discovered.iter().any(|candidate| {
                sources.iter().any(|installed| {
                    candidate.provider == installed.provider
                        && candidate.project_id == installed.project_id
                })
            })
        }) {
            for source in discovered {
                if !sources.iter().any(|installed| {
                    source.provider == installed.provider
                        && source.project_id == installed.project_id
                }) {
                    sources.push(source.clone());
                }
            }
        }
        self.open_version_popup(&entry.name, sources, Some(entry.path.clone()), target_world)
    }

    pub fn begin_managed_modpack_versions(
        &mut self,
        project_title: &str,
        source: crate::instance::ProviderProject,
    ) -> Option<VersionsRequest> {
        let request = self.open_version_popup(project_title, vec![source], None, None)?;
        self.version_popup.as_mut()?.selecting_minecraft_version = false;
        Some(request)
    }

    fn open_version_popup(
        &mut self,
        project_title: &str,
        mut sources: Vec<crate::instance::ProviderProject>,
        installed_path: Option<PathBuf>,
        target_world: Option<(String, PathBuf)>,
    ) -> Option<VersionsRequest> {
        let preferred = crate::config::SETTINGS
            .read()
            .content
            .preferred_provider()
            .to_owned();
        sources.sort_by_key(|source| source.provider != preferred);
        let source = sources.first()?.clone();
        self.next_action_request_id = self.next_action_request_id.wrapping_add(1);
        let request_id = self.next_action_request_id;
        let current_version_id = (!source.version_id.is_empty()).then(|| source.version_id.clone());
        self.version_popup = Some(VersionPopupState {
            request_id,
            project_id: source.project_id.clone(),
            provider: source.provider.clone(),
            project_title: project_title.to_owned(),
            sources,
            source_index: 0,
            installed_path,
            current_version_id: current_version_id.clone(),
            minecraft_versions: Vec::new(),
            selected_minecraft_version: None,
            selecting_minecraft_version: self.modpacks,
            selecting_world: false,
            worlds: ContentListState::default(),
            target_world,
            versions: Vec::new(),
            selected: 0,
            loading: true,
            resolving_dependencies: false,
            confirming: false,
            installing: false,
            dependency_plan: None,
            error: None,
        });
        Some(VersionsRequest {
            request_id,
            project_id: source.project_id,
            provider: source.provider,
            current_version_id,
            pending: self.pending_actions.clone(),
        })
    }

    pub fn switch_version_source(&mut self) -> Option<VersionsRequest> {
        let selecting_minecraft_version = self.modpacks;
        let popup = self.version_popup.as_mut()?;
        if popup.loading || popup.installing || popup.selecting_world || popup.sources.len() < 2 {
            return None;
        }
        popup.source_index = (popup.source_index + 1) % popup.sources.len();
        let source = popup.sources.get(popup.source_index)?.clone();
        self.next_action_request_id = self.next_action_request_id.wrapping_add(1);
        popup.request_id = self.next_action_request_id;
        popup.project_id.clone_from(&source.project_id);
        popup.provider.clone_from(&source.provider);
        popup.minecraft_versions.clear();
        popup.selected_minecraft_version = None;
        popup.selecting_minecraft_version = selecting_minecraft_version;
        popup.selecting_world = false;
        popup.worlds = ContentListState::default();
        if self.kind != ContentKind::DataPack || popup.current_version_id.is_none() {
            popup.target_world = None;
        }
        popup.current_version_id =
            (!source.version_id.is_empty()).then(|| source.version_id.clone());
        popup.versions.clear();
        popup.selected = 0;
        popup.loading = true;
        popup.resolving_dependencies = false;
        popup.confirming = false;
        popup.dependency_plan = None;
        popup.error = None;
        Some(VersionsRequest {
            request_id: popup.request_id,
            project_id: source.project_id,
            provider: source.provider,
            current_version_id: popup.current_version_id.clone(),
            pending: self.pending_actions.clone(),
        })
    }

    pub fn begin_project_page(&mut self) -> Option<ProjectPageRequest> {
        let filtered = self.list.filtered_indices();
        let index = self
            .list
            .list_state
            .selected
            .and_then(|selected| filtered.get(selected))?;
        let entry = self.list.entries.get(*index)?;
        let source = entry.provider_project.as_ref()?;
        let project_id = source.project_id.clone();
        let project_title = entry.name.clone();
        self.next_action_request_id = self.next_action_request_id.wrapping_add(1);
        let request_id = self.next_action_request_id;
        let cached = self.project_pages.get(&project_id);
        let mut document = cached.map(|project| {
            crate::tui::widgets::markdown::Document::new(&project.title, &project.body)
        });
        if let Some(document) = document.as_mut() {
            for url in document.image_urls() {
                if let Some(image) = self.project_images.get(&(project_id.clone(), url.clone())) {
                    document.set_image(&url, Ok(image.clone()));
                }
            }
        }
        let image_urls = document
            .as_ref()
            .map(crate::tui::widgets::markdown::Document::image_urls)
            .unwrap_or_default()
            .into_iter()
            .filter(|url| {
                !self
                    .project_images
                    .contains_key(&(project_id.clone(), url.clone()))
            })
            .collect::<Vec<_>>();
        self.project_page = Some(ProjectPageState {
            request_id,
            project_id: project_id.clone(),
            title: cached
                .map(|project| project.title.clone())
                .unwrap_or_else(|| project_title.clone()),
            document,
            error: None,
            scroll: 0,
            max_scroll: 0,
        });
        if cached.is_some() && image_urls.is_empty() {
            return None;
        }
        Some(ProjectPageRequest {
            request_id,
            project_id,
            project_title,
            provider: source.provider.clone(),
            cached_project: cached.cloned(),
            image_urls,
            pending: self.pending_actions.clone(),
        })
    }

    pub fn project_page_open(&self) -> bool {
        self.project_page.is_some()
    }

    pub fn project_link_at(&self, x: u16, y: u16) -> Option<&str> {
        self.project_page.as_ref()?.document.as_ref()?.link_at(x, y)
    }

    pub fn refresh_installed_manifest(
        &mut self,
        manifest: &crate::instance::ContentManifest,
        minecraft_dir: &std::path::Path,
    ) {
        let installed_identity = |source: &crate::instance::ProviderProject| {
            manifest.files.iter().find_map(|record| {
                record
                    .project_for_provider(&source.provider, &source.project_id)
                    .cloned()
            })
        };
        for sources in self.sources.values_mut() {
            for source in sources {
                source.version_id = installed_identity(source)
                    .map(|installed| installed.version_id)
                    .unwrap_or_default();
            }
        }
        let mut changed = false;
        for entry in &mut self.list.entries {
            if let Some(source) = entry.provider_project.as_mut() {
                source.version_id = installed_identity(source)
                    .map(|installed| installed.version_id)
                    .unwrap_or_default();
            }
            let installed_path = self
                .sources
                .get(&entry.file_stem)
                .into_iter()
                .flatten()
                .chain(entry.provider_project.iter())
                .find_map(|project| {
                    manifest.resolved_project_path(
                        &project.provider,
                        &project.project_id,
                        minecraft_dir,
                    )
                });
            if entry.installed_path != installed_path {
                entry.title_suffix = installed_path.is_some().then(|| "Installed".to_owned());
                entry.installed_path = installed_path;
                changed = true;
            }
        }
        if changed {
            crate::feedback::request_redraw();
        }
    }

    pub fn selected_is_installed(&self) -> bool {
        self.selected_installed_entry().is_some()
    }

    pub fn pending_installed_delete(
        &self,
    ) -> Option<crate::tui::widgets::content::list::PendingContentDelete> {
        let entry = self.selected_installed_entry()?;
        Some(crate::tui::widgets::content::list::PendingContentDelete {
            name: entry.name.clone(),
            path: entry.installed_path.clone()?,
        })
    }

    pub fn clear_installed_path(&mut self, path: &std::path::Path) -> bool {
        let Some(entry) = self
            .list
            .entries
            .iter_mut()
            .find(|entry| entry.installed_path.as_deref() == Some(path))
        else {
            return false;
        };
        entry.installed_path = None;
        entry.title_suffix = None;
        crate::feedback::request_redraw();
        true
    }

    fn selected_installed_entry(&self) -> Option<&ContentEntry> {
        let filtered = self.list.filtered_indices();
        let index = self
            .list
            .list_state
            .selected
            .and_then(|selected| filtered.get(selected))?;
        self.list
            .entries
            .get(*index)
            .filter(|entry| entry.installed_path.is_some())
    }

    pub fn begin_install(&mut self) -> Option<InstallRequest> {
        let popup = self.version_popup.as_ref()?;
        if popup.loading || popup.installing || !popup.confirming {
            return None;
        }
        let version = popup.selected_version()?.clone();
        let request = InstallRequest {
            request_id: popup.request_id,
            generation: self.generation,
            project_id: popup.project_id.clone(),
            project_title: popup.project_title.clone(),
            provider: popup.provider.clone(),
            version,
            installed_path: popup.installed_path.clone(),
            dependency_plan: popup.dependency_plan.clone(),
            target_world: popup.target_world.clone(),
            pending: self.pending_actions.clone(),
        };
        self.version_popup = None;
        Some(request)
    }

    pub fn begin_confirmation(&mut self) -> bool {
        let Some(popup) = self.version_popup.as_mut() else {
            return false;
        };
        if popup.loading
            || popup.installing
            || popup.selecting_minecraft_version
            || popup.selecting_world
            || popup.selected_version().is_none()
        {
            return false;
        }
        popup.confirming = true;
        popup.error = None;
        true
    }

    pub fn begin_world_selection(&mut self, mut worlds: Vec<ContentEntry>) -> bool {
        let Some(popup) = self.version_popup.as_mut() else {
            return false;
        };
        if self.kind != ContentKind::DataPack
            || popup.loading
            || popup.installing
            || popup.confirming
            || popup.selected_version().is_none()
        {
            return false;
        }
        for world in &mut worlds {
            world.icon_lines = world
                .icon_bytes
                .as_ref()
                .and_then(|bytes| crate::instance::content::make_icon_pixels(bytes, 6, 3))
                .or_else(|| Some(crate::instance::content::fallback_icon()));
        }
        popup.worlds = ContentListState::default();
        popup.worlds.entries = worlds;
        popup
            .worlds
            .list_state
            .select((!popup.worlds.entries.is_empty()).then_some(0));
        popup.selecting_world = true;
        popup.target_world = None;
        popup.installed_path = None;
        popup.dependency_plan = None;
        popup.error = None;
        true
    }

    pub fn select_world(
        &mut self,
        manifest: Option<&crate::instance::ContentManifest>,
        minecraft_dir: &std::path::Path,
    ) -> bool {
        let Some(popup) = self.version_popup.as_mut() else {
            return false;
        };
        if !popup.selecting_world || popup.loading || popup.installing {
            return false;
        }
        let Some(world) = popup.worlds.selected_entry() else {
            return false;
        };
        let world_name = world.name.clone();
        let world_path = world.path.clone();
        popup.installed_path = manifest.and_then(|manifest| {
            popup.sources.iter().find_map(|source| {
                manifest.resolved_project_path_under(
                    &source.provider,
                    &source.project_id,
                    minecraft_dir,
                    &world_path.join("datapacks"),
                )
            })
        });
        popup.target_world = Some((world_name, world_path));
        popup.selecting_world = false;
        popup.error = None;
        true
    }

    pub fn begin_dependency_resolution(&mut self) -> Option<DependencyRequest> {
        let kind = self.kind;
        let popup = self.version_popup.as_mut()?;
        if popup.loading
            || popup.installing
            || popup.selecting_minecraft_version
            || popup.confirming
        {
            return None;
        }
        let version = popup.selected_version()?.clone();
        let force_reinstall = popup.current_version_id.as_deref() == Some(version.id.as_str());
        popup.loading = true;
        popup.resolving_dependencies = true;
        popup.error = None;
        Some(DependencyRequest {
            request_id: popup.request_id,
            project_id: popup.project_id.clone(),
            root: crate::instance::content::dependencies::InstallRoot {
                provider: popup.provider.clone(),
                project_id: popup.project_id.clone(),
                title: popup.project_title.clone(),
                version,
                installed_path: popup.installed_path.clone(),
                kind,
                target_world: popup.target_world.as_ref().map(|(_, path)| path.clone()),
                force_reinstall,
            },
            pending: self.pending_actions.clone(),
        })
    }

    pub fn select_minecraft_version(&mut self) -> bool {
        let Some(popup) = self.version_popup.as_mut() else {
            return false;
        };
        if popup.loading || popup.installing || !popup.selecting_minecraft_version {
            return false;
        }
        let Some(version) = popup.minecraft_versions.get(popup.selected).cloned() else {
            return false;
        };
        popup.selected_minecraft_version = Some(version);
        popup.selecting_minecraft_version = false;
        popup.selected = 0;
        popup.error = None;
        true
    }

    pub fn search_due(&self) -> bool {
        self.search_changed_at
            .is_some_and(|changed| changed.elapsed() >= SEARCH_DEBOUNCE)
    }

    fn search_changed(&mut self) {
        self.list.search.query.clone_from(&self.search.query);
        self.list.set_search_filtering(false);
        self.search_changed_at = Some(std::time::Instant::now());
    }

    pub fn push_result(
        pending: &PendingDiscovery,
        generation: u64,
        offset: usize,
        result: Result<DiscoveryPageResult, DiscoveryPageError>,
    ) {
        if let Ok(mut pending) = pending.lock() {
            pending.push(PendingDiscoveryResult {
                generation,
                offset,
                result,
                sources: Vec::new(),
            });
            crate::feedback::request_redraw();
        }
    }

    pub fn push_provider_result(
        pending: &PendingDiscovery,
        generation: u64,
        offset: usize,
        result: Result<DiscoveryPageResult, DiscoveryPageError>,
        sources: Vec<(String, crate::instance::ProviderProject)>,
    ) {
        if let Ok(mut pending) = pending.lock() {
            pending.push(PendingDiscoveryResult {
                generation,
                offset,
                result,
                sources,
            });
            crate::feedback::request_redraw();
        }
    }

    pub fn push_action_result(pending: &PendingActions, result: DiscoveryActionResult) {
        if let Ok(mut pending) = pending.lock() {
            pending.push(result);
            crate::feedback::request_redraw();
        }
    }

    pub fn drain_pending(&mut self) {
        let results = match self.pending.lock() {
            Ok(mut pending) => std::mem::take(&mut *pending),
            Err(_) => return,
        };
        for pending in results {
            if pending.generation != self.generation {
                continue;
            }
            self.page_loading = false;
            self.list.loading = false;
            match pending.result {
                Ok(result) => {
                    self.total_hits = result.total_hits;
                    self.next_offset = pending.offset.saturating_add(result.received);
                    self.exhausted = result.received == 0 || self.next_offset >= self.total_hits;
                    self.error = None;
                    self.retry_page_at = None;
                    self.page_retry_attempt = 0;
                    for (stem, source) in pending.sources {
                        let sources = self.sources.entry(stem).or_default();
                        if !sources.iter().any(|candidate| {
                            candidate.provider == source.provider
                                && candidate.project_id == source.project_id
                        }) {
                            sources.push(source);
                        }
                    }
                }
                Err(error) => {
                    if error.retryable {
                        let multiplier = 1u32 << self.page_retry_attempt.min(4);
                        let delay = PAGE_RETRY_BASE_DELAY
                            .saturating_mul(multiplier)
                            .min(PAGE_RETRY_MAX_DELAY);
                        self.page_retry_attempt = self.page_retry_attempt.saturating_add(1);
                        self.retry_page_at = Some(std::time::Instant::now() + delay);
                        tracing::debug!(
                            "Discovery page at offset {} failed; retrying in {:?}: {}",
                            pending.offset,
                            delay,
                            error.message
                        );
                    } else if pending.offset == 0 {
                        self.total_hits = 0;
                        self.error = Some(error.message);
                        self.exhausted = true;
                    } else {
                        tracing::warn!(
                            "Discovery page at offset {} failed: {}",
                            pending.offset,
                            error.message
                        );
                        self.exhausted = true;
                    }
                }
            }
        }

        self.drain_action_results();
    }

    fn drain_action_results(&mut self) {
        let results = match self.pending_actions.lock() {
            Ok(mut pending) => std::mem::take(&mut *pending),
            Err(_) => return,
        };
        for result in results {
            match result {
                DiscoveryActionResult::ProjectPage {
                    request_id,
                    project_id,
                    result,
                } => {
                    let Some(page) = self.project_page.as_mut().filter(|page| {
                        page.request_id == request_id && page.project_id == project_id
                    }) else {
                        continue;
                    };
                    match result {
                        Ok(project) => {
                            page.title.clone_from(&project.title);
                            page.document = Some(crate::tui::widgets::markdown::Document::new(
                                &project.title,
                                &project.body,
                            ));
                            if let Some(document) = page.document.as_mut() {
                                for url in document.image_urls() {
                                    if let Some(image) =
                                        self.project_images.get(&(project_id.clone(), url.clone()))
                                    {
                                        document.set_image(&url, Ok(image.clone()));
                                    }
                                }
                            }
                            page.error = None;
                            page.scroll = 0;
                            page.max_scroll = 0;
                            self.project_pages.insert(project.id.clone(), project);
                        }
                        Err(error) => page.error = Some(error),
                    }
                }
                DiscoveryActionResult::ProjectImage {
                    request_id,
                    project_id,
                    url,
                    result,
                } => {
                    if let Ok(image) = &result {
                        self.project_images
                            .insert((project_id.clone(), url.clone()), image.clone());
                    }
                    let Some(page) = self.project_page.as_mut().filter(|page| {
                        page.request_id == request_id && page.project_id == project_id
                    }) else {
                        continue;
                    };
                    if let Some(document) = page.document.as_mut() {
                        document.set_image(&url, result);
                    }
                }
                DiscoveryActionResult::Versions {
                    request_id,
                    project_id,
                    result,
                } => {
                    let Some(popup) = self.version_popup.as_mut().filter(|popup| {
                        popup.request_id == request_id && popup.project_id == project_id
                    }) else {
                        continue;
                    };
                    popup.loading = false;
                    match result {
                        Ok(versions) => {
                            popup.minecraft_versions = minecraft_versions(&versions);
                            popup.versions = versions;
                            popup.selected = 0;
                            popup.error = None;
                        }
                        Err(error) => popup.error = Some(error),
                    }
                }
                DiscoveryActionResult::Dependencies {
                    request_id,
                    project_id,
                    result,
                } => {
                    let Some(popup) = self.version_popup.as_mut().filter(|popup| {
                        popup.request_id == request_id && popup.project_id == project_id
                    }) else {
                        continue;
                    };
                    popup.loading = false;
                    popup.resolving_dependencies = false;
                    match result {
                        Ok(plan) => {
                            popup.dependency_plan = Some(plan);
                            popup.confirming = true;
                            popup.error = None;
                        }
                        Err(error) => popup.error = Some(error),
                    }
                }
                DiscoveryActionResult::Install {
                    request_id,
                    generation,
                    project_id,
                    project_title,
                    result,
                } => match result {
                    Ok(completion) => {
                        if generation == self.generation {
                            let stem = self
                                .sources
                                .iter()
                                .find_map(|(stem, sources)| {
                                    sources
                                        .iter()
                                        .any(|source| source.project_id == project_id)
                                        .then(|| stem.clone())
                                })
                                .unwrap_or_else(|| project_id.clone());
                            if let Some(entry) = self
                                .list
                                .entries
                                .iter_mut()
                                .find(|entry| entry.file_stem == stem)
                            {
                                entry.title_suffix = Some("Installed".to_owned());
                                entry.installed_path = Some(completion.path.clone());
                            }
                        }
                        let action = if completion.skipped {
                            "already installed"
                        } else if completion.replaced {
                            "version changed"
                        } else {
                            "installed"
                        };
                        crate::feedback::errors::push_error(crate::feedback::errors::ErrorEvent {
                            id: request_id,
                            level: tracing::Level::INFO,
                            message: format!("{project_title}: {action}"),
                            pushed_at: std::time::Instant::now(),
                        });
                        if !completion.orphaned_dependencies.is_empty() {
                            self.pending_orphan_cleanup = Some(completion.orphaned_dependencies);
                        }
                    }
                    Err(error) => {
                        crate::feedback::errors::push_error(crate::feedback::errors::ErrorEvent {
                            id: request_id,
                            level: tracing::Level::ERROR,
                            message: format!("{project_title}: {error}"),
                            pushed_at: std::time::Instant::now(),
                        });
                    }
                },
            }
        }
    }

    pub fn empty_text(&self) -> &str {
        self.error.as_deref().unwrap_or(if self.modpacks {
            "No modpacks found."
        } else if self.kind == ContentKind::DataPack {
            "No datapacks found."
        } else {
            "No projects found."
        })
    }

    pub fn take_orphan_cleanup(&mut self) -> Option<Vec<PathBuf>> {
        self.pending_orphan_cleanup.take()
    }

    fn should_load_more(&self) -> bool {
        if self.page_loading
            || self.exhausted
            || self.error.is_some()
            || self.stream.is_none()
            || self.search_changed_at.is_some()
            || self
                .retry_page_at
                .is_some_and(|retry_at| std::time::Instant::now() < retry_at)
        {
            return false;
        }
        let viewport_items = usize::from(self.viewport_rows).div_ceil(3);
        let prefetch_items = viewport_items
            .saturating_mul(PREFETCH_VIEWPORTS)
            .max(MIN_PREFETCH_ITEMS);
        let selected = self.list.list_state.selected.unwrap_or(0);
        self.list.entries.len() < viewport_items.saturating_add(prefetch_items)
            || selected.saturating_add(prefetch_items) >= self.list.entries.len()
    }
}

pub fn handle_key(key_event: &KeyEvent, state: &mut DiscoveryState) -> bool {
    if let Some(popup) = state.version_popup.as_mut() {
        if popup.confirming
            && matches!(key_event.code, KeyCode::Left | KeyCode::Char('h'))
            && !popup.installing
        {
            popup.confirming = false;
            if popup.target_world.is_some() && popup.current_version_id.is_none() {
                popup.selecting_world = true;
            }
            popup.error = None;
            return true;
        }
        if popup.selecting_world
            && matches!(key_event.code, KeyCode::Left | KeyCode::Char('h'))
            && !popup.loading
            && !popup.installing
        {
            popup.selecting_world = false;
            popup.target_world = None;
            popup.installed_path = None;
            popup.dependency_plan = None;
            popup.error = None;
            return true;
        }
        if !popup.selecting_minecraft_version
            && popup.selected_minecraft_version.is_some()
            && matches!(key_event.code, KeyCode::Left | KeyCode::Char('h'))
            && !popup.loading
            && !popup.installing
        {
            popup.selecting_minecraft_version = true;
            popup.selected = popup
                .selected_minecraft_version
                .as_ref()
                .and_then(|selected| {
                    popup
                        .minecraft_versions
                        .iter()
                        .position(|version| version == selected)
                })
                .unwrap_or(0);
            return true;
        }
        match key_event.code {
            KeyCode::Esc if !popup.installing => state.version_popup = None,
            _ if popup.selecting_world => {
                super::list::handle_key_no_toggle(key_event, &mut popup.worlds);
            }
            KeyCode::Char('j') | KeyCode::Down
                if !popup.loading && !popup.installing && !popup.confirming =>
            {
                if popup.selected + 1 < popup.item_count() {
                    popup.selected += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up
                if !popup.loading && !popup.installing && !popup.confirming =>
            {
                popup.selected = popup.selected.saturating_sub(1);
            }
            _ => {}
        }
        return true;
    }
    if let Some(page) = state.project_page.as_mut() {
        match key_event.code {
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') => state.project_page = None,
            KeyCode::Char('j') | KeyCode::Down => {
                page.scroll = page.scroll.saturating_add(1).min(page.max_scroll);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                page.scroll = page.scroll.saturating_sub(1);
            }
            KeyCode::PageDown | KeyCode::Char('d') => {
                page.scroll = page.scroll.saturating_add(10).min(page.max_scroll);
            }
            KeyCode::PageUp | KeyCode::Char('u') => {
                page.scroll = page.scroll.saturating_sub(10);
            }
            KeyCode::Char('g') | KeyCode::Home => page.scroll = 0,
            KeyCode::Char('G') | KeyCode::End => page.scroll = page.max_scroll,
            _ => {}
        }
        return true;
    }
    if state.search.active {
        let previous_query = state.search.query.clone();
        match key_event.code {
            KeyCode::Enter => state.search.confirm(),
            KeyCode::Esc => state.search.deactivate(),
            KeyCode::Backspace => state.search.backspace(key_event.modifiers),
            KeyCode::Char(c) => state.search.push(c),
            _ => {}
        }
        let changed = state.search.query != previous_query;
        if changed {
            state.search_changed();
        }
        return true;
    }
    if key_event.code == KeyCode::Char('/') {
        state.search.activate();
        return true;
    }
    if let Some(next) = page_key_direction(key_event) {
        if next {
            state.list.next_page()
        } else {
            state.list.previous_page()
        }
    } else {
        super::list::handle_key_no_toggle(key_event, &mut state.list)
    }
}

fn minecraft_versions(versions: &[VersionInfo]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut minecraft_versions = versions
        .iter()
        .flat_map(|version| &version.game_versions)
        .filter(|game_version| {
            !versions.iter().any(|version| {
                version
                    .loaders
                    .iter()
                    .any(|loader| loader.eq_ignore_ascii_case(game_version))
            })
        })
        .filter(|game_version| seen.insert((*game_version).clone()))
        .cloned()
        .collect::<Vec<_>>();
    minecraft_versions
        .sort_by(|a, b| crate::tui::widgets::popups::compare_game_versions(b.as_str(), a.as_str()));
    minecraft_versions
}

pub(crate) fn page_key_direction(key_event: &KeyEvent) -> Option<bool> {
    match key_event.code {
        KeyCode::Char('[') => Some(false),
        KeyCode::Char(']') => Some(true),
        _ => None,
    }
}

fn discovery_context(instance: &InstanceConfig) -> String {
    format!(
        "{}:{}:{}",
        instance.name,
        instance.game_version,
        loader_slug(instance.loader).unwrap_or("vanilla")
    )
}

fn loader_slug(loader: ModLoader) -> Option<&'static str> {
    match loader {
        ModLoader::Vanilla => None,
        ModLoader::Fabric => Some("fabric"),
        ModLoader::Forge => Some("forge"),
        ModLoader::NeoForge => Some("neoforge"),
        ModLoader::Quilt => Some("quilt"),
    }
}

pub(crate) fn provider_project_entry(
    project: DiscoveryProject,
    provider: &str,
    stem: String,
    installed_path: Option<PathBuf>,
) -> ContentEntry {
    let provider_icon = project.icon_bytes.is_some()
        || project
            .icon_url
            .as_deref()
            .is_some_and(|url| !url.trim().is_empty());
    ContentEntry {
        file_stem: stem.clone(),
        name: project.title,
        source_slug: Some(project.slug),
        installed_path: installed_path.clone(),
        provider_project: Some(crate::instance::ProviderProject {
            provider: provider.to_owned(),
            project_id: project.id.clone(),
            version_id: String::new(),
        }),
        world_details: None,
        title_suffix: installed_path.is_some().then(|| "Installed".to_owned()),
        footer_label: Some(format!("{} downloads", format_downloads(project.downloads))),
        footer_change: None,
        description: project.description,
        enabled: true,
        icon_bytes: project.icon_bytes,
        provider_icon,
        provider_description: false,
        path: PathBuf::from(stem),
        icon_lines: Some(crate::instance::content::fallback_icon()),
    }
}

#[cfg(test)]
pub(crate) fn project_entry(
    project: DiscoveryProject,
    installed_path: Option<PathBuf>,
) -> ContentEntry {
    let stem = project.id.clone();
    provider_project_entry(project, "modrinth", stem, installed_path)
}

pub(crate) fn project_identity(project: &DiscoveryProject) -> String {
    project_identity_parts(&project.title, &project.slug)
}

fn project_identity_parts(title: &str, slug: &str) -> String {
    // ponytail: provider APIs expose no shared project id; title plus slug avoids
    // hiding unrelated projects while still matching normal cross-provider copies.
    let normalize = |value: &str| {
        value
            .chars()
            .filter(|character| character.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    format!("{}:{}", normalize(title), normalize(slug))
}

pub(crate) fn merge_provider_results(
    mut pages: Vec<(&str, DiscoveryResults)>,
    preferred: &str,
    known_projects: std::collections::HashMap<String, (String, String)>,
) -> MergedDiscoveryResults {
    pages.sort_by_key(|(provider, _)| *provider != preferred);
    let received = pages
        .iter()
        .map(|(_, page)| page.projects.len())
        .max()
        .unwrap_or(0);
    let total_hits = pages
        .iter()
        .map(|(_, page)| page.total_hits)
        .max()
        .unwrap_or(0);
    let mut projects = Vec::new();
    let mut sources = Vec::new();
    let mut primary_stems = known_projects;
    let mut used_stems = primary_stems
        .values()
        .map(|(stem, _)| stem.clone())
        .collect::<std::collections::HashSet<_>>();

    for (provider, page) in pages {
        for project in page.projects {
            let identity = project_identity(&project);
            let project_id = project.id.clone();
            let existing = primary_stems.get(&identity).cloned();
            let duplicate_stem = existing
                .as_ref()
                .filter(|(_, existing_provider)| existing_provider != provider)
                .map(|(stem, _)| stem.clone());
            if let Some((stem, existing_provider)) = existing
                && provider == preferred
                && existing_provider != preferred
            {
                projects.push(MergedDiscoveryProject {
                    stem: stem.clone(),
                    provider: provider.to_owned(),
                    project: project.clone(),
                });
                primary_stems.insert(identity.clone(), (stem, provider.to_owned()));
            }
            let stem = duplicate_stem.unwrap_or_else(|| {
                let mut stem = identity.clone();
                if !used_stems.insert(stem.clone()) {
                    stem = format!("{identity}:{provider}:{}", project.id);
                    used_stems.insert(stem.clone());
                }
                primary_stems
                    .entry(identity)
                    .or_insert_with(|| (stem.clone(), provider.to_owned()));
                projects.push(MergedDiscoveryProject {
                    stem: stem.clone(),
                    provider: provider.to_owned(),
                    project,
                });
                stem
            });
            sources.push((
                stem,
                crate::instance::ProviderProject {
                    provider: provider.to_owned(),
                    project_id,
                    version_id: String::new(),
                },
            ));
        }
    }

    MergedDiscoveryResults {
        projects,
        sources,
        received,
        total_hits,
    }
}

fn refresh_source_installed_versions(
    sources: &mut [(String, crate::instance::ProviderProject)],
    target: &DiscoveryTarget,
) {
    let DiscoveryTarget::Content(content) = target else {
        return;
    };
    let Some(manifest) = &content.manifest else {
        return;
    };
    for (_, source) in sources {
        source.version_id = manifest
            .files
            .iter()
            .find_map(|record| record.project_for_provider(&source.provider, &source.project_id))
            .map(|installed| installed.version_id.clone())
            .unwrap_or_default();
    }
}

fn format_downloads(downloads: u64) -> String {
    match downloads {
        1_000_000.. => format!("{:.1}M", downloads as f64 / 1_000_000.0),
        1_000.. => format!("{:.1}K", downloads as f64 / 1_000.0),
        _ => downloads.to_string(),
    }
}

#[cfg(test)]
#[path = "../../tests/widgets/content/discovery.rs"]
mod tests;
