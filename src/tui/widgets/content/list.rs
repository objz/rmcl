// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// generic scrollable list for content items (mods, resource packs, shaders, worlds).
// supports toggling items on/off by renaming files with .disabled suffix,
// search filtering, per-instance caching, and directory change detection.
// also handles minecraft's formatting codes for colored mod names/descriptions
// because apparently mojang thought terminal UIs would need that. thanks guys

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex, mpsc};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use ratatui_image::{CropOptions, Resize, StatefulImage, protocol::StatefulProtocol};
use tui_widget_list::{ListBuilder, ListState as TuiListState, ListView};

use crate::config::theme::THEME;
use crate::instance::content::entry::{ContentEntry, WorldDetails, WorldGameMode};
use crate::instance::content::icons::IconCell;
use crate::time::format_relative_time;

type ScanOneFn = fn(&Path, &str, bool) -> ContentEntry;
static PROVIDER_ICON_SLOTS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(4)));

#[derive(Clone, Copy, Default)]
enum ContentStreamOrder {
    #[default]
    Sorted,
    Source,
}

#[derive(Clone)]
pub struct ContentStream {
    sender: mpsc::Sender<ContentStreamUpdate>,
}

impl ContentStream {
    pub fn send(&self, entry: ContentEntry) -> bool {
        if self.sender.send(ContentStreamUpdate::Entry(entry)).is_ok() {
            crate::feedback::request_redraw();
            true
        } else {
            false
        }
    }

    pub fn send_icon(&self, file_stem: String, path: std::path::PathBuf, bytes: Vec<u8>) -> bool {
        if self
            .sender
            .send(ContentStreamUpdate::Icon {
                file_stem,
                path,
                bytes,
            })
            .is_ok()
        {
            crate::feedback::request_redraw();
            true
        } else {
            false
        }
    }

    pub fn send_icon_unavailable(&self, file_stem: String, path: std::path::PathBuf) -> bool {
        if self
            .sender
            .send(ContentStreamUpdate::IconUnavailable { file_stem, path })
            .is_ok()
        {
            crate::feedback::request_redraw();
            true
        } else {
            false
        }
    }

    pub fn upsert(&self, entry: ContentEntry) -> bool {
        if self.sender.send(ContentStreamUpdate::Upsert(entry)).is_ok() {
            crate::feedback::request_redraw();
            true
        } else {
            false
        }
    }

    pub fn retain(&self, file_stems: HashSet<String>) -> bool {
        if self
            .sender
            .send(ContentStreamUpdate::Retain(file_stems))
            .is_ok()
        {
            crate::feedback::request_redraw();
            true
        } else {
            false
        }
    }
}

enum ContentStreamUpdate {
    Entry(ContentEntry),
    Upsert(ContentEntry),
    Retain(HashSet<String>),
    Icon {
        file_stem: String,
        path: std::path::PathBuf,
        bytes: Vec<u8>,
    },
    IconUnavailable {
        file_stem: String,
        path: std::path::PathBuf,
    },
}

struct CachedList {
    entries: Vec<ContentEntry>,
    selected: Option<usize>,
}

struct PendingContentImage {
    file_stem: String,
    path: std::path::PathBuf,
    icon_lines: Vec<Vec<IconCell>>,
    image: Option<image::DynamicImage>,
}

struct PendingProviderIcon {
    provider: String,
    project_id: String,
    bytes: Vec<u8>,
    description: String,
}

struct DisplayMetadata {
    description: String,
    has_description: bool,
}

struct PaginationState {
    page_size: usize,
    page_count: usize,
    hits: Vec<(Rect, usize)>,
}

const REMOVAL_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

// result from the notify-triggered background diff
struct WatcherDiff {
    toggled: Vec<(String, bool, std::path::PathBuf)>,
    removed: Vec<String>,
    added: Vec<ContentEntry>,
}

pub(crate) struct ContentToggle {
    pub old_path: std::path::PathBuf,
    pub new_path: std::path::PathBuf,
    pub enabled: bool,
}

#[derive(Default)]
pub(crate) struct ContentWatcherUpdate {
    pub toggles: Vec<ContentToggle>,
    pub requires_reconcile: bool,
}

pub struct ContentListState {
    pub entries: Vec<ContentEntry>,
    pub list_state: TuiListState,
    pub scrollbar_state: ScrollbarState,
    pub loaded_for: Option<String>,
    pub loading: bool,
    image_protocols: HashMap<String, StatefulProtocol>,
    requested_images: HashSet<String>,
    pending_entry_images: HashSet<String>,
    pending_images: Arc<Mutex<Vec<PendingContentImage>>>,
    pending_provider_icons: Arc<Mutex<Vec<PendingProviderIcon>>>,
    requested_provider_icons: HashSet<(String, String)>,
    provider_icon_meta_dir: Option<std::path::PathBuf>,
    provider_icon_client: Option<crate::net::HttpClient>,
    images_dirty: bool,
    display_metadata: HashMap<String, DisplayMetadata>,
    pub search: crate::tui::widgets::search::SearchState,
    filter_search: bool,
    cache: HashMap<String, CachedList>,
    // streaming: individual entries arrive here during initial load
    stream_rx: Option<mpsc::Receiver<ContentStreamUpdate>>,
    stream_order: ContentStreamOrder,
    // file watcher: notify callback spawns background work,
    // precomputed diff lands here for the UI to pick up
    watcher_diff: Arc<Mutex<Option<WatcherDiff>>>,
    _watcher: Option<notify::RecommendedWatcher>,
    watched_dir: Option<std::path::PathBuf>,
    // stored for the watcher to scan individual new files
    scan_one_fn: Option<ScanOneFn>,
    content_ext: Option<&'static str>,
    pagination: Option<PaginationState>,
    pending_removals: HashMap<String, std::time::Instant>,
}

#[derive(Clone, Debug)]
pub struct PendingContentDelete {
    pub name: String,
    pub path: std::path::PathBuf,
}

impl Default for ContentListState {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            list_state: TuiListState::default(),
            scrollbar_state: ScrollbarState::default(),
            loaded_for: None,
            loading: false,
            image_protocols: HashMap::new(),
            requested_images: HashSet::new(),
            pending_entry_images: HashSet::new(),
            pending_images: Arc::new(Mutex::new(Vec::new())),
            pending_provider_icons: Arc::new(Mutex::new(Vec::new())),
            requested_provider_icons: HashSet::new(),
            provider_icon_meta_dir: None,
            provider_icon_client: None,
            images_dirty: true,
            display_metadata: HashMap::new(),
            search: crate::tui::widgets::search::SearchState::default(),
            filter_search: true,
            cache: HashMap::new(),
            stream_rx: None,
            stream_order: ContentStreamOrder::default(),
            watcher_diff: Arc::new(Mutex::new(None)),
            _watcher: None,
            watched_dir: None,
            scan_one_fn: None,
            content_ext: None,
            pagination: None,
            pending_removals: HashMap::new(),
        }
    }
}

impl ContentListState {
    pub(crate) fn set_entries(&mut self, entries: Vec<ContentEntry>) {
        self.entries = entries;
        self.list_state = TuiListState::default();
        self.list_state.selected = (!self.entries.is_empty()).then_some(0);
        self.image_protocols.clear();
        self.requested_images.clear();
        self.pending_entry_images.clear();
        self.pending_removals.clear();
        self.images_dirty = true;
        self.rebuild_display_metadata();
        self.update_scrollbar();
    }

    pub fn apply_manifest(
        &mut self,
        manifest: &crate::instance::ContentManifest,
        minecraft_dir: &Path,
        kind: crate::instance::ContentKind,
    ) {
        let mut changed = false;
        let mut invalidated_icons = Vec::new();
        for entry in &mut self.entries {
            let Ok(relative_path) = entry.path.strip_prefix(minecraft_dir) else {
                continue;
            };
            let Some(record) = manifest
                .record(relative_path)
                .filter(|record| record.kind == kind)
            else {
                if entry.provider_project.take().is_some() {
                    if entry.title_suffix.as_deref() == Some("Update") {
                        entry.title_suffix = None;
                    }
                    if entry.provider_icon {
                        entry.icon_bytes = None;
                        entry.icon_lines = Some(crate::instance::content::fallback_icon());
                        entry.provider_icon = false;
                        invalidated_icons.push(entry.file_stem.clone());
                    }
                    if entry.provider_description {
                        entry.description.clear();
                        entry.provider_description = false;
                    }
                    changed = true;
                }
                continue;
            };
            let project = match &record.resolution {
                crate::instance::Resolution::Resolved { project } => Some(project.clone()),
                _ => None,
            };
            if entry.provider_project != project {
                if let Some(previous) = &entry.provider_project {
                    self.requested_provider_icons
                        .remove(&(previous.provider.clone(), previous.project_id.clone()));
                }
                if entry.title_suffix.as_deref() == Some("Update") {
                    entry.title_suffix = None;
                }
                if entry.provider_icon {
                    entry.icon_bytes = None;
                    entry.icon_lines = Some(crate::instance::content::fallback_icon());
                    entry.provider_icon = false;
                    invalidated_icons.push(entry.file_stem.clone());
                }
                if entry.provider_description {
                    entry.description.clear();
                    entry.provider_description = false;
                }
                entry.provider_project = project.clone();
                changed = true;
            }
        }
        if changed {
            for stem in invalidated_icons {
                self.image_protocols.remove(&stem);
                self.requested_images.remove(&stem);
                self.pending_entry_images.remove(&stem);
            }
            self.rebuild_display_metadata();
            crate::feedback::request_redraw();
        }
    }

    pub fn enable_provider_icons(
        &mut self,
        meta_dir: std::path::PathBuf,
        client: crate::net::HttpClient,
    ) {
        self.provider_icon_meta_dir = Some(meta_dir);
        self.provider_icon_client = Some(client);
    }

