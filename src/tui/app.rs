// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};

use tachyonfx::Effect;

use super::widgets::{self, instances};
use crate::instance::{InstanceConfig, InstanceManager};

pub(super) static PENDING_INSTANCES: LazyLock<Arc<Mutex<Vec<InstanceConfig>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(Vec::new())));
pub(super) static COMPLETED_INSTANCE_SETTINGS_UPDATES: LazyLock<Arc<Mutex<Vec<InstanceConfig>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(Vec::new())));
pub(super) static FAILED_INSTANCE_SETTINGS_UPDATES: LazyLock<Arc<Mutex<Vec<String>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(Vec::new())));
pub(super) const RUNTIME_UPDATE_PENDING_MESSAGE: &str = "Wait for the runtime update to finish";

pub struct App {
    pub(super) exit: bool,
    pub(super) focused: FocusedArea,
    pub(super) pre_overlay_focused: FocusedArea,
    pub(super) content_tab: widgets::content::ContentTab,
    pub(super) content_mode: widgets::content::ContentMode,
    pub(super) instances_state: instances::State,
    pub(super) mods_state: widgets::content::list::ContentListState,
    pub(super) mods_discovery_state: widgets::content::DiscoveryState,
    pub(super) resource_packs_state: widgets::content::list::ContentListState,
    pub(super) resource_packs_discovery_state: widgets::content::DiscoveryState,
    pub(super) shaders_state: widgets::content::list::ContentListState,
    pub(super) shaders_discovery_state: widgets::content::DiscoveryState,
    pub(super) datapacks_discovery_state: widgets::content::DiscoveryState,
    pub(super) worlds_state: widgets::content::list::ContentListState,
    pub(super) world_datapacks_state: widgets::content::list::ContentListState,
    pub(super) open_world_datapacks: Option<(String, PathBuf)>,
    pub(super) world_quick_play_support: Option<(String, String, bool)>,
    pub(super) screenshots_state: widgets::screenshots_grid::ScreenshotsState,
    pub(super) logs_state: widgets::logs_viewer::LogsState,
    pub(super) account_state: widgets::account::AccountState,
    pub(super) settings_state: widgets::settings::SettingsState,
    pub(super) instance_settings: Option<widgets::popups::instance_settings::State>,
    pub(super) export_popup: Option<widgets::popups::export_modpack::State>,
    pub(super) pending_instance_settings_updates: HashSet<String>,
    pub(super) global_settings: Option<widgets::popups::global_settings::State>,
    pub(super) picker: ratatui_image::picker::Picker,
    pub(super) detected_image_protocol: ratatui_image::picker::ProtocolType,
    pub(super) instance_manager: InstanceManager,
    pub(super) log_overlay_scroll: usize,
    pub(super) log_overlay_max_scroll: usize,
    pub(super) log_filter_open: bool,
    pub(super) log_filter_selected: usize,
    pub(super) log_level_filters: widgets::logs_viewer::LevelFilters,
    pub(super) log_selection: widgets::log_selection::LogSelection,
    pub(super) log_overlay_inner: ratatui::layout::Rect,
    pub(super) log_overlay_search: widgets::search::SearchState,
    pub(super) log_overlay_scrollbar: ratatui::widgets::ScrollbarState,
    pub(super) throbber_state: throbber_widgets_tui::ThrobberState,
    pub(super) throbber_tick: u8,
    pub(super) error_effects: HashMap<u64, ErrorEffectState>,
    pub(super) pending_editor: Option<std::path::PathBuf>,
    pub(super) edited_config_watches: Vec<super::event::EditedConfigWatch>,
    pub(super) content_for: Option<InstanceContentKey>,
    pub(super) cached_instance_content: HashMap<InstanceContentKey, CachedInstanceContent>,
    pub(super) reconciliation_for: Option<(String, chrono::DateTime<chrono::Utc>)>,
    pub(super) content_manifest: Option<(String, crate::instance::ContentManifest)>,
    pub(super) content_update_snapshot:
        Option<(String, crate::instance::content::updates::UpdateSnapshot)>,
    pub(super) content_update_check_pending: bool,
    pub(super) content_update_popup: Option<widgets::content::update::State>,
    pub(super) modpack_versions_state: Option<widgets::content::DiscoveryState>,
    pub(super) modpack_update_popup: Option<widgets::popups::modpack_update::State>,
    pub(super) provider_conflict: Option<ProviderConflictState>,
    pub(super) dismissed_provider_conflicts: HashSet<PathBuf>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct InstanceContentKey {
    pub root: PathBuf,
    pub name: String,
    pub created: chrono::DateTime<chrono::Utc>,
    pub game_version: String,
    pub loader: crate::instance::ModLoader,
}

#[derive(Default)]
pub(super) struct CachedInstanceContent {
    mods: widgets::content::ContentListState,
    resource_packs: widgets::content::ContentListState,
    shaders: widgets::content::ContentListState,
    worlds: widgets::content::ContentListState,
    world_datapacks: widgets::content::ContentListState,
    open_world_datapacks: Option<(String, PathBuf)>,
    screenshots: widgets::screenshots_grid::ScreenshotsState,
    logs: widgets::logs_viewer::LogsState,
    pub(super) reconciliation_for: Option<(String, chrono::DateTime<chrono::Utc>)>,
    manifest: Option<(String, crate::instance::ContentManifest)>,
    updates: Option<(String, crate::instance::content::updates::UpdateSnapshot)>,
    pub(super) update_check_pending: bool,
    dismissed_provider_conflicts: HashSet<PathBuf>,
}

impl CachedInstanceContent {
    pub fn swap(&mut self, app: &mut App) {
        std::mem::swap(&mut self.mods, &mut app.mods_state);
        std::mem::swap(&mut self.resource_packs, &mut app.resource_packs_state);
        std::mem::swap(&mut self.shaders, &mut app.shaders_state);
        std::mem::swap(&mut self.worlds, &mut app.worlds_state);
        std::mem::swap(&mut self.world_datapacks, &mut app.world_datapacks_state);
        std::mem::swap(
            &mut self.open_world_datapacks,
            &mut app.open_world_datapacks,
        );
        std::mem::swap(&mut self.screenshots, &mut app.screenshots_state);
        std::mem::swap(&mut self.logs, &mut app.logs_state);
        std::mem::swap(&mut self.reconciliation_for, &mut app.reconciliation_for);
        std::mem::swap(&mut self.manifest, &mut app.content_manifest);
        std::mem::swap(&mut self.updates, &mut app.content_update_snapshot);
        std::mem::swap(
            &mut self.update_check_pending,
            &mut app.content_update_check_pending,
        );
        std::mem::swap(
            &mut self.dismissed_provider_conflicts,
            &mut app.dismissed_provider_conflicts,
        );
    }
}

pub(super) struct ProviderConflictState {
    pub relative_path: PathBuf,
    pub candidates: Vec<crate::instance::ProviderProject>,
    pub selected: usize,
}

pub(super) enum ErrorEffectState {
    SlidingIn(Effect, std::time::Instant),
    Idle,
    FadingOut(Effect, std::time::Instant),
}

#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub enum FocusedArea {
    #[default]
    Instances,
    Content,
    Account,
    Settings,
    Overview,
    OverviewExpanded,
    Popup,
    ImportPopup,
    ErrorPopup,
    ConfirmDelete,
    InstanceSettings,
    GlobalSettings,
}

