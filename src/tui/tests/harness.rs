// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::{HashMap, HashSet};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::{Terminal, backend::TestBackend};

use crate::auth::{Account, AccountStore, AccountType};
use crate::instance::{InstanceConfig, InstanceManager, ModLoader};
use crate::tests::TEST_LOCK;
use crate::tui::{
    app::{App, FocusedArea},
    widgets,
};

pub(in crate::tui) struct UiHarness {
    pub app: App,
    terminal: Terminal<TestBackend>,
    runtime: tokio::runtime::Runtime,
    _temp: tempfile::TempDir,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl UiHarness {
    pub fn new() -> Self {
        let guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        widgets::popups::confirm::clear_pending();
        crate::tui::app::FAILED_INSTANCE_SETTINGS_UPDATES
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        crate::tui::app::COMPLETED_INSTANCE_SETTINGS_UPDATES
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        crate::tui::app::PENDING_INSTANCES
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
        widgets::popups::new_instance::reset_for_test();
        widgets::popups::import_modpack::reset_for_test();
        crate::feedback::errors::ERROR_EVENTS
            .lock()
            .expect("error buffer")
            .clear();
        crate::feedback::progress::clear();

        let temp = tempfile::tempdir().expect("temporary UI data");
        let instances_dir = temp.path().join("instances");
        let meta_dir = temp.path().join("meta");
        std::fs::create_dir_all(&instances_dir).expect("instances directory");
        std::fs::create_dir_all(&meta_dir).expect("metadata directory");

        let picker = ratatui_image::picker::Picker::halfblocks();
        let font_size = picker.font_size();
        let instance_manager = InstanceManager::new(instances_dir, meta_dir.clone());
        let account_state = widgets::account::AccountState {
            store: AccountStore::empty_for_test(temp.path().join("accounts.json")),
            list_state: Default::default(),
            add_mode: widgets::account::AddMode::None,
        };

        let app = App {
            exit: false,
            focused: FocusedArea::default(),
            pre_overlay_focused: FocusedArea::default(),
            content_tab: widgets::content::ContentTab::default(),
            content_mode: widgets::content::ContentMode::default(),
            instances_state: widgets::instances::State::default(),
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
            worlds_state: widgets::content::ContentListState::default(),
            world_datapacks_state: widgets::content::ContentListState::default(),
            open_world_datapacks: None,
            world_quick_play_support: None,
            screenshots_state: {
                let mut state = widgets::screenshots_grid::ScreenshotsState::default();
                state.font_size = (font_size.width, font_size.height);
                state
            },
            logs_state: widgets::logs_viewer::LogsState::default(),
            account_state,
            settings_state: widgets::settings::SettingsState::new(meta_dir),
            instance_settings: None,
            pending_instance_settings_updates: Default::default(),
            global_settings: None,
            picker,
            detected_image_protocol: ratatui_image::picker::ProtocolType::Halfblocks,
            instance_manager,
            log_overlay_scroll: 0,
            log_overlay_max_scroll: 0,
            log_overlay_search: widgets::search::SearchState::default(),
            log_overlay_scrollbar: ratatui::widgets::ScrollbarState::default(),
            throbber_state: throbber_widgets_tui::ThrobberState::default(),
            throbber_tick: 0,
            error_effects: HashMap::new(),
            pending_editor: None,
            reconciliation_for: None,
            content_manifest: None,
            content_update_snapshot: None,
            content_update_popup: None,
            modpack_versions_state: None,
            modpack_update_popup: None,
            provider_conflict: None,
            dismissed_provider_conflicts: HashSet::new(),
        };

        Self {
            app,
            terminal: Terminal::new(TestBackend::new(100, 30)).expect("test terminal"),
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime"),
            _temp: temp,
            _guard: guard,
        }
    }

    pub fn add_instance(&mut self, name: &str) {
        let config = InstanceConfig {
            name: name.to_owned(),
            game_version: "1.21.1".to_owned(),
            loader: ModLoader::Fabric,
            loader_version: Some("0.16.14".to_owned()),
            created: chrono::Utc::now(),
            last_played: None,
            java_path: None,
            memory_max: None,
            memory_min: None,
            jvm_args: Vec::new(),
            environment: Default::default(),
            window_mode: Default::default(),
            inherit_window_mode: false,
            resolution: None,
            inherit_resolution: false,
            preferred_account: None,
            pre_launch_command: Default::default(),
            post_exit_command: Default::default(),
            glfw_path: None,
            config_sync_profile: None,
            modpack_source: None,
        };
        std::fs::create_dir_all(self.instance_path(name)).expect("instance directory");
        self.app
            .instance_manager
            .save(&config)
            .expect("instance config");
        self.app.instances_state.add_instance(config);
    }

    pub fn add_account(&mut self, username: &str) {
        self.app.account_state.store.accounts.push(Account {
            uuid: username.to_owned(),
            username: username.to_owned(),
            account_type: AccountType::Microsoft,
            active: true,
            refresh_token: Some("refresh".to_owned()),
            cached_mc_token: None,
            cached_mc_token_expires_at: None,
        });
        self.app.account_state.list_state.selected = Some(0);
    }

    pub fn instance_path(&self, name: &str) -> std::path::PathBuf {
        self.app.instance_manager.instances_dir.join(name)
    }

    pub fn key(&mut self, code: KeyCode) {
        self.key_with(code, KeyModifiers::NONE);
    }

    pub fn key_with(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let _runtime = self.runtime.enter();
        self.app
            .handle_key_event(KeyEvent::new(code, modifiers))
            .expect("handle key");
    }

    pub fn mouse(&mut self, kind: MouseEventKind) {
        let _runtime = self.runtime.enter();
        self.app.handle_mouse_event(MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
    }

    pub fn draw(&mut self) {
        let _runtime = self.runtime.enter();
        let app = &mut self.app;
        self.terminal
            .draw(|frame| app.render_frame(frame))
            .expect("draw UI");
    }

    pub fn screen(&self) -> String {
        self.terminal.backend().to_string()
    }
}