    pub fn apply_update_snapshot(
        &mut self,
        snapshot: Option<&crate::instance::content::updates::UpdateSnapshot>,
    ) -> bool {
        let mut changed = false;
        for entry in &mut self.entries {
            let update = entry.provider_project.as_ref().is_some_and(|installed| {
                snapshot
                    .and_then(|snapshot| snapshot.update_for(installed))
                    .is_some()
            });
            let suffix = update.then(|| "Update".to_owned());
            if entry.title_suffix != suffix {
                entry.title_suffix = suffix;
                changed = true;
            }
        }
        if changed {
            crate::feedback::request_redraw();
        }
        changed
    }

    pub fn drain_provider_icons(&mut self) -> bool {
        let pending = match self.pending_provider_icons.lock() {
            Ok(mut pending) => pending.drain(..).collect::<Vec<_>>(),
            Err(_) => return false,
        };
        let mut changed = false;
        for metadata in pending {
            for entry in &mut self.entries {
                let matches_project = entry.provider_project.as_ref().is_some_and(|project| {
                    project.provider == metadata.provider
                        && project.project_id == metadata.project_id
                });
                if !matches_project {
                    continue;
                }
                if entry.icon_bytes.is_none() && !metadata.bytes.is_empty() {
                    entry.icon_bytes = Some(metadata.bytes.clone());
                    entry.provider_icon = true;
                    changed = true;
                }
                if entry.description.trim().is_empty() && !metadata.description.trim().is_empty() {
                    entry.description = metadata.description.clone();
                    entry.provider_description = true;
                    self.display_metadata
                        .insert(entry.file_stem.clone(), display_metadata(entry));
                    changed = true;
                }
            }
        }
        if changed {
            self.images_dirty = true;
            crate::feedback::request_redraw();
        }
        changed
    }

    fn request_visible_provider_icons(&mut self, filtered: &[usize], viewport_height: u16) {
        let Some(meta_dir) = self.provider_icon_meta_dir.clone() else {
            return;
        };
        let Some(client) = self.provider_icon_client.clone() else {
            return;
        };
        let projects = self.visible_provider_projects(filtered, viewport_height);
        for project in projects {
            let key = (project.provider.clone(), project.project_id.clone());
            if !self.requested_provider_icons.insert(key) {
                continue;
            }
            let pending = self.pending_provider_icons.clone();
            let slots = PROVIDER_ICON_SLOTS.clone();
            let meta_dir = meta_dir.clone();
            let client = client.clone();
            tokio::spawn(async move {
                let Ok(_permit) = slots.acquire_owned().await else {
                    return;
                };
                match load_provider_metadata(&client, &meta_dir, &project).await {
                    Ok((bytes, description)) => {
                        if let Ok(mut pending) = pending.lock() {
                            pending.push(PendingProviderIcon {
                                provider: project.provider,
                                project_id: project.project_id,
                                bytes,
                                description,
                            });
                            crate::feedback::request_redraw();
                        }
                    }
                    Err(error) => tracing::debug!(
                        "Could not load provider metadata for {} project {}: {}",
                        project.provider,
                        project.project_id,
                        error
                    ),
                }
            });
        }
    }

    fn visible_provider_projects(
        &self,
        filtered: &[usize],
        viewport_height: u16,
    ) -> Vec<crate::instance::ProviderProject> {
        let mut remaining = viewport_height;
        let first = self.list_state.scroll_offset_index();
        let truncation = self.list_state.scroll_truncation();
        let mut projects = Vec::new();

        for (visible_index, &entry_index) in filtered.iter().enumerate().skip(first) {
            let Some(entry) = self.entries.get(entry_index) else {
                continue;
            };
            let height = self.entry_height(entry);
            let visible_height = if visible_index == first {
                height.saturating_sub(truncation)
            } else {
                height
            };
            if visible_height == 0 {
                continue;
            }
            if (entry.icon_bytes.is_none() || entry.description.trim().is_empty())
                && let Some(project) = entry.provider_project.clone()
            {
                projects.push(project);
            }
            remaining = remaining.saturating_sub(visible_height);
            if remaining == 0 {
                break;
            }
        }

        projects.sort_by(|left, right| {
            (&left.provider, &left.project_id).cmp(&(&right.provider, &right.project_id))
        });
        projects.dedup_by(|left, right| {
            left.provider == right.provider && left.project_id == right.project_id
        });
        projects
    }

    fn entry_height(&self, entry: &ContentEntry) -> u16 {
        let icon_rows = entry.icon_lines.as_ref().map_or(0, Vec::len);
        let metadata = self.display_metadata.get(&entry.file_stem);
        let has_second_line = metadata.is_some_and(|metadata| metadata.has_description)
            || entry.footer_label.is_some()
            || entry.world_details.is_some();
        icon_rows.max(if has_second_line { 2 } else { 1 }) as u16
    }

    pub fn start_stream(&mut self, source: impl Into<String>) -> ContentStream {
        self.start_stream_with_order(source, ContentStreamOrder::Sorted)
    }

    pub fn start_source_stream(&mut self, source: impl Into<String>) -> ContentStream {
        self.start_stream_with_order(source, ContentStreamOrder::Source)
    }

    pub fn refresh_source_stream(&mut self, source: impl Into<String>) -> ContentStream {
        let (sender, receiver) = mpsc::channel();
        self.stream_rx = Some(receiver);
        self.stream_order = ContentStreamOrder::Source;
        self.loaded_for = Some(source.into());
        ContentStream { sender }
    }

    fn start_stream_with_order(
        &mut self,
        source: impl Into<String>,
        order: ContentStreamOrder,
    ) -> ContentStream {
        self.images_dirty = true;
        self.image_protocols.clear();
        self.requested_images.clear();
        self.pending_entry_images.clear();
        self.pending_removals.clear();
        self.requested_provider_icons.clear();
        self.entries.clear();
        self.display_metadata.clear();
        self.list_state = TuiListState::default();
        self.loading = true;
        self.loaded_for = Some(source.into());
        self.update_scrollbar();
        let (sender, receiver) = mpsc::channel();
        self.stream_rx = Some(receiver);
        self.stream_order = order;
        ContentStream { sender }
    }

    pub fn request_image_loads(&mut self, picker: &ratatui_image::picker::Picker) {
        if !self.images_dirty {
            return;
        }
        self.images_dirty = false;

        let use_image_protocol =
            picker.protocol_type() != ratatui_image::picker::ProtocolType::Halfblocks;
        let use_quadrants = crate::config::SETTINGS.read().ui.image_protocol
            == crate::config::settings::ImageProtocol::Quadrants;
        if !use_image_protocol {
            self.image_protocols.clear();
        }

        let valid_stems: HashSet<&str> = self
            .entries
            .iter()
            .map(|entry| entry.file_stem.as_str())
            .collect();
        self.image_protocols
            .retain(|stem, _| valid_stems.contains(stem.as_str()));
        self.requested_images
            .retain(|stem| valid_stems.contains(stem.as_str()));

        let font_size = picker.font_size();
        let font_dimensions = (font_size.width, font_size.height);
        for entry in &self.entries {
            if entry.icon_bytes.is_none() || !self.requested_images.insert(entry.file_stem.clone())
            {
                continue;
            }
            let file_stem = entry.file_stem.clone();
            let path = entry.path.clone();
            let bytes = entry.icon_bytes.clone().unwrap_or_default();
            let rows = entry.icon_lines.as_ref().map_or(3, Vec::len) as u32;
            let columns = square_icon_columns(rows as u16, font_dimensions);
            let pending = self.pending_images.clone();

            tokio::spawn(async move {
                let result = tokio::task::spawn_blocking(move || {
                    let Some(image) = image::load_from_memory(&bytes).ok() else {
                        return PendingContentImage {
                            file_stem,
                            path,
                            icon_lines: crate::instance::content::fallback_icon(),
                            image: None,
                        };
                    };
                    let icon_lines = if use_quadrants {
                        crate::instance::content::make_icon_quadrants_from_image(
                            &image,
                            columns,
                            rows as u16,
                        )
                    } else {
                        crate::instance::content::make_icon_pixels_from_image(
                            &image,
                            columns,
                            rows as u16,
                        )
                    };
                    let side = rows * u32::from(font_size.height.max(1));
                    let image = use_image_protocol.then(|| {
                        image.resize_exact(side, side, image::imageops::FilterType::Lanczos3)
                    });
                    PendingContentImage {
                        file_stem,
                        path,
                        icon_lines,
                        image,
                    }
                })
                .await
                .ok();

                if let Some(result) = result
                    && let Ok(mut pending) = pending.lock()
                {
                    pending.push(result);
                    crate::feedback::request_redraw();
                }
            });
        }
    }

    pub fn drain_image_loads(&mut self, picker: &ratatui_image::picker::Picker) {
        let images = match self.pending_images.lock() {
            Ok(mut pending) => std::mem::take(&mut *pending),
            Err(_) => return,
        };

        for result in images {
            if let Some(entry) = self
                .entries
                .iter_mut()
                .find(|entry| entry.file_stem == result.file_stem && entry.path == result.path)
            {
                self.pending_entry_images.remove(&result.file_stem);
                entry.icon_lines = Some(result.icon_lines);
                if let Some(image) = result.image {
                    self.image_protocols
                        .insert(result.file_stem, picker.new_resize_protocol(image));
                }
            }
        }
    }