impl App {
    pub(super) fn selected_instance_supports_quick_play(&mut self) -> bool {
        let Some((name, game_version)) = self
            .instances_state
            .selected_instance()
            .map(|instance| (instance.name.clone(), instance.game_version.clone()))
        else {
            return false;
        };
        if let Some((cached_name, cached_version, supported)) = &self.world_quick_play_support
            && cached_name == &name
            && cached_version == &game_version
        {
            return *supported;
        }
        let supported = crate::instance::launch::supports_quick_play(
            &self.instance_manager.meta_dir,
            &game_version,
        );
        self.world_quick_play_support = Some((name, game_version, supported));
        supported
    }

    pub fn new(
        picker: ratatui_image::picker::Picker,
        detected_image_protocol: ratatui_image::picker::ProtocolType,
    ) -> Self {
        let instances_dir = crate::config::SETTINGS.read().paths.resolve_instances_dir();
        let meta_dir = crate::config::SETTINGS.read().paths.resolve_meta_dir();

        let _ = std::fs::create_dir_all(&instances_dir);
        let _ = std::fs::create_dir_all(&meta_dir);
        crate::instance::import::refresh::recover_interrupted(&instances_dir);

        let manager = InstanceManager::new(instances_dir, meta_dir);
        let settings_state = widgets::settings::SettingsState::new(
            manager.meta_dir.clone(),
            manager.instances_dir.clone(),
        );
        let instances = manager.load_all();
        instances::spawn_modpack_update_checks(&instances);
        let instances_state = instances::State::with_instances(instances);

        App {
            exit: false,
            focused: FocusedArea::default(),
            pre_overlay_focused: FocusedArea::default(),
            content_tab: widgets::content::ContentTab::default(),
            content_mode: widgets::content::ContentMode::default(),
            instances_state,
            mods_state: widgets::content::ContentListState::default(),
            mods_discovery_state: widgets::content::DiscoveryState::new(
                crate::instance::ContentKind::Mod,
            ),
            resource_packs_state: widgets::content::ContentListState::default(),
            resource_packs_discovery_state: widgets::content::DiscoveryState::new(
                crate::instance::ContentKind::ResourcePack,
            ),
            shaders_state: widgets::content::ContentListState::default(),
            shaders_discovery_state: widgets::content::DiscoveryState::new(
                crate::instance::ContentKind::Shader,
            ),
            datapacks_discovery_state: widgets::content::DiscoveryState::new(
                crate::instance::ContentKind::DataPack,
            ),
            worlds_state: widgets::content::list::ContentListState::default(),
            world_datapacks_state: widgets::content::ContentListState::default(),
            open_world_datapacks: None,
            world_quick_play_support: None,
            logs_state: widgets::logs_viewer::LogsState::default(),
            account_state: widgets::account::AccountState::default(),
            settings_state,
            instance_settings: None,
            export_popup: None,
            pending_instance_settings_updates: HashSet::new(),
            global_settings: None,
            screenshots_state: {
                let mut s = widgets::screenshots_grid::ScreenshotsState::default();
                let font_size = picker.font_size();
                s.font_size = (font_size.width, font_size.height);
                s
            },
            picker,
            detected_image_protocol,
            instance_manager: manager,
            log_overlay_scroll: 0,
            log_overlay_max_scroll: 0,
            log_filter_open: false,
            log_filter_selected: 0,
            log_level_filters: [None; 5],
            log_selection: widgets::log_selection::LogSelection::default(),
            log_overlay_inner: ratatui::layout::Rect::default(),
            log_overlay_search: widgets::search::SearchState::default(),
            log_overlay_scrollbar: ratatui::widgets::ScrollbarState::default(),
            throbber_state: throbber_widgets_tui::ThrobberState::default(),
            throbber_tick: 0,
            error_effects: HashMap::new(),
            pending_editor: None,
            edited_config_watches: Vec::new(),
            content_for: None,
            cached_instance_content: HashMap::new(),
            reconciliation_for: None,
            content_manifest: None,
            content_update_snapshot: None,
            content_update_check_pending: false,
            content_update_popup: None,
            modpack_versions_state: None,
            modpack_update_popup: None,
            provider_conflict: None,
            dismissed_provider_conflicts: HashSet::new(),
        }
    }

    pub(super) fn into_picker(self) -> ratatui_image::picker::Picker {
        self.picker
    }
}