    // drain streaming entries from the initial load. each entry arrives
    // individually and is inserted in sorted position for a smooth fill-in
    pub fn drain_pending(&mut self) -> bool {
        let Some(rx) = &self.stream_rx else {
            return false;
        };

        let mut received = false;
        let mut received_count = 0usize;
        let mut finished = false;
        let mut restore_selected = None;
        loop {
            match rx.try_recv() {
                Ok(ContentStreamUpdate::Entry(entry)) => {
                    received = true;
                    self.images_dirty = true;
                    received_count += 1;
                    if entry.icon_bytes.is_some() || entry.provider_icon {
                        self.pending_entry_images.insert(entry.file_stem.clone());
                    }
                    self.display_metadata
                        .insert(entry.file_stem.clone(), display_metadata(&entry));
                    match self.stream_order {
                        ContentStreamOrder::Sorted => {
                            let pos = self
                                .entries
                                .binary_search_by(|e| {
                                    e.name.to_lowercase().cmp(&entry.name.to_lowercase())
                                })
                                .unwrap_or_else(|i| i);
                            self.entries.insert(pos, entry);
                        }
                        ContentStreamOrder::Source => self.entries.push(entry),
                    }
                }
                Ok(ContentStreamUpdate::Upsert(mut entry)) => {
                    received = true;
                    self.images_dirty = true;
                    received_count += 1;
                    let stem = entry.file_stem.clone();
                    if let Some(existing) = self
                        .entries
                        .iter_mut()
                        .find(|existing| existing.file_stem == stem)
                    {
                        let same_source = existing.path == entry.path
                            && existing.provider_project == entry.provider_project;
                        if entry.icon_bytes.is_none() && same_source {
                            entry.icon_bytes = existing.icon_bytes.take();
                            entry.icon_lines = existing.icon_lines.take();
                            entry.provider_icon = existing.provider_icon;
                        } else if entry.icon_bytes != existing.icon_bytes {
                            self.image_protocols.remove(&entry.file_stem);
                            self.requested_images.remove(&entry.file_stem);
                        }
                        if entry.description.trim().is_empty()
                            && same_source
                            && existing.provider_description
                        {
                            entry.description = std::mem::take(&mut existing.description);
                            entry.provider_description = true;
                        }
                        *existing = entry;
                    } else {
                        self.entries.push(entry);
                    }
                    if let Some(entry) = self.entries.iter().find(|entry| entry.file_stem == stem) {
                        self.display_metadata
                            .insert(entry.file_stem.clone(), display_metadata(entry));
                        if (entry.icon_bytes.is_some() || entry.provider_icon)
                            && !self.image_protocols.contains_key(&entry.file_stem)
                        {
                            self.pending_entry_images.insert(stem);
                        } else {
                            self.pending_entry_images.remove(&stem);
                        }
                    }
                }
                Ok(ContentStreamUpdate::Retain(file_stems)) => {
                    received = true;
                    let selected_stem = self.selected_file_stem();
                    self.entries
                        .retain(|entry| file_stems.contains(&entry.file_stem));
                    self.display_metadata
                        .retain(|stem, _| file_stems.contains(stem));
                    self.image_protocols
                        .retain(|stem, _| file_stems.contains(stem));
                    self.requested_images
                        .retain(|stem| file_stems.contains(stem));
                    self.pending_entry_images
                        .retain(|stem| file_stems.contains(stem));
                    restore_selected = Some(selected_stem);
                    self.images_dirty = true;
                }
                Ok(ContentStreamUpdate::Icon {
                    file_stem,
                    path,
                    bytes,
                }) => {
                    received = true;
                    if let Some(entry) = self
                        .entries
                        .iter_mut()
                        .find(|entry| entry.file_stem == file_stem && entry.path == path)
                    {
                        entry.icon_bytes = Some(bytes);
                        entry.provider_icon = true;
                        self.pending_entry_images.insert(file_stem.clone());
                        self.requested_images.remove(&file_stem);
                        self.images_dirty = true;
                    }
                }
                Ok(ContentStreamUpdate::IconUnavailable { file_stem, path }) => {
                    received = true;
                    if let Some(entry) = self
                        .entries
                        .iter_mut()
                        .find(|entry| entry.file_stem == file_stem && entry.path == path)
                    {
                        entry.provider_icon = false;
                        self.pending_entry_images.remove(&file_stem);
                        self.images_dirty = true;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.stream_rx = None;
                    finished = true;
                    break;
                }
            }
        }

        if let Some(selected_stem) = restore_selected {
            self.restore_selected_file_stem(selected_stem.as_deref());
        }

        if received || finished {
            self.loading = false;
            if received_count > 0 {
                tracing::trace!(
                    "Drained {} streamed content entries for {}",
                    received_count,
                    self.loaded_for.as_deref().unwrap_or("<unknown>")
                );
            }
            if finished {
                tracing::debug!(
                    "Finished content scan for {} with {} entries",
                    self.loaded_for.as_deref().unwrap_or("<unknown>"),
                    self.entries.len()
                );
            }
            if self.list_state.selected.is_none() && !self.entries.is_empty() {
                self.list_state.selected = Some(0);
            }
            self.update_scrollbar();
        }
        received || finished
    }

    // pick up the precomputed diff from the notify watcher callback.
    // skip while streaming is in progress to avoid duplicate entries.
    pub(crate) fn drain_watcher(&mut self) -> ContentWatcherUpdate {
        if self.stream_rx.is_some() {
            return ContentWatcherUpdate::default();
        }

        let expired_removals = self.expire_pending_removals();

        let diff = match self.watcher_diff.lock() {
            Ok(mut slot) => slot.take(),
            _ => None,
        };

        let Some(diff) = diff else {
            return ContentWatcherUpdate {
                requires_reconcile: expired_removals,
                ..ContentWatcherUpdate::default()
            };
        };
        let mut update = ContentWatcherUpdate {
            requires_reconcile: expired_removals || !diff.added.is_empty(),
            ..ContentWatcherUpdate::default()
        };
        self.images_dirty |= update.requires_reconcile;

        // apply toggles (enabled/path changes)
        tracing::debug!(
            "Applying content watcher diff for {}: toggled={} removed={} added={}",
            self.loaded_for.as_deref().unwrap_or("<unknown>"),
            diff.toggled.len(),
            diff.removed.len(),
            diff.added.len()
        );
        for (stem, enabled, path) in &diff.toggled {
            self.pending_removals.remove(stem);
            if let Some(entry) = self.entries.iter_mut().find(|e| &e.file_stem == stem) {
                let old_path = if entry.path == *path {
                    opposite_toggle_path(path, *enabled)
                } else {
                    entry.path.clone()
                };
                update.toggles.push(ContentToggle {
                    old_path,
                    new_path: path.clone(),
                    enabled: *enabled,
                });
                entry.enabled = *enabled;
                entry.path = path.clone();
            } else {
                update.requires_reconcile = true;
            }
        }

        // A version change is reported by the filesystem as a removal followed
        // by an addition. Keep the old row briefly so the replacement can take
        // its place without flashing out of the list.
        for stem in diff.removed {
            self.pending_removals
                .entry(stem)
                .or_insert_with(std::time::Instant::now);
        }

        // insert new entries in sorted position
        for mut entry in diff.added {
            let replacement = self.entries.iter().position(|existing| {
                self.pending_removals.contains_key(&existing.file_stem)
                    && existing.name.eq_ignore_ascii_case(&entry.name)
            });
            if let Some(index) = replacement {
                let old_stem = self.entries[index].file_stem.clone();
                self.pending_removals.remove(&old_stem);
                preserve_visual_metadata(&mut entry, &mut self.entries[index]);
                self.display_metadata.remove(&old_stem);
                let protocol = self.image_protocols.remove(&old_stem);
                let image_requested = self.requested_images.remove(&old_stem);
                let image_pending = self.pending_entry_images.remove(&old_stem);
                self.entries[index] = entry;
                let entry = &self.entries[index];
                if let Some(protocol) = protocol {
                    self.image_protocols
                        .insert(entry.file_stem.clone(), protocol);
                }
                if image_requested {
                    self.requested_images.insert(entry.file_stem.clone());
                }
                if image_pending {
                    self.pending_entry_images.insert(entry.file_stem.clone());
                }
                self.display_metadata
                    .insert(entry.file_stem.clone(), display_metadata(entry));
                self.images_dirty = true;
                continue;
            }
            if entry.icon_bytes.is_some() {
                self.pending_entry_images.insert(entry.file_stem.clone());
            }
            self.display_metadata
                .insert(entry.file_stem.clone(), display_metadata(&entry));
            let pos = self
                .entries
                .binary_search_by(|e| e.name.to_lowercase().cmp(&entry.name.to_lowercase()))
                .unwrap_or_else(|i| i);
            self.entries.insert(pos, entry);
        }

        // clamp selected
        if let Some(sel) = self.list_state.selected {
            if self.entries.is_empty() {
                self.list_state.selected = None;
            } else {
                self.list_state.selected = Some(sel.min(self.entries.len().saturating_sub(1)));
            }
        }

        self.update_scrollbar();
        update
    }

    fn expire_pending_removals(&mut self) -> bool {
        let expired = self
            .pending_removals
            .iter()
            .filter(|(_, since)| since.elapsed() >= REMOVAL_GRACE)
            .map(|(stem, _)| stem.clone())
            .collect::<Vec<_>>();
        let mut removed = false;
        for stem in expired {
            self.pending_removals.remove(&stem);
            let restored = self
                .entries
                .iter()
                .find(|entry| entry.file_stem == stem)
                .is_some_and(|entry| entry.path.exists());
            if restored {
                continue;
            }
            let before = self.entries.len();
            self.entries.retain(|entry| entry.file_stem != stem);
            removed |= self.entries.len() != before;
            self.display_metadata.remove(&stem);
            self.image_protocols.remove(&stem);
            self.requested_images.remove(&stem);
            self.pending_entry_images.remove(&stem);
        }
        if removed {
            self.images_dirty = true;
            self.update_scrollbar();
        }
        removed
    }

    // starts a notify file watcher on the given directory. changes trigger
    // a background diff that lands in watcher_diff for drain_watcher to apply.
    pub fn watch_dir(&mut self, dir: std::path::PathBuf) {
        use notify::{RecursiveMode, Watcher};
        use std::sync::atomic::{AtomicBool, Ordering};

        // drop previous watcher
        self._watcher = None;

        let watcher_diff = self.watcher_diff.clone();
        let ext: &'static str = self.content_ext.unwrap_or(".jar");
        let scan_one = self.scan_one_fn;

        let running = Arc::new(AtomicBool::new(false));
        let running_cb = running.clone();
        let pending_paths = Arc::new(Mutex::new(HashSet::<std::path::PathBuf>::new()));
        let pending_paths_cb = pending_paths.clone();
        let needs_rescan = Arc::new(AtomicBool::new(false));
        let needs_rescan_cb = needs_rescan.clone();

        // initialize known stems from the current directory state so existing
        // files are not treated as "new" on the first notify event
        let known_stems = Arc::new(Mutex::new(read_dir_stems(&dir, ext)));

        let watch_dir = dir.clone();
        let watcher = notify::recommended_watcher(move |res: Result<notify::Event, _>| {
            let event_paths = match res {
                Ok(event) => match watcher_event_handling(&event.kind) {
                    WatcherEventHandling::Ignore => return,
                    WatcherEventHandling::Paths => event.paths,
                    WatcherEventHandling::Rescan => {
                        needs_rescan_cb.store(true, Ordering::Relaxed);
                        event.paths
                    }
                },
                Err(e) => {
                    tracing::warn!(
                        "Content watcher event error for {}: {}",
                        watch_dir.display(),
                        e
                    );
                    needs_rescan_cb.store(true, Ordering::Relaxed);
                    Vec::new()
                }
            };
            if let Ok(mut pending) = pending_paths_cb.lock() {
                pending.extend(event_paths);
            }

            if running_cb.swap(true, Ordering::Relaxed) {
                return;
            }

            let dir = watch_dir.clone();
            let diff_slot = watcher_diff.clone();
            let running = running_cb.clone();
            let known = known_stems.clone();
            let pending_paths = pending_paths_cb.clone();
            let needs_rescan = needs_rescan_cb.clone();

            std::thread::spawn(move || {
                // always clear `running` even if we panic
                struct ResetOnDrop(Arc<AtomicBool>);
                impl Drop for ResetOnDrop {
                    fn drop(&mut self) {
                        self.0.store(false, Ordering::Relaxed);
                    }
                }
                let _guard = ResetOnDrop(running.clone());

                loop {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    let paths = pending_paths
                        .lock()
                        .ok()
                        .map(|mut pending| pending.drain().collect::<Vec<_>>())
                        .unwrap_or_default();
                    let full_rescan = needs_rescan.swap(false, Ordering::Relaxed);
                    let result = if full_rescan {
                        diff_directory(&dir, ext, scan_one, &known)
                    } else {
                        diff_event_paths(&dir, &paths, ext, scan_one, &known)
                    };

                    if let Some(diff) = result
                        && let Ok(mut slot) = diff_slot.lock()
                    {
                        if let Some(pending) = slot.as_mut() {
                            merge_watcher_diff(pending, diff);
                        } else {
                            *slot = Some(diff);
                        }
                        crate::feedback::request_redraw();
                    }

                    let no_pending = pending_paths.lock().is_ok_and(|pending| pending.is_empty());
                    if no_pending && !needs_rescan.load(Ordering::Relaxed) {
                        // Release ownership before the final check. An event
                        // racing with this boundary either starts a new worker
                        // or is observed here and kept by this worker.
                        running.store(false, Ordering::Release);
                        let still_empty =
                            pending_paths.lock().is_ok_and(|pending| pending.is_empty());
                        if still_empty && !needs_rescan.load(Ordering::Acquire) {
                            break;
                        }
                        if running
                            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            });
        });

        match watcher {
            Ok(mut w) => {
                if let Err(e) = w.watch(&dir, RecursiveMode::NonRecursive) {
                    tracing::warn!("Failed to watch {}: {e}", dir.display());
                } else {
                    tracing::debug!("Watching content directory {}", dir.display());
                    self._watcher = Some(w);
                }
            }
            Err(e) => {
                tracing::warn!("Failed to create file watcher: {e}");
            }
        }

        self.watched_dir = Some(dir);
    }

    pub fn filtered_indices(&self) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                !self.pending_entry_images.contains(&entry.file_stem)
                    && (!self.filter_search
                        || self.search.matches(&entry.name)
                        || self.search.matches(&entry.description))
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn set_search_filtering(&mut self, enabled: bool) {
        let selected_stem = self.selected_file_stem();
        self.filter_search = enabled;
        self.restore_selected_file_stem(selected_stem.as_deref());
    }

    fn selected_file_stem(&self) -> Option<String> {
        let filtered = self.filtered_indices();
        let entry_index = self
            .list_state
            .selected
            .and_then(|index| filtered.get(index))?;
        self.entries
            .get(*entry_index)
            .map(|entry| entry.file_stem.clone())
    }

    pub fn selected_entry(&self) -> Option<&ContentEntry> {
        let filtered = self.filtered_indices();
        let index = self
            .list_state
            .selected
            .and_then(|index| filtered.get(index))?;
        self.entries.get(*index)
    }

    pub(crate) fn selected_has_provider_project(&self) -> bool {
        self.selected_entry()
            .is_some_and(|entry| entry.provider_project.is_some())
    }

    fn restore_selected_file_stem(&mut self, file_stem: Option<&str>) {
        let filtered = self.filtered_indices();
        self.list_state.selected = file_stem
            .and_then(|stem| {
                filtered.iter().position(|index| {
                    self.entries
                        .get(*index)
                        .is_some_and(|entry| entry.file_stem == stem)
                })
            })
            .or_else(|| (!filtered.is_empty()).then_some(0));
        self.update_scrollbar();
    }

    pub fn pending_delete(&self) -> Option<PendingContentDelete> {
        let filtered = self.filtered_indices();
        let real_idx = self.list_state.selected.and_then(|i| filtered.get(i))?;
        let entry = self.entries.get(*real_idx)?;
        Some(PendingContentDelete {
            name: entry.name.clone(),
            path: entry.path.clone(),
        })
    }
}

impl ContentListState {
    pub fn forget_instance(&mut self, instance_name: &str) {
        let world_prefix = format!("{instance_name}:");
        self.cache
            .retain(|source, _| source != instance_name && !source.starts_with(&world_prefix));
        if self
            .loaded_for
            .as_deref()
            .is_some_and(|source| source == instance_name || source.starts_with(&world_prefix))
        {
            self.loaded_for = None;
        }
    }

    // saves current entries to cache before loading new ones, and restores
    // from cache if this instance was seen before (avoids re-scanning).
    // content_dir is the actual directory to scan (e.g. .minecraft/mods).
    pub fn start_load(
        &mut self,
        content_dir: &Path,
        instance_name: &str,
        scan_one_fn: ScanOneFn,
        ext: &'static str,
    ) {
        self.scan_one_fn = Some(scan_one_fn);
        self.content_ext = Some(ext);
        self.images_dirty = true;
        self.image_protocols.clear();
        self.requested_images.clear();
        self.pending_entry_images.clear();
        self.pending_removals.clear();

        // save current entries to cache
        if let Some(prev) = self.loaded_for.take()
            && !self.entries.is_empty()
        {
            tracing::trace!(
                "Caching {} content entries for {}",
                self.entries.len(),
                prev
            );
            self.cache.insert(
                prev,
                CachedList {
                    entries: std::mem::take(&mut self.entries),
                    selected: self.list_state.selected,
                },
            );
        }

        // try cache first
        if let Some(cached) = self.cache.remove(instance_name) {
            self.entries = cached.entries;
            self.pending_entry_images.extend(
                self.entries
                    .iter()
                    .filter(|entry| entry.icon_bytes.is_some())
                    .map(|entry| entry.file_stem.clone()),
            );
            self.rebuild_display_metadata();
            self.list_state.selected = cached.selected;
            self.loading = false;
            self.stream_rx = None;
            self.loaded_for = Some(instance_name.to_string());
            self.update_scrollbar();
            tracing::debug!(
                "Restored {} cached content entries for {}",
                self.entries.len(),
                instance_name
            );
            return;
        }

        // no cache, stream entries one by one as each file is scanned
        let stream = self.start_stream(instance_name);

        let dir = content_dir.to_path_buf();
        tracing::debug!(
            "Starting content scan for {} in {}",
            instance_name,
            content_dir.display()
        );

        tokio::spawn(async move {
            let _ = tokio::task::spawn_blocking(move || {
                let read_dir = match std::fs::read_dir(&dir) {
                    Ok(rd) => rd,
                    Err(e) => {
                        tracing::warn!("Failed to read content directory {}: {}", dir.display(), e);
                        return;
                    }
                };
                let disabled_ext = format!("{ext}.disabled");

                for dir_entry in read_dir.flatten() {
                    let path = dir_entry.path();
                    let Some(fname) = path.file_name().and_then(|n| n.to_str()) else {
                        tracing::trace!(
                            "Skipping content path with invalid filename: {}",
                            path.display()
                        );
                        continue;
                    };

                    let (enabled, file_stem) = if let Some(stem) = fname.strip_suffix(&disabled_ext)
                    {
                        (false, stem.to_owned())
                    } else if let Some(stem) = fname.strip_suffix(ext) {
                        (true, stem.to_owned())
                    } else if path.is_dir() {
                        crate::instance::content::parse_enabled_stem_dir(fname)
                    } else {
                        tracing::trace!(
                            "Skipping content path with unsupported extension: {}",
                            path.display()
                        );
                        continue;
                    };

                    let entry = scan_one_fn(&path, &file_stem, enabled);
                    if !stream.send(entry) {
                        break; // receiver dropped (instance switched)
                    }
                }
            })
            .await;
        });
    }

    fn update_scrollbar(&mut self) {
        let count = self.entries.len();
        let max = count.saturating_sub(1);
        let pos = self.list_state.selected.unwrap_or(0);
        self.scrollbar_state = ScrollbarState::new(max).position(pos);
    }

    pub fn previous_page(&mut self) -> bool {
        let Some(pagination) = self.pagination.as_ref() else {
            return false;
        };
        let current = self.list_state.selected.unwrap_or(0) / pagination.page_size;
        self.jump_to_page(current.saturating_sub(1))
    }

    pub fn next_page(&mut self) -> bool {
        let Some(pagination) = self.pagination.as_ref() else {
            return false;
        };
        let current = self.list_state.selected.unwrap_or(0) / pagination.page_size;
        self.jump_to_page((current + 1).min(pagination.page_count.saturating_sub(1)))
    }

    pub fn click_page(&mut self, x: u16, y: u16) -> bool {
        let page = self.pagination.as_ref().and_then(|pagination| {
            pagination
                .hits
                .iter()
                .find(|(area, _)| {
                    x >= area.x && x < area.right() && y >= area.y && y < area.bottom()
                })
                .map(|(_, page)| *page)
        });
        page.is_some_and(|page| self.jump_to_page(page))
    }

    fn jump_to_page(&mut self, page: usize) -> bool {
        let Some(pagination) = self.pagination.as_ref() else {
            return false;
        };
        let target = page
            .min(pagination.page_count.saturating_sub(1))
            .saturating_mul(pagination.page_size);
        self.list_state.select(Some(target));
        self.update_scrollbar();
        true
    }

    fn rebuild_display_metadata(&mut self) {
        self.display_metadata = self
            .entries
            .iter()
            .map(|entry| (entry.file_stem.clone(), display_metadata(entry)))
            .collect();
    }

    // enable/disable by renaming the file with/without .disabled extension.
    // this is how most minecraft launchers handle it
    pub fn toggle_selected(&mut self) {
        let Some(index) = self.list_state.selected else {
            return;
        };
        let Some(entry) = self.entries.get(index) else {
            return;
        };
        match crate::instance::content::entry::toggle_entry_path(entry) {
            Ok(Some(new_path)) => {
                let entry = &mut self.entries[index];
                entry.enabled = !entry.enabled;
                entry.path = new_path;
            }
            Ok(None) => {}
            Err(e) => {
                tracing::error!(
                    "Failed to toggle '{}' at {}: {}",
                    entry.file_stem,
                    entry.path.display(),
                    e
                );
            }
        }
    }

    pub fn remove_path(&mut self, path: &Path) {
        let file_stem = self
            .entries
            .iter()
            .find(|entry| entry.path == path)
            .map(|entry| entry.file_stem.clone());
        self.entries.retain(|entry| entry.path != path);
        if let Some(file_stem) = file_stem {
            self.image_protocols.remove(&file_stem);
            self.requested_images.remove(&file_stem);
            self.display_metadata.remove(&file_stem);
        }
        self.images_dirty = true;
        if let Some(sel) = self.list_state.selected {
            let visible_count = self.filtered_indices().len();
            if visible_count == 0 {
                self.list_state.selected = None;
            } else {
                self.list_state.selected = Some(sel.min(visible_count.saturating_sub(1)));
            }
        }
        self.update_scrollbar();
    }
}

fn handle_search_keys(key_event: &KeyEvent, state: &mut ContentListState) -> bool {
    if state.search.active {
        match key_event.code {
            KeyCode::Enter => {
                state.search.confirm();
                state.list_state.selected = Some(0);
                state.update_scrollbar();
            }
            KeyCode::Esc => {
                state.search.deactivate();
                state.list_state.selected = Some(0);
                state.update_scrollbar();
            }
            KeyCode::Backspace => {
                state.search.backspace(key_event.modifiers);
                state.list_state.selected = Some(0);
                state.update_scrollbar();
            }
            KeyCode::Char(c) => {
                state.search.push(c);
                state.list_state.selected = Some(0);
                state.update_scrollbar();
            }
            _ => {}
        }
        return true;
    }
    if key_event.code == KeyCode::Char('/') {
        state.search.activate();
        state.list_state.selected = Some(0);
        state.update_scrollbar();
        return true;
    }
    false
}

async fn load_provider_metadata(
    client: &crate::net::HttpClient,
    meta_dir: &Path,
    installed: &crate::instance::ProviderProject,
) -> Result<(Vec<u8>, String), crate::net::NetError> {
    let provider_id = &installed.provider;
    let project_id = &installed.project_id;
    let metadata = crate::storage::MetadataPaths::new(meta_dir);
    let icon_path = metadata
        .provider_icons(provider_id)
        .join(format!("{project_id}.img"));
    let cached_icon = tokio::fs::read(&icon_path)
        .await
        .ok()
        .filter(|bytes| !bytes.is_empty() && image::load_from_memory(bytes).is_ok());

    let registry = crate::instance::content::provider::ProviderRegistry::configured(client.clone());
    let provider = registry.get(provider_id).ok_or_else(|| {
        crate::net::NetError::Parse(format!("Content provider '{provider_id}' is unavailable"))
    })?;
    let project_path = metadata
        .provider_projects(provider_id)
        .join(format!("{project_id}.json"));
    let cached_project = tokio::fs::read(&project_path)
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let project = match cached_project {
        Some(project) => project,
        None => {
            let project = match provider.project(project_id).await {
                Ok(project) => project,
                Err(error) => {
                    return cached_icon.map(|bytes| (bytes, String::new())).ok_or(error);
                }
            };
            crate::storage::write_atomic(
                &project_path,
                &serde_json::to_vec_pretty(&project)
                    .map_err(|error| crate::net::NetError::Parse(error.to_string()))?,
            )?;
            project
        }
    };
    let bytes = match (cached_icon, project.icon_url.as_deref()) {
        (Some(bytes), _) => bytes,
        (None, Some(url)) => match provider.icon(url).await {
            Ok(bytes) if !bytes.is_empty() && image::load_from_memory(&bytes).is_ok() => {
                crate::storage::write_atomic(&icon_path, &bytes)?;
                bytes
            }
            Ok(_) => {
                tracing::debug!("Provider returned an invalid icon for project '{project_id}'");
                Vec::new()
            }
            Err(error) => {
                tracing::debug!("Could not fetch icon for project '{project_id}': {error}");
                Vec::new()
            }
        },
        (None, None) => Vec::new(),
    };
    Ok((bytes, project.description))
}

pub fn handle_key_no_toggle(key_event: &KeyEvent, state: &mut ContentListState) -> bool {
    handle_key_inner(key_event, state, false)
}

pub fn handle_key(key_event: &KeyEvent, state: &mut ContentListState) -> bool {
    handle_key_inner(key_event, state, true)
}

fn handle_key_inner(
    key_event: &KeyEvent,
    state: &mut ContentListState,
    toggle_on_enter: bool,
) -> bool {
    if handle_search_keys(key_event, state) {
        return true;
    }
    let filtered = state.filtered_indices();
    let count = filtered.len();

    match key_event.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if count == 0 {
                return true;
            }
            let current = state.list_state.selected.unwrap_or(0);
            state.list_state.selected = Some((current + 1).min(count - 1));
            state.update_scrollbar();
            true
        }
        KeyCode::Char('k') | KeyCode::Up => {
            let current = state.list_state.selected.unwrap_or(0);
            state.list_state.selected = Some(current.saturating_sub(1));
            state.update_scrollbar();
            true
        }
        KeyCode::Enter if key_event.modifiers.contains(KeyModifiers::SHIFT) => {
            if let Some(&real_idx) = state.list_state.selected.and_then(|i| filtered.get(i))
                && let Some(dir) = state.entries[real_idx].path.parent()
                && let Err(e) = open::that_detached(dir)
            {
                tracing::error!("Failed to open directory: {}", e);
            }
            true
        }
        KeyCode::Enter if toggle_on_enter => {
            if let Some(&real_idx) = state.list_state.selected.and_then(|i| filtered.get(i)) {
                state.list_state.selected = Some(real_idx);
                state.toggle_selected();
                state.list_state.selected =
                    Some(filtered.iter().position(|&i| i == real_idx).unwrap_or(0));
            }
            true
        }
        _ => false,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render(
    frame: &mut Frame,
    area: Rect,
    state: &mut ContentListState,
    is_focused: bool,
    loading_text: &str,
    empty_text: &str,
    picker: &ratatui_image::picker::Picker,
    paginate: bool,
    multiline_descriptions: bool,
) {
    let theme = THEME.as_ref();
    state.pagination = None;
    if state.loading {
        frame.render_widget(
            Paragraph::new(loading_text).style(Style::default().fg(theme.text_dim())),
            area,
        );
        return;
    }

    let filtered = state.filtered_indices();

    if filtered.is_empty() {
        state.list_state.selected = None;
        let text = if state.pending_entry_images.is_empty() {
            empty_text
        } else {
            loading_text
        };
        frame.render_widget(
            Paragraph::new(text).style(Style::default().fg(theme.text_dim())),
            area,
        );
        return;
    }

    if state.list_state.selected.is_none() {
        state.list_state.selected = Some(0);
    }
    let count = filtered.len();

    // clamp selected so the ListView builder never gets an out-of-bounds index
    if let Some(sel) = state.list_state.selected
        && sel >= count
    {
        state.list_state.selected = Some(count.saturating_sub(1));
    }
    let (list_area, pagination) = if paginate {
        pagination_layout(area, count)
    } else {
        (area, None)
    };
    state.request_visible_provider_icons(&filtered, list_area.height);

    let use_image_protocol =
        picker.protocol_type() != ratatui_image::picker::ProtocolType::Halfblocks;
    let entries = &state.entries;
    let display_metadata = &state.display_metadata;
    let filtered_rows = &filtered;
    let search = &state.search;
    let ready_image_stems: HashSet<String> = state.image_protocols.keys().cloned().collect();

    let builder = ListBuilder::new(move |context| {
        let theme = THEME.as_ref();
        let entry = &entries[filtered_rows[context.index]];
        let name = &entry.name;
        let metadata = display_metadata.get(&entry.file_stem);
        let enabled = entry.enabled;
        let icon_pixels = &entry.icon_lines;
        let world_details = entry.world_details.as_ref();
        let title_suffix = world_details
            .and_then(|details| details.game_mode)
            .map(WorldGameMode::label)
            .or(entry.title_suffix.as_deref());
        let world_footer = world_details.map(|details| format_relative_time(details.last_played));
        let footer_label = world_footer.as_deref().or(entry.footer_label.as_deref());
        // Keep rendering the terminal fallback until the asynchronous image
        // decoder has produced a protocol. Invalid and unsupported images
        // therefore remain visible as a question mark instead of blank space.
        let has_image = ready_image_stems.contains(&entry.file_stem);
        let protocol_columns = protocol_icon_columns(entry, picker) as usize;
        let show_selected = is_focused && context.is_selected;
        let use_mc_colors = enabled;

        let stripe_bg = if context.index % 2 == 0 {
            theme.background()
        } else {
            theme.stripe()
        };

        let (name_style, description_style, background) = match (enabled, show_selected) {
            (true, true) => (
                Style::default()
                    .fg(theme.accent())
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(theme.text_dim()),
                stripe_bg,
            ),
            (true, false) => (
                Style::default()
                    .fg(theme.text())
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(theme.text_dim()),
                stripe_bg,
            ),
            (false, true) => (
                Style::default()
                    .fg(theme.accent())
                    .add_modifier(Modifier::CROSSED_OUT),
                Style::default().fg(theme.text_dim()),
                stripe_bg,
            ),
            (false, false) => (
                Style::default()
                    .fg(theme.text_dim())
                    .add_modifier(Modifier::CROSSED_OUT),
                Style::default().fg(theme.text_dim()),
                stripe_bg,
            ),
        };
        let title_suffix_color = world_details
            .and_then(|details| details.game_mode)
            .map(world_game_mode_color)
            .unwrap_or_else(|| {
                if matches!(title_suffix, Some("Update" | "Skipped")) {
                    theme.warning()
                } else {
                    theme.success()
                }
            });
        let title_suffix_style = crate::tui::widgets::status_badge_style(title_suffix_color);
        let footer_label_style = Style::default().fg(if world_details.is_some() {
            theme.text_dim()
        } else {
            theme.text()
        });
        let footer_spans = entry.footer_change.as_ref().map_or_else(
            || {
                footer_label.map_or_else(Vec::new, |label| {
                    vec![Span::styled(label.to_owned(), footer_label_style)]
                })
            },
            |(from, to)| version_change_spans(from, to),
        );
        let footer_width = footer_spans.iter().map(Span::width).sum();
        let has_footer = !footer_spans.is_empty();

        let world_descriptions = world_details.map(world_descriptions);
        let has_icon = icon_pixels.is_some();
        let mut descriptions = if let Some(lines) = world_descriptions.as_ref() {
            lines.iter().map(String::as_str).collect()
        } else {
            metadata
                .map(|metadata| {
                    metadata
                        .description
                        .lines()
                        .map(str::trim)
                        .filter(|line| !line.is_empty())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        if !multiline_descriptions {
            descriptions.truncate(1);
        }
        let has_description = !descriptions.is_empty();
        let rendered_icon_columns = if use_image_protocol && has_image {
            protocol_columns
        } else {
            icon_pixels
                .as_ref()
                .and_then(|rows| rows.first())
                .map_or(0, Vec::len)
        };
        let description_width = available_description_width(
            usize::from(context.cross_axis_size),
            rendered_icon_columns,
            has_icon,
        );
        let visible_descriptions = descriptions
            .iter()
            .enumerate()
            .map(|(index, description)| {
                ellipsize(
                    description,
                    if index == 0 {
                        description_text_width(description_width, footer_width, has_description)
                    } else {
                        description_width
                    },
                )
            })
            .collect::<Vec<_>>();
        let visible_description = visible_descriptions.first().map_or("", String::as_str);
        let compact = !has_icon && !has_description && !has_footer;

        let selector = if show_selected {
            Span::styled("\u{258c}", Style::default().fg(theme.accent()))
        } else {
            Span::raw(" ")
        };

        if compact {
            let mut line = Vec::new();
            line.push(selector.clone());
            line.extend(searchable_spans(search, name, name_style, use_mc_colors));
            line.extend(title_suffix_spans(
                title_suffix,
                description_style,
                title_suffix_style,
            ));

            let item = Text::from(vec![Line::from(line)]).style(Style::default().bg(background));
            (item, 1)
        } else if has_icon {
            let icon_row_count = icon_pixels.as_ref().map(|r| r.len()).unwrap_or(0);
            let text_rows = 1 + visible_descriptions.len().max(usize::from(has_footer));
            let height = icon_row_count.max(text_rows) as u16;

            let pad = if show_selected {
                Span::styled("\u{258c}", Style::default().fg(theme.accent()))
            } else {
                Span::raw(" ")
            };

            let mut line_0 = vec![selector.clone()];
            line_0.extend(icon_spans(
                icon_pixels.as_ref(),
                0,
                use_image_protocol && has_image,
                protocol_columns,
            ));
            line_0.push(Span::raw(" "));
            line_0.extend(searchable_spans(search, name, name_style, use_mc_colors));
            line_0.extend(title_suffix_spans(
                title_suffix,
                description_style,
                title_suffix_style,
            ));

            let mut lines = vec![Line::from(line_0)];

            for r in 1..text_rows {
                let mut row = vec![pad.clone()];
                row.extend(icon_spans(
                    icon_pixels.as_ref(),
                    r,
                    use_image_protocol && has_image,
                    protocol_columns,
                ));
                row.push(Span::raw(" "));
                if let Some(description) = visible_descriptions.get(r - 1) {
                    row.extend(search.highlight_spans(description, description_style));
                }
                if r == 1 && has_footer {
                    row.extend(right_aligned_footer_spans(
                        description_width,
                        visible_description,
                        has_description,
                        footer_spans.clone(),
                    ));
                }
                lines.push(Line::from(row));
            }

            for r in text_rows..icon_row_count {
                let mut row = vec![pad.clone()];
                row.extend(icon_spans(
                    icon_pixels.as_ref(),
                    r,
                    use_image_protocol && has_image,
                    protocol_columns,
                ));
                lines.push(Line::from(row));
            }

            let item = Text::from(lines).style(Style::default().bg(background));
            (item, height)
        } else {
            let mut line_0 = Vec::new();
            line_0.push(selector.clone());
            line_0.extend(searchable_spans(search, name, name_style, use_mc_colors));
            line_0.extend(title_suffix_spans(
                title_suffix,
                description_style,
                title_suffix_style,
            ));

            let mut lines = vec![Line::from(line_0)];

            if has_description || has_footer {
                let pad = if show_selected {
                    Span::styled("\u{258c}", Style::default().fg(theme.accent()))
                } else {
                    Span::raw(" ")
                };
                let mut description = vec![pad];
                if has_description {
                    description
                        .extend(search.highlight_spans(visible_description, description_style));
                }
                if has_footer {
                    description.extend(right_aligned_footer_spans(
                        description_width,
                        visible_description,
                        has_description,
                        footer_spans.clone(),
                    ));
                }
                lines.push(Line::from(description));
            }

            let height = lines.len() as u16;
            let item = Text::from(lines).style(Style::default().bg(background));
            (item, height)
        }
    });

    let list = ListView::new(builder, count);
    frame.render_stateful_widget(list, list_area, &mut state.list_state);

    if picker.protocol_type() != ratatui_image::picker::ProtocolType::Halfblocks {
        render_image_icons(frame, list_area, state, &filtered, picker);
    }

    let scrollbar_area = Rect {
        x: list_area.x + list_area.width.saturating_sub(0),
        y: list_area.y + 1,
        width: 1,
        height: list_area.height.saturating_sub(2),
    };
    frame.render_stateful_widget(
        Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("\u{25b2}"))
            .style(
                Style::default()
                    .fg(theme.text_dim())
                    .add_modifier(Modifier::BOLD),
            )
            .thumb_symbol("\u{2551}")
            .track_symbol(Some(""))
            .end_symbol(Some("\u{25bc}")),
        scrollbar_area,
        &mut state.scrollbar_state,
    );

    if let Some((pager_area, page_size)) = pagination {
        render_pager(frame, pager_area, state, count, page_size);
    }
}

fn pagination_layout(area: Rect, item_count: usize) -> (Rect, Option<(Rect, usize)>) {
    const ITEM_HEIGHT: u16 = 3;
    if area.height < ITEM_HEIGHT + 1 || item_count <= usize::from(area.height / ITEM_HEIGHT).max(1)
    {
        return (area, None);
    }
    let pager_area = Rect::new(area.x, area.bottom().saturating_sub(1), area.width, 1);
    let page_size = usize::from(area.height / ITEM_HEIGHT).max(1);
    (area, Some((pager_area, page_size)))
}

fn pager_pages(current: usize, page_count: usize) -> Vec<Option<usize>> {
    if page_count == 0 {
        return Vec::new();
    }
    let core_len = page_count.min(3);
    let current = current.min(page_count - 1);
    if current < 3 {
        return (0..page_count.min(4)).map(Some).collect();
    }
    let core_start = current.saturating_sub(1).min(page_count - core_len);
    let mut pages = Vec::with_capacity(5);
    if core_start > 0 {
        pages.push(Some(0));
        if core_start > 1 {
            pages.push(None);
        }
    }
    pages.extend((core_start..core_start + core_len).map(Some));
    pages
}

fn render_pager(
    frame: &mut Frame,
    area: Rect,
    state: &mut ContentListState,
    item_count: usize,
    page_size: usize,
) {
    let theme = THEME.as_ref();
    let page_count = item_count.div_ceil(page_size);
    let current = (state.list_state.selected.unwrap_or(0) / page_size).min(page_count - 1);
    let mut tokens = Vec::new();
    for page in pager_pages(current, page_count) {
        match page {
            Some(page) if page == current => tokens.push((
                format!("[{}]", page + 1),
                Some(page),
                Style::default()
                    .fg(theme.accent())
                    .add_modifier(Modifier::BOLD),
            )),
            Some(page) => tokens.push((
                format!(" {} ", page + 1),
                Some(page),
                Style::default().fg(theme.text()),
            )),
            None => tokens.push((
                " \u{2026} ".to_owned(),
                None,
                Style::default().fg(theme.text_dim()),
            )),
        }
    }

    let total_width = tokens
        .iter()
        .map(|(label, _, _)| Span::raw(label).width())
        .sum::<usize>();
    let pager_width = total_width.min(usize::from(area.width)) as u16;
    let start_x = area.x + area.width.saturating_sub(pager_width) / 2;
    let pager_area = Rect::new(start_x, area.y, pager_width, area.height);
    let mut x = start_x;
    let mut hits = Vec::new();
    let spans = tokens
        .into_iter()
        .map(|(label, page, style)| {
            let width = Span::raw(&label).width().min(usize::from(u16::MAX)) as u16;
            if let Some(page) = page
                && x < area.right()
            {
                hits.push((
                    Rect::new(x, area.y, width.min(area.right() - x), area.height),
                    page,
                ));
            }
            x = x.saturating_add(width);
            Span::styled(label, style)
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.border())),
        pager_area,
    );
    state.pagination = Some(PaginationState {
        page_size,
        page_count,
        hits,
    });
}

fn render_image_icons(
    frame: &mut Frame,
    area: Rect,
    state: &mut ContentListState,
    filtered: &[usize],
    picker: &ratatui_image::picker::Picker,
) {
    let truncation = state.list_state.scroll_truncation();
    let mut y = area.y;
    let first = state.list_state.scroll_offset_index();

    for (visible_index, &entry_index) in filtered.iter().enumerate().skip(first) {
        let Some(entry) = state.entries.get(entry_index) else {
            continue;
        };
        let icon_rows = entry.icon_lines.as_ref().map_or(0, Vec::len);
        let has_description = state
            .display_metadata
            .get(&entry.file_stem)
            .is_some_and(|metadata| metadata.has_description);
        let height = icon_rows.max(if has_description { 2 } else { 1 }) as u16;

        if y >= area.y + area.height {
            break;
        }
        let top_crop = if visible_index == first {
            truncation.min(icon_rows as u16)
        } else {
            0
        };
        let visible_icon_rows = (icon_rows as u16)
            .saturating_sub(top_crop)
            .min(area.y + area.height - y);
        if visible_icon_rows > 0
            && entry.icon_bytes.is_some()
            && let Some(protocol) = state.image_protocols.get_mut(&entry.file_stem)
        {
            let icon_area = Rect {
                x: area.x + 1,
                y,
                width: protocol_icon_columns(entry, picker).min(area.width.saturating_sub(1)),
                height: visible_icon_rows,
            };
            if icon_area.height > 0 && icon_area.width > 0 {
                let clipped = top_crop > 0 || visible_icon_rows < icon_rows as u16;
                let resize = if clipped {
                    Resize::Crop(Some(CropOptions {
                        clip_top: top_crop > 0,
                        clip_left: false,
                    }))
                } else {
                    Resize::Scale(None)
                };
                let widget: StatefulImage<StatefulProtocol> =
                    StatefulImage::default().resize(resize);
                frame.render_stateful_widget(widget, icon_area, protocol);
            }
        }
        let visible_height = if visible_index == first {
            height.saturating_sub(truncation)
        } else {
            height
        };
        y = y.saturating_add(visible_height);
        if visible_index + 1 >= filtered.len() {
            break;
        }
    }
}

// minecraft's 16-color palette, keyed by the formatting code character.
// these exact RGB values come from the minecraft wiki
fn mc_color(code: char) -> Option<Color> {
    match code {
        '0' => Some(Color::Rgb(0x00, 0x00, 0x00)),
        '1' => Some(Color::Rgb(0x00, 0x00, 0xAA)),
        '2' => Some(Color::Rgb(0x00, 0xAA, 0x00)),
        '3' => Some(Color::Rgb(0x00, 0xAA, 0xAA)),
        '4' => Some(Color::Rgb(0xAA, 0x00, 0x00)),
        '5' => Some(Color::Rgb(0xAA, 0x00, 0xAA)),
        '6' => Some(Color::Rgb(0xFF, 0xAA, 0x00)),
        '7' => Some(Color::Rgb(0xAA, 0xAA, 0xAA)),
        '8' => Some(Color::Rgb(0x55, 0x55, 0x55)),
        '9' => Some(Color::Rgb(0x55, 0x55, 0xFF)),
        'a' | 'A' => Some(Color::Rgb(0x55, 0xFF, 0x55)),
        'b' | 'B' => Some(Color::Rgb(0x55, 0xFF, 0xFF)),
        'c' | 'C' => Some(Color::Rgb(0xFF, 0x55, 0x55)),
        'd' | 'D' => Some(Color::Rgb(0xFF, 0x55, 0xFF)),
        'e' | 'E' => Some(Color::Rgb(0xFF, 0xFF, 0x55)),
        'f' | 'F' => Some(Color::Rgb(0xFF, 0xFF, 0xFF)),
        _ => None,
    }
}

fn searchable_spans(
    search: &crate::tui::widgets::search::SearchState,
    text: &str,
    base_style: Style,
    use_mc_colors: bool,
) -> Vec<Span<'static>> {
    if !search.is_empty() {
        search.highlight_spans(&strip_mc_codes(text), base_style)
    } else if use_mc_colors {
        parse_mc_text(text, base_style)
    } else {
        vec![Span::styled(strip_mc_codes(text), base_style)]
    }
}

// parses minecraft's section-sign (U+00A7) formatting codes into styled spans.
// handles colors (0-f), bold (l), strikethrough (m), underline (n), italic (o), reset (r)
fn parse_mc_text(text: &str, base_style: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut current_style = base_style;
    let mut current_text = String::new();
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{00A7}'
            && let Some(&code) = chars.peek()
        {
            if !current_text.is_empty() {
                spans.push(Span::styled(current_text.clone(), current_style));
                current_text.clear();
            }
            chars.next();

            if let Some(color) = mc_color(code) {
                current_style = base_style.fg(color);
            } else {
                match code {
                    'l' | 'L' => {
                        current_style = current_style.add_modifier(Modifier::BOLD);
                    }
                    'm' | 'M' => {
                        current_style = current_style.add_modifier(Modifier::CROSSED_OUT);
                    }
                    'n' | 'N' => {
                        current_style = current_style.add_modifier(Modifier::UNDERLINED);
                    }
                    'o' | 'O' => {
                        current_style = current_style.add_modifier(Modifier::ITALIC);
                    }
                    'r' | 'R' => {
                        current_style = base_style;
                    }
                    _ => {}
                }
            }
            continue;
        }
        current_text.push(ch);
    }

    if !current_text.is_empty() {
        spans.push(Span::styled(current_text, current_style));
    }

    spans
}

fn strip_mc_codes(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{00A7}' {
            chars.next();
        } else {
            result.push(ch);
        }
    }
    result
}

fn display_metadata(entry: &ContentEntry) -> DisplayMetadata {
    let description = strip_mc_codes(&entry.description);
    let description = description.trim().to_string();
    DisplayMetadata {
        has_description: !description.is_empty(),
        description,
    }
}

fn preserve_visual_metadata(entry: &mut ContentEntry, previous: &mut ContentEntry) {
    if entry.icon_bytes.is_none() {
        entry.icon_bytes = previous.icon_bytes.take();
        entry.icon_lines = previous.icon_lines.take();
        entry.provider_icon = previous.provider_icon;
    }
    if entry.description.trim().is_empty() && previous.provider_description {
        entry.description = std::mem::take(&mut previous.description);
        entry.provider_description = true;
    }
    if entry.provider_project.is_none() {
        entry.provider_project = previous.provider_project.clone();
    }
}

// renders one row of a mod icon using half-block characters (U+2584).
// each cell packs two vertical pixels via fg/bg colors, giving
// double the vertical resolution out of the terminal
fn icon_spans(
    icon_pixels: Option<&Vec<Vec<IconCell>>>,
    row: usize,
    use_image_protocol: bool,
    protocol_columns: usize,
) -> Vec<Span<'static>> {
    if use_image_protocol {
        return vec![Span::raw(" ".repeat(protocol_columns))];
    }
    match icon_pixels.and_then(|rows| rows.get(row)) {
        Some(cols) => cols
            .iter()
            .map(|cell| {
                Span::styled(
                    cell.symbol.to_string(),
                    Style::default()
                        .fg(Color::Rgb(cell.fg_r, cell.fg_g, cell.fg_b))
                        .bg(Color::Rgb(cell.bg_r, cell.bg_g, cell.bg_b)),
                )
            })
            .collect(),
        None => {
            let theme = THEME.as_ref();
            vec![Span::styled(
                "      ",
                Style::default().fg(theme.text_dim()),
            )]
        }
    }
}

fn world_game_mode_color(mode: WorldGameMode) -> Color {
    let theme = THEME.as_ref();
    match mode {
        WorldGameMode::Survival => theme.success(),
        WorldGameMode::Creative => theme.info(),
        WorldGameMode::Adventure => theme.warning(),
        WorldGameMode::Spectator => theme.text_dim(),
        WorldGameMode::Hardcore => theme.error(),
    }
}

fn world_descriptions(details: &WorldDetails) -> Vec<String> {
    let summary = match (&details.minecraft_version, &details.size) {
        (Some(version), Some(size)) => format!("{version}  •  {size}"),
        (Some(version), None) => version.clone(),
        (None, Some(size)) => size.clone(),
        (None, None) => String::new(),
    };
    let mut lines = Vec::with_capacity(5);
    if !summary.is_empty() {
        lines.push(summary);
    }
    lines.extend(
        details
            .datapacks
            .iter()
            .take(3)
            .map(|name| format!("  • {name}")),
    );
    if details.datapacks.len() > 3 {
        lines.push(format!("  +{} more", details.datapacks.len() - 3));
    }
    lines
}

fn title_suffix_spans(
    suffix: Option<&str>,
    spacing_style: Style,
    label_style: Style,
) -> Vec<Span<'static>> {
    suffix.map_or_else(Vec::new, |suffix| {
        vec![
            Span::styled("  ", spacing_style),
            Span::styled(format!(" {suffix} "), label_style),
        ]
    })
}

fn available_description_width(
    row_width: usize,
    rendered_icon_columns: usize,
    has_icon: bool,
) -> usize {
    let selector_and_scrollbar = 2;
    let icon_and_gap = if has_icon {
        rendered_icon_columns + 1
    } else {
        0
    };
    row_width.saturating_sub(selector_and_scrollbar + icon_and_gap)
}

fn description_text_width(
    available_width: usize,
    footer_width: usize,
    has_description: bool,
) -> usize {
    available_width.saturating_sub(footer_width + usize::from(has_description && footer_width > 0))
}

fn right_aligned_footer_spans(
    available_width: usize,
    description: &str,
    has_description: bool,
    footer: Vec<Span<'static>>,
) -> Vec<Span<'static>> {
    let description_width = if has_description {
        Span::raw(description).width()
    } else {
        0
    };
    let footer_width = footer.iter().map(Span::width).sum::<usize>();
    let padding = available_width.saturating_sub(description_width + footer_width);
    let mut spans = vec![Span::raw(" ".repeat(padding))];
    spans.extend(footer);
    spans
}

fn version_change_spans(from: &str, to: &str) -> Vec<Span<'static>> {
    let theme = THEME.as_ref();
    let old = Style::default()
        .fg(theme.background())
        .bg(theme.text_dim())
        .add_modifier(Modifier::BOLD);
    let new = Style::default()
        .fg(theme.background())
        .bg(theme.accent())
        .add_modifier(Modifier::BOLD);
    vec![
        Span::styled(format!(" {from} "), old),
        Span::styled(
            "  ➜  ",
            Style::default()
                .fg(theme.text())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {to} "), new),
    ]
}

fn ellipsize(text: &str, max_width: usize) -> String {
    if Span::raw(text).width() <= max_width {
        return text.to_owned();
    }
    if max_width <= 3 {
        return ".".repeat(max_width);
    }

    let content_width = max_width - 3;
    let mut visible = String::new();
    for character in text.chars() {
        visible.push(character);
        if Span::raw(visible.as_str()).width() > content_width {
            visible.pop();
            break;
        }
    }
    visible = visible.trim_end().to_owned();
    visible.push_str("...");
    visible
}

fn protocol_icon_columns(entry: &ContentEntry, picker: &ratatui_image::picker::Picker) -> u16 {
    let rows = entry.icon_lines.as_ref().map_or(3, Vec::len) as u16;
    let font_size = picker.font_size();
    square_icon_columns(rows, (font_size.width, font_size.height))
}

fn square_icon_columns(rows: u16, font_size: (u16, u16)) -> u16 {
    let width = u32::from(font_size.0.max(1));
    let height = u32::from(font_size.1.max(1));
    ((u32::from(rows) * height + width / 2) / width).max(1) as u16
}

#[derive(Debug, PartialEq, Eq)]
enum WatcherEventHandling {
    Ignore,
    Paths,
    Rescan,
}

fn watcher_event_handling(kind: &notify::EventKind) -> WatcherEventHandling {
    match kind {
        notify::EventKind::Access(_) => WatcherEventHandling::Ignore,
        notify::EventKind::Create(_) | notify::EventKind::Remove(_) => WatcherEventHandling::Paths,
        notify::EventKind::Modify(notify::event::ModifyKind::Name(_)) => {
            WatcherEventHandling::Rescan
        }
        notify::EventKind::Modify(_) => WatcherEventHandling::Paths,
        notify::EventKind::Any | notify::EventKind::Other => WatcherEventHandling::Rescan,
    }
}

// reads a content directory and builds a stem -> (path, enabled) map.
// used both by watch_dir to initialize known state and by the watcher
// thread to detect changes. when ext is empty (worlds), only directories
// are included.
fn diff_directory(
    dir: &std::path::Path,
    ext: &str,
    scan_one: Option<ScanOneFn>,
    known: &Arc<Mutex<HashMap<String, (std::path::PathBuf, bool)>>>,
) -> Option<WatcherDiff> {
    let on_disk = read_dir_stems(dir, ext);
    let mut known_map = known.lock().ok()?;
    let mut toggled = Vec::new();
    let mut removed = Vec::new();
    let mut added = Vec::new();
    for (stem, (old_path, old_enabled)) in known_map.iter() {
        if let Some((disk_path, disk_enabled)) = on_disk.get(stem) {
            if disk_enabled != old_enabled || disk_path != old_path {
                toggled.push((stem.clone(), *disk_enabled, disk_path.clone()));
            }
        } else {
            removed.push(stem.clone());
        }
    }
    for (stem, (path, enabled)) in &on_disk {
        if !known_map.contains_key(stem)
            && let Some(scan_one) = scan_one
        {
            added.push(scan_one(path, stem, *enabled));
        }
    }
    *known_map = on_disk;
    watcher_diff(toggled, removed, added)
}

fn diff_event_paths(
    dir: &std::path::Path,
    paths: &[std::path::PathBuf],
    ext: &str,
    scan_one: Option<ScanOneFn>,
    known: &Arc<Mutex<HashMap<String, (std::path::PathBuf, bool)>>>,
) -> Option<WatcherDiff> {
    let mut known_map = known.lock().ok()?;
    let mut toggled = Vec::new();
    let mut removed = Vec::new();
    let mut added = Vec::new();
    for path in paths {
        if path.parent() != Some(dir) {
            continue;
        }
        if path.exists() {
            let Some((stem, enabled)) = watched_stem(path, ext) else {
                continue;
            };
            match known_map.get(&stem) {
                Some((known_path, known_enabled))
                    if known_path == path && *known_enabled == enabled =>
                {
                    // A modify event still needs a rescan so changed archive
                    // metadata and icons become visible.
                    if let Some(scan_one) = scan_one {
                        removed.push(stem.clone());
                        added.push(scan_one(path, &stem, enabled));
                    }
                }
                Some(_) => {
                    toggled.push((stem.clone(), enabled, path.clone()));
                }
                None => {
                    if let Some(scan_one) = scan_one {
                        added.push(scan_one(path, &stem, enabled));
                    }
                }
            }
            known_map.insert(stem, (path.clone(), enabled));
        } else {
            let removed_stems = known_map
                .iter()
                .filter(|(_, (known_path, _))| known_path == path)
                .map(|(stem, _)| stem.clone())
                .collect::<Vec<_>>();
            for stem in removed_stems {
                known_map.remove(&stem);
                removed.push(stem);
            }
        }
    }
    watcher_diff(toggled, removed, added)
}

fn watcher_diff(
    toggled: Vec<(String, bool, std::path::PathBuf)>,
    removed: Vec<String>,
    added: Vec<ContentEntry>,
) -> Option<WatcherDiff> {
    if toggled.is_empty() && removed.is_empty() && added.is_empty() {
        None
    } else {
        Some(WatcherDiff {
            toggled,
            removed,
            added,
        })
    }
}

fn merge_watcher_diff(pending: &mut WatcherDiff, mut next: WatcherDiff) {
    pending.toggled.append(&mut next.toggled);
    pending.removed.append(&mut next.removed);
    pending.added.append(&mut next.added);

    pending.toggled.reverse();
    pending.toggled.sort_by(|left, right| left.0.cmp(&right.0));
    pending.toggled.dedup_by(|left, right| left.0 == right.0);
    pending.removed.sort();
    pending.removed.dedup();
    pending.added.reverse();
    pending
        .added
        .sort_by(|left, right| left.file_stem.cmp(&right.file_stem));
    pending
        .added
        .dedup_by(|left, right| left.file_stem == right.file_stem);
}

fn watched_stem(path: &std::path::Path, ext: &str) -> Option<(String, bool)> {
    let name = path.file_name()?.to_str()?;
    if path.is_dir() || ext.is_empty() {
        let (enabled, stem) = crate::instance::content::parse_enabled_stem_dir(name);
        return Some((stem, enabled));
    }
    let disabled_ext = format!("{ext}.disabled");
    if let Some(stem) = name.strip_suffix(&disabled_ext) {
        Some((stem.to_owned(), false))
    } else {
        name.strip_suffix(ext).map(|stem| (stem.to_owned(), true))
    }
}

fn opposite_toggle_path(path: &std::path::Path, enabled: bool) -> std::path::PathBuf {
    let mut opposite = path.to_owned();
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return opposite;
    };
    if enabled {
        opposite.set_file_name(format!("{name}.disabled"));
    } else if let Some(name) = name.strip_suffix(".disabled") {
        opposite.set_file_name(name);
    }
    opposite
}

fn read_dir_stems(dir: &std::path::Path, ext: &str) -> HashMap<String, (std::path::PathBuf, bool)> {
    let mut map = HashMap::new();
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return map;
    };
    let dirs_only = ext.is_empty();
    let disabled_ext = format!("{ext}.disabled");

    for dir_entry in read_dir.flatten() {
        let path = dir_entry.path();
        let Some(fname) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if dirs_only {
            if !path.is_dir() && !fname.ends_with(".disabled") {
                continue;
            }
            let (enabled, stem) = crate::instance::content::parse_enabled_stem_dir(fname);
            map.insert(stem, (path, enabled));
            continue;
        }
        if let Some(stem) = fname.strip_suffix(&disabled_ext) {
            map.insert(stem.to_owned(), (path, false));
        } else if let Some(stem) = fname.strip_suffix(ext) {
            map.insert(stem.to_owned(), (path, true));
        } else if path.is_dir() {
            let (enabled, stem) = crate::instance::content::parse_enabled_stem_dir(fname);
            map.insert(stem, (path, enabled));
        }
    }

    map
}

#[cfg(test)]
#[path = "../../tests/widgets/content/list.rs"]
mod tests;
