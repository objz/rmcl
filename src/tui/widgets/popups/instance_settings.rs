// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// modal editor for settings belonging to the selected Minecraft instance.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, ListItem, Paragraph},
};
use ratatui_textarea::TextArea;
use std::sync::{Arc, Mutex};

use crate::{
    auth::{Account, AccountType},
    config::{
        SETTINGS,
        theme::{BORDER_STYLE, THEME},
    },
    instance::loader::GameVersion,
    instance::models::{
        InstanceConfig, LaunchCommand, ModLoader, memory_kib, normalize_memory_value,
        parse_resolution,
    },
    tui::widgets::{
        popups::{
            LoadState,
            settings_controls::{
                DisplayResolution, GlfwChoice, GlfwPicker, JavaChoice, JavaPicker,
                ResolutionChoice, ResolutionPickerAction, SettingsPicker, SettingsPickerAction,
                SettingsPickerBadge, SettingsPickerOption, adjust_memory, auto_label,
                bundled_glfw_version, bundled_label as bundled_badge, default_label,
                default_resolution, display_resolutions, environment_labels, format_tag_values,
                handle_resolution_picker_key, handle_text_area_input, is_default_resolution,
                parse_environment, parse_tag_values, render_memory_gauge, render_settings_picker,
                resolution_choices, resolution_items, settings_text_area, tagged_row_count,
                tagged_value_lines, toggle_window_mode, window_mode_title,
            },
        },
        search::SearchState,
        status_badge,
    },
};

const FIELD_COUNT: usize = 15;
const FIELD_ORDER: [usize; FIELD_COUNT] = [0, 1, 2, 10, 3, 4, 5, 6, 7, 14, 8, 9, 11, 12, 13];
type SharedLoad<T> = Arc<Mutex<LoadState<T>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VersionPicker {
    Game,
    Loader,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChoicePicker {
    Loader,
    Java,
    Resolution,
    Account,
    Glfw,
}

enum PickerLoad {
    Idle,
    Loading,
    Loaded,
    Error,
}

#[derive(Debug, Clone)]
pub struct State {
    original: InstanceConfig,
    pub draft: InstanceConfig,
    selected: usize,
    editing: Option<TextArea<'static>>,
    pub desktop: bool,
    original_desktop: bool,
    error: Option<String>,
    picker: Option<VersionPicker>,
    picker_index: usize,
    picker_initialized: bool,
    picker_search: SearchState,
    show_snapshots: bool,
    game_versions: SharedLoad<Vec<GameVersion>>,
    loader_versions: SharedLoad<Vec<String>>,
    choice_picker: Option<ChoicePicker>,
    choice_index: usize,
    java_picker: JavaPicker,
    glfw_picker: GlfwPicker,
    account_picker: SettingsPicker,
    accounts: Vec<Account>,
    meta_dir: std::path::PathBuf,
    display_resolutions: Vec<DisplayResolution>,
    runtime_update_pending: bool,
    save_retry_pending: bool,
}

pub enum Action {
    None,
    Save(Box<InstanceConfig>, bool),
    Error(String),
    Warning(String),
    ConfirmRuntime { name: String },
    ConfirmJavaAuto { name: String },
    ConfirmAccountAuto { name: String },
    OpenRaw,
    Close,
}

impl State {
    pub fn new(instance: &InstanceConfig, meta_dir: &std::path::Path) -> Self {
        Self::with_accounts(
            instance,
            meta_dir,
            crate::auth::AccountStore::load().accounts,
        )
    }

    pub fn with_accounts(
        instance: &InstanceConfig,
        meta_dir: &std::path::Path,
        accounts: Vec<Account>,
    ) -> Self {
        let auto_java_path = SETTINGS
            .read()
            .paths
            .effective_java_path()
            .map(str::to_owned)
            .unwrap_or_else(crate::instance::java::detect_java_path);
        let java_cache = crate::storage::MetadataPaths::new(meta_dir).java_installations();
        let mut java_picker = JavaPicker::with_cache(auto_java_path, Some(java_cache));
        java_picker.set_current(instance.java_path.as_deref());
        let desktop = crate::instance::desktop::exists(&instance.name);
        Self {
            original: instance.clone(),
            draft: instance.clone(),
            selected: 0,
            editing: None,
            desktop,
            original_desktop: desktop,
            error: None,
            picker: None,
            picker_index: 0,
            picker_initialized: false,
            picker_search: SearchState::default(),
            show_snapshots: false,
            game_versions: Arc::new(Mutex::new(LoadState::Idle)),
            loader_versions: Arc::new(Mutex::new(LoadState::Idle)),
            choice_picker: None,
            choice_index: 0,
            java_picker,
            glfw_picker: GlfwPicker::with_bundled_version(bundled_glfw_version(
                meta_dir,
                &instance.game_version,
            )),
            account_picker: SettingsPicker::default(),
            accounts,
            meta_dir: meta_dir.to_path_buf(),
            display_resolutions: display_resolutions(),
            runtime_update_pending: false,
            save_retry_pending: false,
        }
    }

    fn dirty(&self) -> bool {
        self.draft != self.original || self.desktop != self.original_desktop
    }

    fn runtime_changed(&self) -> bool {
        self.draft.game_version != self.original.game_version
            || self.draft.loader != self.original.loader
            || self.draft.loader_version != self.original.loader_version
    }

    fn validate_before_save(&mut self) -> bool {
        self.error = None;
        let settings = SETTINGS.read();
        if self.draft.game_version.trim().is_empty() {
            self.error = Some("Game version is required.".to_owned());
        } else if self.draft.loader != ModLoader::Vanilla
            && self
                .draft
                .loader_version
                .as_deref()
                .is_none_or(str::is_empty)
        {
            self.error = Some("Select a loader version.".to_owned());
        } else if let (Some(min), Some(max)) = (
            memory_kib(
                self.draft
                    .memory_min
                    .as_deref()
                    .unwrap_or(&settings.defaults.memory_min),
            ),
            memory_kib(
                self.draft
                    .memory_max
                    .as_deref()
                    .unwrap_or(&settings.defaults.memory_max),
            ),
        ) && min > max
        {
            self.error = Some("Minimum memory cannot exceed maximum memory.".to_owned());
        } else if self.draft.pre_launch_command.enabled
            && self.draft.pre_launch_command.command.trim().is_empty()
        {
            self.error = Some("Enter a pre-launch command before enabling it.".to_owned());
        } else if self.draft.post_exit_command.enabled
            && self.draft.post_exit_command.command.trim().is_empty()
        {
            self.error = Some("Enter a post-exit command before enabling it.".to_owned());
        }
        self.error.is_none()
    }

    fn value(&self, field: usize) -> String {
        match field {
            0 => self.draft.game_version.clone(),
            1 => self.draft.loader.to_string(),
            2 => self.draft.loader_version.clone().unwrap_or_default(),
            3 => self.draft.java_path.clone().unwrap_or_default(),
            4 => self.draft.memory_min.clone().unwrap_or_default(),
            5 => self.draft.memory_max.clone().unwrap_or_default(),
            6 => format_tag_values(&self.draft.jvm_args),
            7 => format_tag_values(&environment_labels(&self.draft.environment)),
            8 => window_mode_title(
                self.draft
                    .effective_window_mode(SETTINGS.read().defaults.window_mode),
            )
            .to_owned(),
            9 => self
                .draft
                .effective_resolution(SETTINGS.read().defaults.resolution)
                .map(|(w, h)| format!("{w}x{h}"))
                .unwrap_or_default(),
            10 => self.draft.preferred_account.clone().unwrap_or_default(),
            11 => if self.desktop { "yes" } else { "no" }.to_owned(),
            12 => self.draft.pre_launch_command.command.clone(),
            13 => self.draft.post_exit_command.command.clone(),
            14 => self.draft.glfw_path.clone().unwrap_or_default(),
            _ => String::new(),
        }
    }

    fn display_value(&self, field: usize) -> String {
        match field {
            2 if self.draft.loader == ModLoader::Vanilla => "not applicable".to_owned(),
            3 => self.java_picker.display_label(
                self.draft
                    .java_path
                    .as_deref()
                    .unwrap_or_else(|| self.java_picker.detected_path()),
            ),
            4 if self.draft.memory_min.is_none() => SETTINGS.read().defaults.memory_min.clone(),
            5 if self.draft.memory_max.is_none() => SETTINGS.read().defaults.memory_max.clone(),
            6 if self.draft.jvm_args.is_empty() => "no arguments".to_owned(),
            6 => self.draft.jvm_args.join(" "),
            7 if self.draft.environment.is_empty() => "no variables".to_owned(),
            7 => self.value(7),
            9 if self.draft.inherit_resolution => SETTINGS.read().defaults.resolution.map_or_else(
                || "game default".to_owned(),
                |(width, height)| format!("{width}x{height}"),
            ),
            9 if self.draft.resolution.is_none() => "game default".to_owned(),
            10 => self.preferred_account_label(),
            11 if self.desktop => "enabled".to_owned(),
            11 => "disabled".to_owned(),
            12 if self.draft.pre_launch_command.command.is_empty() => "no command".to_owned(),
            13 if self.draft.post_exit_command.command.is_empty() => "no command".to_owned(),
            14 => self
                .glfw_picker
                .display_label(self.draft.glfw_path.as_deref()),
            _ => self.value(field).replace('\n', " ↵ "),
        }
    }

    fn preferred_account_label(&self) -> String {
        if let Some(username) = self
            .draft
            .preferred_account
            .as_deref()
            .and_then(|uuid| self.accounts.iter().find(|account| account.uuid == uuid))
            .map(|account| account.username.clone())
        {
            return username;
        }
        self.accounts
            .iter()
            .find(|account| account.active)
            .map(|account| account.username.clone())
            .unwrap_or_else(|| "no active account".to_owned())
    }

    fn account_is_auto(&self) -> bool {
        self.draft
            .preferred_account
            .as_deref()
            .is_none_or(|uuid| !self.accounts.iter().any(|account| account.uuid == uuid))
    }

    fn launch_command(&self, field: usize) -> Option<&LaunchCommand> {
        match field {
            12 => Some(&self.draft.pre_launch_command),
            13 => Some(&self.draft.post_exit_command),
            _ => None,
        }
    }

    fn launch_command_mut(&mut self, field: usize) -> Option<&mut LaunchCommand> {
        match field {
            12 => Some(&mut self.draft.pre_launch_command),
            13 => Some(&mut self.draft.post_exit_command),
            _ => None,
        }
    }

    fn toggle_selected_command(&mut self) -> Action {
        let phase = if self.selected == 12 {
            "pre-launch"
        } else {
            "post-exit"
        };
        let Some(command) = self.launch_command_mut(self.selected) else {
            return Action::None;
        };
        if !command.enabled && command.command.trim().is_empty() {
            return Action::Warning(format!("Enter a {phase} command before enabling it"));
        }
        command.enabled = !command.enabled;
        Action::None
    }

    fn sync_account_picker(&mut self) {
        let preferred = self
            .draft
            .preferred_account
            .as_deref()
            .filter(|uuid| self.accounts.iter().any(|account| account.uuid == *uuid))
            .or_else(|| {
                self.accounts
                    .iter()
                    .find(|account| account.active)
                    .map(|account| account.uuid.as_str())
            });
        let automatic = self.account_is_auto();
        let options = self
            .accounts
            .iter()
            .map(|account| SettingsPickerOption {
                key: account.uuid.clone(),
                title: account.username.clone(),
                detail: (account.account_type == AccountType::Offline)
                    .then(|| "(Offline)".to_owned()),
                leading: Some(if account.active { "▸ " } else { "  " }.to_owned()),
                active: account.active,
                badge: (automatic && account.active).then_some(SettingsPickerBadge::Auto),
            })
            .collect();
        self.account_picker.sync(options, preferred);
    }

    fn begin_edit(&mut self) {
        match self.selected {
            0 => self.open_game_picker(),
            1 => self.open_choice_picker(ChoicePicker::Loader),
            2 if self.draft.loader == ModLoader::Vanilla => {
                self.error = Some("Vanilla does not use a loader version".to_owned());
            }
            2 => self.open_loader_picker(),
            3 => self.open_choice_picker(ChoicePicker::Java),
            4 | 5 => {
                self.editing = Some(settings_text_area(vec![
                    self.effective_memory(self.selected),
                ]));
            }
            6 => self.editing = Some(settings_text_area(vec![self.value(self.selected)])),
            7 => self.editing = Some(settings_text_area(vec![self.value(7)])),
            8 => {
                let current = self
                    .draft
                    .effective_window_mode(SETTINGS.read().defaults.window_mode);
                self.draft.inherit_window_mode = false;
                self.draft.window_mode = toggle_window_mode(current);
            }
            9 => self.open_choice_picker(ChoicePicker::Resolution),
            10 => self.open_choice_picker(ChoicePicker::Account),
            11 => self.desktop = !self.desktop,
            12 | 13 => {
                self.editing = Some(settings_text_area(vec![self.value(self.selected)]));
            }
            14 => self.open_choice_picker(ChoicePicker::Glfw),
            field => self.editing = Some(settings_text_area(vec![self.value(field)])),
        }
    }

    fn open_choice_picker(&mut self, picker: ChoicePicker) {
        self.choice_picker = Some(picker);
        match picker {
            ChoicePicker::Loader => {
                self.choice_index = super::select_list::MOD_LOADERS
                    .iter()
                    .position(|loader| *loader == self.draft.loader)
                    .unwrap_or(0);
            }
            ChoicePicker::Java => {
                self.java_picker.open(self.draft.java_path.as_deref());
                self.java_picker.initialize();
            }
            ChoicePicker::Resolution => {
                let resolution = self
                    .draft
                    .effective_resolution(SETTINGS.read().defaults.resolution);
                self.choice_index = self
                    .resolution_choices()
                    .iter()
                    .position(|choice| choice.resolution() == resolution)
                    .unwrap_or(0);
            }
            ChoicePicker::Account => {
                self.account_picker.reset();
                self.sync_account_picker();
            }
            ChoicePicker::Glfw => {
                self.glfw_picker.open(self.draft.glfw_path.as_deref());
                self.glfw_picker.initialize();
            }
        }
    }

    fn choice_values(&self) -> Vec<String> {
        self.choice_picker
            .map_or_else(Vec::new, |picker| self.choice_values_for(picker))
    }

    fn choice_values_for(&self, picker: ChoicePicker) -> Vec<String> {
        match picker {
            ChoicePicker::Loader => super::select_list::MOD_LOADERS
                .iter()
                .map(ToString::to_string)
                .collect(),
            ChoicePicker::Java => self.java_picker.labels(),
            ChoicePicker::Resolution => self
                .resolution_choices()
                .iter()
                .map(ResolutionChoice::label)
                .collect(),
            ChoicePicker::Account => self.account_picker.labels(),
            ChoicePicker::Glfw => self.glfw_picker.labels(),
        }
    }

    fn handle_choice_key(&mut self, key: &KeyEvent) {
        if self.choice_picker == Some(ChoicePicker::Resolution) {
            let count = self.resolution_choices().len();
            match handle_resolution_picker_key(&mut self.choice_index, count, key) {
                ResolutionPickerAction::Back => self.choice_picker = None,
                ResolutionPickerAction::Default => {
                    self.apply_default_resolution();
                    self.choice_picker = None;
                }
                ResolutionPickerAction::Select => self.apply_choice(),
                ResolutionPickerAction::None => {}
            }
            return;
        }
        let picker_action = match self.choice_picker {
            Some(ChoicePicker::Java) => {
                self.java_picker.initialize();
                Some(self.java_picker.selection_mut().handle_key(key))
            }
            Some(ChoicePicker::Account) => {
                self.sync_account_picker();
                Some(self.account_picker.handle_key(key))
            }
            Some(ChoicePicker::Glfw) => {
                self.glfw_picker.initialize();
                Some(self.glfw_picker.selection_mut().handle_key(key))
            }
            _ => None,
        };
        if let Some(action) = picker_action {
            match action {
                SettingsPickerAction::Back => self.choice_picker = None,
                SettingsPickerAction::Select => self.apply_choice(),
                SettingsPickerAction::None => {}
            }
            return;
        }
        let count = self.choice_values().len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('h') | KeyCode::Left => self.choice_picker = None,
            KeyCode::Char('j') | KeyCode::Down if count > 0 => {
                self.choice_index = (self.choice_index + 1).min(count - 1);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.choice_index = self.choice_index.saturating_sub(1);
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => self.apply_choice(),
            _ => {}
        }
    }

    fn apply_choice(&mut self) {
        let mut open_loader_versions = false;
        match self.choice_picker {
            Some(ChoicePicker::Loader) => {
                let available = super::select_list::MOD_LOADERS;
                let loader = available[self.choice_index.min(available.len() - 1)];
                if self.draft.loader != loader {
                    self.draft.loader = loader;
                    self.draft.loader_version = None;
                    self.loader_versions = Arc::new(Mutex::new(LoadState::Idle));
                    self.game_versions = Arc::new(Mutex::new(LoadState::Idle));
                    open_loader_versions = loader != ModLoader::Vanilla;
                }
            }
            Some(ChoicePicker::Java) => match self.java_picker.selected_choice() {
                JavaChoice::Installation(path) => self.draft.java_path = Some(path),
            },
            Some(ChoicePicker::Resolution) => {
                let selected = self
                    .resolution_choices()
                    .get(self.choice_index)
                    .and_then(ResolutionChoice::resolution);
                if let Some(resolution) = selected {
                    self.draft.resolution = Some(resolution);
                    self.draft.inherit_resolution = false;
                }
            }
            Some(ChoicePicker::Account) => {
                self.draft.preferred_account =
                    self.account_picker.selected_key().map(str::to_owned);
            }
            Some(ChoicePicker::Glfw) => {
                self.draft.glfw_path = match self.glfw_picker.selected_choice() {
                    GlfwChoice::Bundled => None,
                    GlfwChoice::System(path) => Some(path),
                };
            }
            None => {}
        }
        self.choice_picker = None;
        if open_loader_versions {
            self.open_loader_picker();
        }
    }

    fn open_game_picker(&mut self) {
        self.picker = Some(VersionPicker::Game);
        self.picker_index = 0;
        self.picker_initialized = false;
        self.picker_search.deactivate();
        let mut load = self
            .game_versions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(*load, LoadState::Idle | LoadState::Error(_)) {
            *load = LoadState::Loading;
            let target = self.game_versions.clone();
            let loader = self.draft.loader;
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let result = super::version_lists::game_versions(loader).await;
                    *target
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = match result {
                        Ok(versions) => LoadState::Loaded(versions),
                        Err(error) => {
                            crate::feedback::errors::push_message(
                                tracing::Level::ERROR,
                                format!("Failed to load game versions: {error}"),
                            );
                            LoadState::Error(error)
                        }
                    };
                    crate::feedback::request_redraw();
                });
            } else {
                *load = LoadState::Idle;
            }
        }
    }

    fn open_loader_picker(&mut self) {
        self.picker = Some(VersionPicker::Loader);
        self.picker_index = 0;
        self.picker_initialized = false;
        self.picker_search.deactivate();
        let mut load = self
            .loader_versions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(*load, LoadState::Idle | LoadState::Error(_)) {
            *load = LoadState::Loading;
            let target = self.loader_versions.clone();
            let loader = self.draft.loader;
            let game_version = self.draft.game_version.clone();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let result = super::version_lists::loader_versions(loader, &game_version).await;
                    *target
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = match result {
                        Ok(versions) => LoadState::Loaded(versions),
                        Err(error) => {
                            crate::feedback::errors::push_message(
                                tracing::Level::ERROR,
                                format!("Failed to load loader versions: {error}"),
                            );
                            LoadState::Error(error)
                        }
                    };
                    crate::feedback::request_redraw();
                });
            } else {
                *load = LoadState::Idle;
            }
        }
    }

    fn visible_game_versions(&self) -> Vec<GameVersion> {
        match &*self
            .game_versions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            LoadState::Loaded(versions) => versions
                .iter()
                .filter(|version| self.show_snapshots || version.stable)
                .filter(|version| self.picker_search.matches(&version.id))
                .cloned()
                .collect(),
            _ => Vec::new(),
        }
    }

    fn visible_loader_versions(&self) -> Vec<String> {
        match &*self
            .loader_versions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            LoadState::Loaded(versions) => versions
                .iter()
                .filter(|version| self.picker_search.matches(version))
                .cloned()
                .collect(),
            _ => Vec::new(),
        }
    }

    fn initialize_picker_index(&mut self) {
        if self.picker_initialized {
            return;
        }
        let index = match self.picker {
            Some(VersionPicker::Game) => self
                .visible_game_versions()
                .iter()
                .position(|version| version.id == self.draft.game_version),
            Some(VersionPicker::Loader) => {
                self.draft.loader_version.as_deref().and_then(|selected| {
                    self.visible_loader_versions()
                        .iter()
                        .position(|version| version == selected)
                })
            }
            None => None,
        };
        if let Some(index) = index {
            self.picker_index = index;
            self.picker_initialized = true;
        }
    }

    fn effective_memory(&self, field: usize) -> String {
        let settings = SETTINGS.read();
        if field == 4 {
            self.draft
                .memory_min
                .clone()
                .unwrap_or_else(|| settings.defaults.memory_min.clone())
        } else {
            self.draft
                .memory_max
                .clone()
                .unwrap_or_else(|| settings.defaults.memory_max.clone())
        }
    }

    fn set_memory(&mut self, field: usize, value: Option<String>) {
        if field == 4 {
            self.draft.memory_min = value.clone();
        } else {
            self.draft.memory_max = value.clone();
        }

        let min = self.effective_memory(4);
        let max = self.effective_memory(5);
        if memory_kib(&min)
            .zip(memory_kib(&max))
            .is_some_and(|(min, max)| min > max)
        {
            if field == 4 {
                self.draft.memory_max = value;
            } else {
                self.draft.memory_min = value;
            }
        }
    }

    fn adjust_selected_memory(&mut self, forward: bool) {
        let value = adjust_memory(&self.effective_memory(self.selected), forward);
        self.set_memory(self.selected, Some(value));
        self.error = None;
    }

    fn move_selection(&mut self, forward: bool) {
        let position = FIELD_ORDER
            .iter()
            .position(|field| *field == self.selected)
            .unwrap_or(0);
        let position = if forward {
            (position + 1).min(FIELD_ORDER.len() - 1)
        } else {
            position.saturating_sub(1)
        };
        self.selected = FIELD_ORDER[position];
    }

    fn resolution_choices(&self) -> Vec<ResolutionChoice> {
        resolution_choices(
            self.draft
                .effective_resolution(SETTINGS.read().defaults.resolution),
            &self.display_resolutions,
        )
    }

    fn apply_default_memory(&mut self) {
        let settings = SETTINGS.read();
        if self.selected == 4 {
            self.draft.memory_min = Some(settings.defaults.memory_min.clone());
        } else {
            self.draft.memory_max = Some(settings.defaults.memory_max.clone());
        }
        self.error = None;
    }

    fn apply_default_resolution(&mut self) {
        self.draft.resolution = Some(default_resolution());
        self.draft.inherit_resolution = false;
        self.error = None;
    }

    fn toggle_global_window_default(&mut self) {
        if self.selected == 8 {
            if self.draft.inherit_window_mode {
                self.draft.window_mode = SETTINGS.read().defaults.window_mode;
            }
            self.draft.inherit_window_mode = !self.draft.inherit_window_mode;
        } else if self.selected == 9 {
            if self.draft.inherit_resolution {
                self.draft.resolution = SETTINGS.read().defaults.resolution;
            }
            self.draft.inherit_resolution = !self.draft.inherit_resolution;
        }
        self.error = None;
    }

    fn toggle_auto_java(&mut self) -> Action {
        let Some(current) = self.draft.java_path.as_deref() else {
            self.draft.java_path = Some(self.java_picker.detected_path().to_owned());
            self.error = None;
            return Action::None;
        };
        if self.java_picker.automatic_change(current) {
            return Action::ConfirmJavaAuto {
                name: self.draft.name.clone(),
            };
        }
        self.enable_auto_java();
        Action::None
    }

    pub fn enable_auto_java(&mut self) {
        self.draft.java_path = None;
        self.error = None;
    }

    fn toggle_auto_account(&mut self) -> Action {
        if self.account_is_auto() {
            if let Some(account) = self.accounts.iter().find(|account| account.active) {
                self.draft.preferred_account = Some(account.uuid.clone());
                self.error = None;
            } else {
                self.error = Some("No active account is available.".to_owned());
            }
            return Action::None;
        }
        let Some(preferred_uuid) = self.draft.preferred_account.as_deref() else {
            return Action::None;
        };
        let preferred = self
            .accounts
            .iter()
            .find(|account| account.uuid == preferred_uuid);
        let active = self.accounts.iter().find(|account| account.active);
        if let (Some(preferred), Some(active)) = (preferred, active)
            && preferred.uuid != active.uuid
        {
            return Action::ConfirmAccountAuto {
                name: self.draft.name.clone(),
            };
        }
        self.enable_auto_account();
        Action::None
    }

    pub fn enable_auto_account(&mut self) {
        self.draft.preferred_account = None;
        self.error = None;
    }

    fn close_version_picker(&mut self) {
        let cancel_runtime_change = self.picker == Some(VersionPicker::Loader)
            && self.runtime_changed()
            && self.draft.loader_version.is_none();
        self.picker = None;
        self.picker_search.deactivate();
        if cancel_runtime_change {
            let _ = self.cancel_runtime_change();
        }
    }

    fn handle_picker_key(&mut self, key: &KeyEvent) {
        self.initialize_picker_index();
        if self.picker_search.active {
            match key.code {
                KeyCode::Esc => {
                    self.picker_search.deactivate();
                    return;
                }
                KeyCode::Backspace => {
                    self.picker_search.backspace(key.modifiers);
                    self.picker_index = 0;
                    return;
                }
                KeyCode::Char('j') | KeyCode::Down | KeyCode::Char('k') | KeyCode::Up => {}
                KeyCode::Char(character) => {
                    self.picker_search.push(character);
                    self.picker_index = 0;
                    return;
                }
                _ => {}
            }
        }
        let count = match self.picker {
            Some(VersionPicker::Game) => self.visible_game_versions().len(),
            Some(VersionPicker::Loader) => self.visible_loader_versions().len(),
            None => 0,
        };
        match key.code {
            KeyCode::Esc => self.close_version_picker(),
            KeyCode::Char('h') | KeyCode::Left if !self.picker_search.active => {
                self.close_version_picker();
            }
            KeyCode::Char('/') if !self.picker_search.active => {
                self.picker_search.activate();
                self.picker_index = 0;
            }
            KeyCode::Char('s') if self.picker == Some(VersionPicker::Game) => {
                self.show_snapshots = !self.show_snapshots;
                self.picker_index = 0;
                self.picker_initialized = false;
            }
            KeyCode::Char('j') | KeyCode::Down if count > 0 => {
                self.picker_index = (self.picker_index + 1).min(count - 1);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.picker_index = self.picker_index.saturating_sub(1);
            }
            KeyCode::Enter if self.picker_search.active => self.picker_search.confirm(),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => match self.picker {
                Some(VersionPicker::Game) => {
                    if let Some(version) = self.visible_game_versions().get(self.picker_index) {
                        if self.draft.game_version != version.id {
                            self.draft.game_version = version.id.clone();
                            self.draft.loader_version = None;
                            self.loader_versions = Arc::new(Mutex::new(LoadState::Idle));
                            self.picker = None;
                            if self.draft.loader != ModLoader::Vanilla {
                                self.open_loader_picker();
                            }
                        } else {
                            self.picker = None;
                        }
                    }
                }
                Some(VersionPicker::Loader) => {
                    if let Some(version) = self.visible_loader_versions().get(self.picker_index) {
                        self.draft.loader_version = Some(version.clone());
                        self.picker = None;
                    }
                }
                None => {}
            },
            _ => {}
        }
    }

    fn commit_edit(&mut self) {
        let Some(editor) = self.editing.take() else {
            return;
        };
        let raw = editor.lines().join("\n");
        let value = raw.trim();
        self.error = None;
        let invalid = |state: &mut Self, message: String| {
            state.error = Some(message);
            state.editing = Some(settings_text_area(editor.lines().to_vec()));
        };
        match self.selected {
            0 if value.is_empty() => invalid(self, "Game version is required.".to_owned()),
            0 => self.draft.game_version = value.to_owned(),
            2 => self.draft.loader_version = (!value.is_empty()).then(|| value.to_owned()),
            3 => self.draft.java_path = (!value.is_empty()).then(|| value.to_owned()),
            4 | 5 if !value.is_empty() && normalize_memory_value(value).is_none() => invalid(
                self,
                "Use a positive memory value ending in K, M, or G.".to_owned(),
            ),
            4 => self.set_memory(4, normalize_memory_value(value)),
            5 => self.set_memory(5, normalize_memory_value(value)),
            6 => match parse_tag_values(value) {
                Ok(arguments) => self.draft.jvm_args = arguments,
                Err(error) => invalid(self, error),
            },
            7 => match parse_environment(value) {
                Ok(environment) => self.draft.environment = environment,
                Err(error) => invalid(self, error),
            },
            9 if value.is_empty() => {
                self.draft.resolution = None;
                self.draft.inherit_resolution = false;
            }
            9 => match parse_resolution(value) {
                Ok(resolution) => {
                    self.draft.resolution = Some(resolution);
                    self.draft.inherit_resolution = false;
                }
                Err(error) => invalid(self, error),
            },
            12 | 13 => {
                if let Some(command) = self.launch_command_mut(self.selected) {
                    command.command = value.to_owned();
                    if value.is_empty() {
                        command.enabled = false;
                    }
                }
            }
            14 => self.draft.glfw_path = (!value.is_empty()).then(|| value.to_owned()),
            _ => {}
        }
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> Action {
        if self.runtime_update_pending {
            return if key.code == KeyCode::Esc {
                Action::Close
            } else {
                Action::None
            };
        }
        let before = self.draft.clone();
        let desktop_before = self.desktop;
        let action = self.handle_key_inner(key);
        if !matches!(action, Action::None) {
            return action;
        }
        if let Some(error) = self.error.take() {
            return Action::Error(error);
        }
        if (before == self.draft && desktop_before == self.desktop && !self.save_retry_pending)
            || !self.dirty()
        {
            return Action::None;
        }
        if self.runtime_changed() {
            if self.draft.loader != ModLoader::Vanilla
                && self
                    .draft
                    .loader_version
                    .as_deref()
                    .is_none_or(str::is_empty)
            {
                return Action::None;
            }
            if !self.validate_before_save() {
                return Action::Error(
                    self.error
                        .take()
                        .unwrap_or_else(|| "invalid instance settings".to_owned()),
                );
            }
            return Action::ConfirmRuntime {
                name: self.draft.name.clone(),
            };
        }
        if self.validate_before_save() {
            self.save_retry_pending = false;
            Action::Save(Box::new(self.draft.clone()), self.desktop)
        } else {
            Action::Error(
                self.error
                    .take()
                    .unwrap_or_else(|| "invalid instance settings".to_owned()),
            )
        }
    }

    fn handle_key_inner(&mut self, key: &KeyEvent) -> Action {
        if self.choice_picker.is_some() {
            self.handle_choice_key(key);
            return Action::None;
        }
        if self.picker.is_some() {
            self.handle_picker_key(key);
            return Action::None;
        }
        if let Some(input) = &mut self.editing {
            match key.code {
                KeyCode::Enter => self.commit_edit(),
                KeyCode::Esc => self.editing = None,
                _ => handle_text_area_input(input, key),
            }
            return Action::None;
        }

        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(true),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(false),
            KeyCode::Char('h') | KeyCode::Left if matches!(self.selected, 4 | 5) => {
                self.adjust_selected_memory(false);
            }
            KeyCode::Char('l') | KeyCode::Right if matches!(self.selected, 4 | 5) => {
                self.adjust_selected_memory(true);
            }
            KeyCode::Enter => self.begin_edit(),
            KeyCode::Char('d') if matches!(self.selected, 4 | 5) => {
                self.apply_default_memory();
            }
            KeyCode::Char('d') if self.selected == 9 => self.apply_default_resolution(),
            KeyCode::Char('i') if matches!(self.selected, 8 | 9) => {
                self.toggle_global_window_default();
            }
            KeyCode::Char('c') if self.selected == 9 => {
                self.editing = Some(settings_text_area(vec![self.value(9)]));
            }
            KeyCode::Char('a') if self.selected == 3 => return self.toggle_auto_java(),
            KeyCode::Char('c') if self.selected == 3 => {
                self.editing = Some(settings_text_area(vec![self.value(3)]));
            }
            KeyCode::Char('a') if self.selected == 10 => return self.toggle_auto_account(),
            KeyCode::Char(' ') if matches!(self.selected, 12 | 13) => {
                return self.toggle_selected_command();
            }
            KeyCode::Char('c') if self.selected == 14 => {
                self.editing = Some(settings_text_area(vec![self.value(14)]));
            }
            KeyCode::Char('E') => return Action::OpenRaw,
            KeyCode::Esc => return Action::Close,
            _ => {}
        }
        Action::None
    }

    pub fn confirmed_save(&mut self) -> Option<(Box<InstanceConfig>, bool)> {
        self.validate_before_save()
            .then(|| (Box::new(self.draft.clone()), self.desktop))
    }

    pub fn mark_runtime_update_pending(&mut self) {
        self.runtime_update_pending = true;
    }

    pub fn runtime_update_pending_for(&self, name: &str) -> bool {
        self.runtime_update_pending && self.draft.name == name
    }

    pub fn mark_saved(&mut self, saved: &InstanceConfig, desktop: bool) {
        self.original = saved.clone();
        self.draft = saved.clone();
        self.original_desktop = desktop;
        self.desktop = desktop;
        self.glfw_picker
            .set_bundled_version(bundled_glfw_version(&self.meta_dir, &saved.game_version));
        self.runtime_update_pending = false;
        self.save_retry_pending = false;
        self.error = None;
    }

    pub fn mark_save_failed(&mut self) {
        self.save_retry_pending = true;
    }

    pub fn invalidate_java_cache(&mut self) {
        self.java_picker.invalidate_cache();
    }

    pub fn cancel_runtime_change(&mut self) -> Option<(Box<InstanceConfig>, bool)> {
        self.draft.game_version = self.original.game_version.clone();
        self.draft.loader = self.original.loader;
        self.draft.loader_version = self.original.loader_version.clone();
        self.game_versions = Arc::new(Mutex::new(LoadState::Idle));
        self.loader_versions = Arc::new(Mutex::new(LoadState::Idle));
        self.picker_initialized = false;
        self.runtime_update_pending = false;
        self.error = None;
        (self.dirty() && self.validate_before_save())
            .then(|| (Box::new(self.draft.clone()), self.desktop))
    }
}

pub fn popup_rect(area: Rect, state: &State) -> Rect {
    let height = if state.picker.is_some()
        || matches!(
            state.choice_picker,
            Some(ChoicePicker::Java | ChoicePicker::Glfw)
        ) {
        (area.height * 2 / 3).max(10)
    } else if state.choice_picker.is_some() {
        10
    } else {
        let form_width = (area.width * 68 / 100).saturating_sub(2);
        24 + tagged_row_count(&state.draft.jvm_args, form_width).saturating_sub(1) as u16
            + tagged_row_count(&environment_labels(&state.draft.environment), form_width)
                .saturating_sub(1) as u16
    };
    let width = match state.choice_picker {
        Some(ChoicePicker::Java | ChoicePicker::Glfw) => 72,
        Some(ChoicePicker::Resolution) => 64,
        _ => 68,
    };
    area.centered(
        Constraint::Percentage(width),
        Constraint::Length(height.min(area.height.saturating_sub(4))),
    )
}

pub fn render(frame: &mut Frame, area: Rect, state: &mut State) {
    if state.choice_picker == Some(ChoicePicker::Java) {
        state.java_picker.initialize();
    }
    if state.choice_picker == Some(ChoicePicker::Account) {
        state.sync_account_picker();
    }
    if state.choice_picker == Some(ChoicePicker::Glfw) {
        state.glfw_picker.initialize();
    }
    let theme = THEME.as_ref();
    frame.render_widget(Clear, area);
    let title = match (state.picker, state.choice_picker) {
        (Some(VersionPicker::Game), _) => "Minecraft Version",
        (Some(VersionPicker::Loader), _) => "Loader Version",
        (_, Some(ChoicePicker::Loader)) => "Mod Loader",
        (_, Some(ChoicePicker::Java)) => "Java Runtime",
        (_, Some(ChoicePicker::Resolution)) => "Resolution",
        (_, Some(ChoicePicker::Account)) => "Preferred Account",
        (_, Some(ChoicePicker::Glfw)) => "GLFW Library",
        _ => "Instance Settings",
    };
    let title = format!(" {title} · {} ", state.draft.name);
    let keybinds = if state.editing.is_some() {
        super::keybind_line(&[("Enter", " apply"), ("Esc", " cancel")])
    } else if state.picker == Some(VersionPicker::Game) {
        super::keybind_line(&[
            ("/", " search"),
            ("s", " snap"),
            ("h", " back"),
            ("Enter", " select"),
        ])
    } else if state.picker.is_some() {
        super::keybind_line(&[("/", " search"), ("h", " back"), ("Enter", " select")])
    } else if matches!(
        state.choice_picker,
        Some(ChoicePicker::Java | ChoicePicker::Account | ChoicePicker::Glfw)
    ) {
        super::keybind_line(&[("h", " back"), ("Enter", " select")])
    } else if state.choice_picker == Some(ChoicePicker::Resolution) {
        super::keybind_line(&[("d", " default"), ("h", " back"), ("Enter", " select")])
    } else if state.choice_picker.is_some() {
        super::keybind_line(&[("h", " back"), ("Enter", " select")])
    } else if matches!(state.selected, 4 | 5) {
        super::keybind_line(&[
            ("h/l", " adjust"),
            ("Enter", " exact"),
            ("d", " default"),
            ("Esc", " back"),
        ])
    } else if state.selected == 3 {
        super::keybind_line(&[
            ("Enter", " runtimes"),
            ("a", " auto"),
            ("c", " custom"),
            ("Esc", " back"),
        ])
    } else if state.selected == 9 {
        super::keybind_line(&[
            ("Enter", " presets"),
            ("c", " custom"),
            ("d", " default"),
            ("i", " global"),
            ("Esc", " back"),
        ])
    } else if state.selected == 10 {
        super::keybind_line(&[
            ("Enter", " accounts"),
            ("a", " auto"),
            ("E", " raw"),
            ("Esc", " back"),
        ])
    } else if matches!(state.selected, 0..=2) {
        super::keybind_line(&[
            ("j/k", ""),
            ("Enter", " select"),
            ("E", " raw"),
            ("Esc", " back"),
        ])
    } else if state.selected == 8 {
        super::keybind_line(&[
            ("j/k", ""),
            ("Enter", " toggle"),
            ("i", " global"),
            ("E", " raw"),
            ("Esc", " back"),
        ])
    } else if state.selected == 11 {
        super::keybind_line(&[
            ("j/k", ""),
            ("Enter", " toggle"),
            ("E", " raw"),
            ("Esc", " back"),
        ])
    } else if matches!(state.selected, 12 | 13) {
        let enabled = state
            .launch_command(state.selected)
            .is_some_and(|command| command.enabled);
        super::keybind_line(&[
            ("Enter", " command"),
            ("Space", if enabled { " disable" } else { " enable" }),
            ("E", " raw"),
            ("Esc", " back"),
        ])
    } else if state.selected == 14 {
        super::keybind_line(&[
            ("Enter", " libraries"),
            ("c", " custom"),
            ("E", " raw"),
            ("Esc", " back"),
        ])
    } else {
        super::keybind_line(&[
            ("j/k", ""),
            ("Enter", " edit"),
            ("E", " raw"),
            ("Esc", " back"),
        ])
    };
    let mut block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BORDER_STYLE.to_border_type())
        .border_style(Style::default().fg(theme.text_dim()))
        .style(Style::default().bg(theme.surface()))
        .title_bottom(keybinds.right_aligned());
    if state.picker.is_some()
        && let Some(search) = state.picker_search.title_line()
    {
        block = block.title_top(search);
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.picker.is_some() {
        render_version_picker(frame, inner, state);
        return;
    }
    if state.choice_picker.is_some() {
        render_choice_picker(frame, inner, state);
        return;
    }

    render_settings_list(frame, inner, state);
}

fn render_settings_list(frame: &mut Frame, area: Rect, state: &State) {
    let theme = THEME.as_ref();
    let sections: [(&str, &[usize]); 4] = [
        ("Game", &[0, 1, 2, 10]),
        ("Java", &[3, 4, 5, 6, 7, 14]),
        ("Window", &[8, 9, 11]),
        ("Commands", &[12, 13]),
    ];
    let mut rows: Vec<(Option<usize>, Vec<Line<'static>>)> = Vec::new();
    for (section_index, (title, fields)) in sections.iter().enumerate() {
        if section_index > 0 {
            rows.push((None, vec![Line::default()]));
        }
        rows.push((None, vec![section_line(title)]));
        for index in *fields {
            let label = field_label(*index);
            let lines = match index {
                6 => tagged_field_lines(
                    state,
                    6,
                    label,
                    &state.draft.jvm_args,
                    "no arguments",
                    area.width,
                ),
                7 => tagged_field_lines(
                    state,
                    7,
                    label,
                    &environment_labels(&state.draft.environment),
                    "no variables",
                    area.width,
                ),
                12 | 13 => command_field_lines(state, *index, label),
                _ => vec![field_line(state, *index, label)],
            };
            rows.push((Some(*index), lines));
        }
    }

    let mut selected_end = 0u16;
    let mut cursor = 0u16;
    for (field, lines) in &rows {
        let height = lines.len() as u16;
        if *field == Some(state.selected) {
            selected_end = cursor.saturating_add(height);
        }
        cursor = cursor.saturating_add(height);
    }
    let scroll = if selected_end > area.height {
        selected_end.saturating_sub(area.height)
    } else {
        0
    };

    let mut row_start = 0u16;
    for (field, lines) in rows {
        let height = lines.len() as u16;
        let row_end = row_start.saturating_add(height);
        if row_end <= scroll || row_start >= scroll.saturating_add(area.height) {
            row_start = row_end;
            continue;
        }
        let skip = scroll.saturating_sub(row_start) as usize;
        let y = area.y.saturating_add(row_start.saturating_sub(scroll));
        let visible_height = (height.saturating_sub(skip as u16))
            .min(area.y.saturating_add(area.height).saturating_sub(y));
        let row_area = Rect {
            y,
            height: visible_height,
            ..area
        };
        let selected = field == Some(state.selected);
        frame.render_widget(
            Paragraph::new(
                lines
                    .into_iter()
                    .skip(skip)
                    .take(visible_height as usize)
                    .collect::<Vec<_>>(),
            )
            .style(Style::default().bg(if selected {
                theme.stripe()
            } else {
                theme.surface()
            })),
            row_area,
        );

        if let Some(index @ (4 | 5)) = field
            && row_start >= scroll
        {
            let value = state.effective_memory(index);
            render_memory_gauge(
                frame,
                Rect {
                    x: area.x.saturating_add(20),
                    y: area.y.saturating_add(row_start.saturating_sub(scroll)),
                    width: area.width.saturating_sub(21),
                    height: 1,
                },
                &value,
                state.display_value(index),
                state.selected == index,
            );
        }
        if field == Some(state.selected)
            && row_start >= scroll
            && let Some(editor) = state.editing.as_ref()
        {
            let (edit_x, edit_offset) = if matches!(field, Some(12 | 13)) {
                (22, 0)
            } else {
                (20, 0)
            };
            frame.render_widget(
                editor,
                Rect {
                    x: area.x.saturating_add(edit_x),
                    y: area
                        .y
                        .saturating_add(row_start.saturating_sub(scroll))
                        .saturating_add(edit_offset),
                    width: area.width.saturating_sub(edit_x),
                    height: 1,
                },
            );
        }
        row_start = row_end;
    }
}

fn field_label(index: usize) -> &'static str {
    match index {
        0 => "Game version",
        1 => "Loader",
        2 => "Loader version",
        3 => "Java runtime",
        4 => "Memory min",
        5 => "Memory max",
        6 => "JVM args",
        7 => "Environment",
        8 => "Window mode",
        9 => "Resolution",
        10 => "Preferred account",
        11 => "Desktop shortcut",
        12 => "Pre-launch",
        13 => "Post-exit",
        14 => "GLFW",
        _ => "",
    }
}

fn section_line(title: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("  {title}"),
        Style::default()
            .fg(THEME.as_ref().text())
            .add_modifier(Modifier::BOLD),
    ))
}

fn field_line(state: &State, index: usize, label: &str) -> Line<'static> {
    let theme = THEME.as_ref();
    let selected = index == state.selected;
    let editing = selected && state.editing.is_some();
    let value = if editing || matches!(index, 4..=7) {
        String::new()
    } else {
        state.display_value(index)
    };
    let mut spans = vec![
        Span::styled(
            if selected { "▌ " } else { "  " },
            Style::default().fg(theme.accent()),
        ),
        Span::styled(
            format!("{label:<18}"),
            Style::default().fg(theme.text_dim()),
        ),
        Span::styled(
            value,
            Style::default()
                .fg(if index == 11 {
                    if state.desktop {
                        theme.text()
                    } else {
                        theme.text_dim()
                    }
                } else if selected {
                    theme.accent()
                } else {
                    theme.text()
                })
                .add_modifier(if selected {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        ),
    ];
    if index == 3 && state.draft.java_path.is_none() && !editing {
        spans.extend([Span::raw("  "), auto_label()]);
    }
    if index == 10 && state.account_is_auto() {
        spans.extend([Span::raw("  "), auto_label()]);
    }
    if index == 14 && state.draft.glfw_path.is_none() && !editing {
        spans.extend([Span::raw("  "), bundled_badge()]);
    }
    if index == 9
        && !editing
        && !state.draft.inherit_resolution
        && is_default_resolution(state.draft.resolution)
    {
        spans.extend([Span::raw("  "), default_label()]);
    }
    if (index == 8 && state.draft.inherit_window_mode)
        || (index == 9 && state.draft.inherit_resolution)
    {
        spans.extend([Span::raw("  "), status_badge("Global", theme.accent())]);
    }
    Line::from(spans)
}

fn command_field_lines(state: &State, index: usize, label: &str) -> Vec<Line<'static>> {
    let theme = THEME.as_ref();
    let selected = state.selected == index;
    let enabled = state
        .launch_command(index)
        .is_some_and(|command| command.enabled);
    let label_style = Style::default()
        .fg(if selected {
            theme.accent()
        } else {
            theme.text()
        })
        .add_modifier(if selected {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });
    let checkbox_style = Style::default()
        .fg(if enabled {
            theme.success()
        } else {
            theme.text_dim()
        })
        .add_modifier(if enabled {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });
    let mut spans = vec![
        Span::styled(
            if selected { "▌ " } else { "  " },
            Style::default().fg(theme.accent()),
        ),
        Span::styled(if enabled { "[✓] " } else { "[ ] " }, checkbox_style),
        Span::styled(format!("{label:<16}"), label_style),
    ];
    if selected && state.editing.is_some() {
        return vec![Line::from(spans)];
    }
    let command = state.display_value(index);
    spans.push(Span::styled(
        command,
        Style::default()
            .fg(if !enabled || state.value(index).is_empty() {
                theme.text_dim()
            } else if selected {
                theme.accent()
            } else {
                theme.text()
            })
            .add_modifier(if selected && !state.value(index).is_empty() {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
    ));
    vec![Line::from(spans)]
}

fn tagged_field_lines(
    state: &State,
    index: usize,
    label: &str,
    values: &[String],
    empty: &str,
    width: u16,
) -> Vec<Line<'static>> {
    let selected = state.selected == index;
    tagged_value_lines(
        field_line(state, index, label).spans,
        selected,
        state.editing.is_some(),
        values,
        empty,
        width,
    )
}

fn render_choice_picker(frame: &mut Frame, area: Rect, state: &mut State) {
    let theme = THEME.as_ref();
    let mut list_area = area;
    if state.choice_picker == Some(ChoicePicker::Java)
        && let Some(status) = state.java_picker.take_status()
    {
        match status {
            Ok(message) => {
                frame.render_widget(
                    Paragraph::new(message).style(Style::default().fg(theme.text_dim())),
                    Rect { height: 1, ..area },
                );
                list_area.y = list_area.y.saturating_add(1);
                list_area.height = list_area.height.saturating_sub(1);
            }
            Err(error) => crate::feedback::errors::push_message(tracing::Level::ERROR, error),
        }
    }
    if state.choice_picker == Some(ChoicePicker::Glfw)
        && let Some(status) = state.glfw_picker.take_status()
    {
        frame.render_widget(
            Paragraph::new(status).style(Style::default().fg(theme.text_dim())),
            Rect { height: 1, ..area },
        );
        list_area.y = list_area.y.saturating_add(1);
        list_area.height = list_area.height.saturating_sub(1);
    }
    if state.choice_picker == Some(ChoicePicker::Account) && state.accounts.is_empty() {
        frame.render_widget(
            Paragraph::new("No accounts.").style(Style::default().fg(theme.text_dim())),
            list_area,
        );
        return;
    }
    let items = match state.choice_picker {
        Some(ChoicePicker::Resolution) => {
            resolution_items(&state.resolution_choices(), state.choice_index)
        }
        Some(ChoicePicker::Java | ChoicePicker::Account | ChoicePicker::Glfw) => Vec::new(),
        _ => state
            .choice_values()
            .iter()
            .map(|value| {
                ListItem::new(Line::from(Span::styled(
                    value.clone(),
                    Style::default().fg(theme.text()),
                )))
            })
            .collect(),
    };
    let settings_picker = match state.choice_picker {
        Some(ChoicePicker::Java) => Some(state.java_picker.selection()),
        Some(ChoicePicker::Account) => Some(&state.account_picker),
        Some(ChoicePicker::Glfw) => Some(state.glfw_picker.selection()),
        _ => None,
    };
    if let Some(picker) = settings_picker {
        render_settings_picker(picker, list_area, frame.buffer_mut());
    } else if state.choice_picker == Some(ChoicePicker::Resolution) {
        super::select_list::render_styled(items, state.choice_index, list_area, frame.buffer_mut());
    } else {
        super::select_list::render(items, state.choice_index, list_area, frame.buffer_mut());
    }
}

fn render_version_picker(frame: &mut Frame, area: Rect, state: &State) {
    let theme = THEME.as_ref();
    let status = match state.picker {
        Some(VersionPicker::Game) => match &*state
            .game_versions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            LoadState::Idle => PickerLoad::Idle,
            LoadState::Loading => PickerLoad::Loading,
            LoadState::Loaded(_) => PickerLoad::Loaded,
            LoadState::Error(_) => PickerLoad::Error,
        },
        Some(VersionPicker::Loader) => match &*state
            .loader_versions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            LoadState::Idle => PickerLoad::Idle,
            LoadState::Loading => PickerLoad::Loading,
            LoadState::Loaded(_) => PickerLoad::Loaded,
            LoadState::Error(_) => PickerLoad::Error,
        },
        None => return,
    };
    match status {
        PickerLoad::Idle | PickerLoad::Loading => frame.render_widget(
            Paragraph::new("Loading versions...").style(Style::default().fg(theme.text_dim())),
            area,
        ),
        PickerLoad::Error => frame.render_widget(
            Paragraph::new("Reopen to retry").style(Style::default().fg(theme.text_dim())),
            area,
        ),
        PickerLoad::Loaded => {
            let versions: Vec<String> = match state.picker {
                Some(VersionPicker::Game) => state
                    .visible_game_versions()
                    .into_iter()
                    .map(|version| {
                        if version.stable {
                            version.id
                        } else {
                            format!("{} (snapshot)", version.id)
                        }
                    })
                    .collect(),
                Some(VersionPicker::Loader) => {
                    state.visible_loader_versions().into_iter().collect()
                }
                None => Vec::new(),
            };
            if versions.is_empty() {
                frame.render_widget(Paragraph::new("No matching versions."), area);
            } else {
                let items = versions
                    .iter()
                    .map(|version| {
                        ListItem::new(
                            state
                                .picker_search
                                .highlight_line(version, Style::default().fg(theme.text())),
                        )
                    })
                    .collect();
                super::select_list::render(items, state.picker_index, area, frame.buffer_mut());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::WindowMode;
    use chrono::Utc;

    fn instance() -> InstanceConfig {
        InstanceConfig {
            name: "test".to_owned(),
            game_version: "1.21.1".to_owned(),
            loader: ModLoader::Fabric,
            loader_version: Some("0.16.0".to_owned()),
            created: Utc::now(),
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
        }
    }

    #[test]
    fn memory_and_resolution_inputs_are_normalized() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());
        state.selected = 4;
        state.editing = Some(settings_text_area(vec!["2048m".to_owned()]));
        state.commit_edit();
        assert_eq!(state.draft.memory_min.as_deref(), Some("2048M"));

        state.selected = 9;
        state.editing = Some(settings_text_area(vec!["1920X1080".to_owned()]));
        state.commit_edit();
        assert_eq!(state.draft.resolution, Some((1920, 1080)));
    }

    #[test]
    fn memory_and_jvm_arguments_are_edited_inline() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = instance();
        config.memory_min = Some("512M".to_owned());
        let mut state = State::new(&config, temp.path());
        state.selected = 4;
        state.handle_key(&KeyEvent::from(KeyCode::Enter));
        state.handle_key(&KeyEvent::from(KeyCode::Left));
        state.handle_key(&KeyEvent::from(KeyCode::Char('0')));
        state.handle_key(&KeyEvent::from(KeyCode::Enter));
        assert_eq!(state.draft.memory_min.as_deref(), Some("5120M"));

        state.selected = 6;
        state.begin_edit();
        for character in "-Xfoo -Xbar".chars() {
            state.handle_key(&KeyEvent::from(KeyCode::Char(character)));
        }
        state.handle_key(&KeyEvent::from(KeyCode::Enter));
        assert_eq!(state.draft.jvm_args, ["-Xfoo", "-Xbar"]);
    }

    #[test]
    fn runtime_changes_require_confirmation_before_save() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());
        *state.loader_versions.lock().unwrap() = LoadState::Loaded(vec!["0.16.15".to_owned()]);
        state.picker = Some(VersionPicker::Loader);
        state.picker_initialized = true;
        state.picker_index = 0;

        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Enter)),
            Action::ConfirmRuntime { .. }
        ));
        assert!(state.confirmed_save().is_some());
    }

    #[test]
    fn version_picker_selects_loaded_version_and_clears_loader_version() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());
        *state.game_versions.lock().unwrap() = LoadState::Loaded(vec![
            GameVersion {
                id: "1.21.2".to_owned(),
                stable: true,
            },
            GameVersion {
                id: "1.21.1".to_owned(),
                stable: true,
            },
        ]);
        state.picker = Some(VersionPicker::Game);
        state.picker_initialized = true;
        state.picker_index = 0;

        state.handle_picker_key(&KeyEvent::from(KeyCode::Enter));

        assert_eq!(state.draft.game_version, "1.21.2");
        assert_eq!(state.draft.loader_version, None);
    }

    #[test]
    fn popup_changes_do_not_replace_the_existing_config_profile() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = instance();
        config.config_sync_profile = Some("shared".to_owned());
        let mut state = State::new(&config, temp.path());
        state.selected = 11;

        let Action::Save(config, _) = state.handle_key(&KeyEvent::from(KeyCode::Enter)) else {
            panic!("expected settings save");
        };

        assert_eq!(config.config_sync_profile.as_deref(), Some("shared"));
    }

    #[test]
    fn selecting_loader_clears_the_previous_loaders_version() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());

        state.open_choice_picker(ChoicePicker::Loader);
        state.handle_choice_key(&KeyEvent::from(KeyCode::Char('j')));
        state.handle_choice_key(&KeyEvent::from(KeyCode::Enter));

        assert_eq!(state.draft.loader, ModLoader::Forge);
        assert_eq!(state.draft.loader_version, None);
        assert!(!state.validate_before_save());
    }

    #[test]
    fn loader_change_selects_a_version_then_requests_confirmation() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());
        state.open_choice_picker(ChoicePicker::Loader);
        state.handle_key(&KeyEvent::from(KeyCode::Char('j')));

        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Enter)),
            Action::None
        ));
        assert_eq!(state.picker, Some(VersionPicker::Loader));

        *state.loader_versions.lock().unwrap() = LoadState::Loaded(vec!["1.0.0".to_owned()]);
        state.picker_initialized = true;
        state.picker_index = 0;
        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Enter)),
            Action::ConfirmRuntime { .. }
        ));
    }

    #[test]
    fn invalid_field_action_is_returned_for_toast_display() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = instance();
        config.loader = ModLoader::Vanilla;
        config.loader_version = None;
        let mut state = State::new(&config, temp.path());
        state.selected = 2;

        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Enter)),
            Action::Error(message) if message == "Vanilla does not use a loader version"
        ));
        assert!(state.error.is_none());
    }

    #[test]
    fn loader_picker_and_desktop_toggle_use_enter() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());

        state.selected = 1;
        state.handle_key(&KeyEvent::from(KeyCode::Enter));
        state.handle_key(&KeyEvent::from(KeyCode::Char('j')));
        state.handle_key(&KeyEvent::from(KeyCode::Enter));
        assert_eq!(state.draft.loader, ModLoader::Forge);
        assert_eq!(state.picker, Some(VersionPicker::Loader));

        state.picker = None;
        state.selected = 11;
        let desktop = state.desktop;
        state.handle_key(&KeyEvent::from(KeyCode::Enter));
        assert_ne!(state.desktop, desktop);
    }

    #[test]
    fn memory_uses_slider_and_jvm_args_use_inline_editor() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());

        state.selected = 4;
        state.handle_key(&KeyEvent::from(KeyCode::Char('l')));
        assert!(state.draft.memory_min.is_some());
        state.begin_edit();
        assert!(state.editing.is_some());

        state.editing = None;
        state.selected = 6;
        state.begin_edit();
        assert!(state.editing.is_some());
        assert!(state.choice_picker.is_none());
        assert!(state.picker.is_none());
    }

    #[test]
    fn resolution_and_java_use_selectors() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());

        state.selected = 9;
        state.begin_edit();
        assert!(state.editing.is_none());
        assert_eq!(state.choice_picker, Some(ChoicePicker::Resolution));
        state.choice_index = state
            .resolution_choices()
            .iter()
            .position(|choice| choice.resolution() == Some((1920, 1080)))
            .unwrap();
        state.apply_choice();
        assert_eq!(state.draft.resolution, Some((1920, 1080)));

        state.selected = 3;
        state.begin_edit();
        assert_eq!(state.choice_picker, Some(ChoicePicker::Java));
        let original_java = state.draft.java_path.clone();
        state.handle_choice_key(&KeyEvent::from(KeyCode::Char('a')));
        assert_eq!(state.choice_picker, Some(ChoicePicker::Java));
        assert_eq!(state.draft.java_path, original_java);
        state.handle_choice_key(&KeyEvent::from(KeyCode::Char('c')));
        assert_eq!(state.choice_picker, Some(ChoicePicker::Java));
        assert!(state.editing.is_none());
        state.handle_choice_key(&KeyEvent::from(KeyCode::Esc));
        state.handle_key(&KeyEvent::from(KeyCode::Char('c')));
        assert!(state.editing.is_some());
    }

    #[test]
    fn right_arrow_selects_loader_without_moving_down() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());
        state.selected = 1;
        state.handle_key(&KeyEvent::from(KeyCode::Enter));
        assert_eq!(state.choice_picker, Some(ChoicePicker::Loader));

        state.handle_key(&KeyEvent::from(KeyCode::Right));

        assert!(state.choice_picker.is_none());
        assert_eq!(state.draft.loader, ModLoader::Fabric);
    }

    #[test]
    fn failed_instance_save_retries_without_another_edit() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());
        state.draft.jvm_args.push("-Xretry".to_owned());
        state.mark_save_failed();

        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Down)),
            Action::Save(..)
        ));
    }

    #[test]
    fn cancelling_runtime_change_keeps_other_edits_ready_to_save() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());
        state.draft.game_version = "1.21.2".to_owned();
        state.draft.jvm_args.push("-Xkeep".to_owned());

        let (saved, _) = state.cancel_runtime_change().expect("remaining edit");

        assert_eq!(saved.game_version, state.original.game_version);
        assert_eq!(saved.jvm_args, ["-Xkeep"]);
    }

    #[test]
    fn java_and_resolution_defaults_are_direct_actions_not_list_rows() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());

        state.selected = 3;
        state.draft.java_path = Some("/custom/java".to_owned());
        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Char('a'))),
            Action::ConfirmJavaAuto { .. }
        ));
        assert_eq!(state.draft.java_path.as_deref(), Some("/custom/java"));
        state.enable_auto_java();
        assert_eq!(state.draft.java_path, None);
        state.handle_key(&KeyEvent::from(KeyCode::Char('a')));
        assert_eq!(
            state.draft.java_path.as_deref(),
            Some(state.java_picker.detected_path())
        );
        let saved = state.draft.clone();
        state.mark_saved(&saved, state.desktop);
        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Char('a'))),
            Action::Save(..)
        ));
        assert_eq!(state.draft.java_path, None);

        state.selected = 9;
        state.display_resolutions = vec![DisplayResolution {
            width: 2560,
            height: 1440,
            name: "DP-4".to_owned(),
            primary: true,
        }];
        state.draft.resolution = Some((1920, 1080));
        state.handle_key(&KeyEvent::from(KeyCode::Char('d')));
        assert_eq!(state.draft.resolution, Some((854, 480)));
        assert!(
            field_line(&state, 9, field_label(9))
                .to_string()
                .contains("Default")
        );
        state.handle_key(&KeyEvent::from(KeyCode::Enter));
        assert!(!state.choice_values().iter().any(|value| value == "Default"));
        assert!(!state.choice_values().iter().any(|value| value == "Custom…"));

        state.handle_key(&KeyEvent::from(KeyCode::Esc));
        state.handle_key(&KeyEvent::from(KeyCode::Char('c')));
        assert!(state.editing.is_some());
    }

    #[test]
    fn memory_default_copies_only_the_selected_launcher_value() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());
        state.draft.memory_min = Some("6G".to_owned());
        state.draft.memory_max = Some("42G".to_owned());
        state.selected = 4;
        let expected = SETTINGS.read().defaults.memory_min.clone();

        state.handle_key(&KeyEvent::from(KeyCode::Char('d')));

        assert_eq!(state.draft.memory_min.as_deref(), Some(expected.as_str()));
        assert_eq!(state.draft.memory_max.as_deref(), Some("42G"));
        assert!(!state.display_value(4).contains("default"));
    }

    #[test]
    fn memory_slider_keeps_bounds_linked_and_desktop_has_no_dot() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());
        state.draft.memory_min = Some("4G".to_owned());
        state.draft.memory_max = Some("4G".to_owned());
        state.selected = 4;

        state.adjust_selected_memory(true);

        assert_eq!(state.draft.memory_min.as_deref(), Some("6G"));
        assert_eq!(state.draft.memory_max.as_deref(), Some("6G"));
        let desktop = state.display_value(11);
        assert!(matches!(desktop.as_str(), "enabled" | "disabled"));
        assert!(!desktop.contains('●') && !desktop.contains('○'));
    }

    #[test]
    fn detected_resolutions_are_listed_before_presets_without_duplicates() {
        let displays = vec![DisplayResolution {
            width: 1920,
            height: 1080,
            name: "Primary display".to_owned(),
            primary: true,
        }];

        let choices = resolution_choices(None, &displays);

        assert!(matches!(choices[0], ResolutionChoice::Display(_)));
        assert_eq!(
            choices
                .iter()
                .filter(|choice| choice.resolution() == Some((1920, 1080)))
                .count(),
            1
        );
    }

    #[test]
    fn explicit_game_default_resolution_is_labeled_clearly() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());
        state.display_resolutions = vec![DisplayResolution {
            width: 2560,
            height: 1440,
            name: "DP-4".to_owned(),
            primary: true,
        }];

        assert_eq!(state.display_value(9), "game default");
    }

    #[test]
    fn window_settings_can_explicitly_inherit_launcher_defaults() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());

        state.selected = 8;
        state.handle_key(&KeyEvent::from(KeyCode::Char('i')));
        assert!(state.draft.inherit_window_mode);
        assert_eq!(
            state.display_value(8),
            window_mode_title(SETTINGS.read().defaults.window_mode)
        );

        state.selected = 9;
        state.handle_key(&KeyEvent::from(KeyCode::Char('i')));
        assert!(state.draft.inherit_resolution);
        assert_eq!(
            state
                .draft
                .effective_resolution(SETTINGS.read().defaults.resolution),
            SETTINGS.read().defaults.resolution
        );
    }

    #[test]
    fn runtime_changes_do_not_render_an_inline_notification() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());
        state.draft.game_version = "1.21.2".to_owned();
        let backend = ratatui::backend::TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                let area = popup_rect(frame.area(), &state);
                render(frame, area, &mut state);
            })
            .unwrap();

        assert!(!terminal.backend().to_string().contains("installed mods"));
    }

    #[test]
    fn cancelling_chained_loader_version_restores_the_original_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());
        let original = state.original.clone();
        *state.game_versions.lock().unwrap() = LoadState::Loaded(vec![GameVersion {
            id: "1.21.2".to_owned(),
            stable: true,
        }]);
        state.picker = Some(VersionPicker::Game);
        state.picker_initialized = true;

        state.handle_key(&KeyEvent::from(KeyCode::Enter));
        assert_eq!(state.picker, Some(VersionPicker::Loader));
        assert_eq!(state.draft.loader_version, None);

        state.handle_key(&KeyEvent::from(KeyCode::Esc));
        assert_eq!(state.draft.game_version, original.game_version);
        assert_eq!(state.draft.loader, original.loader);
        assert_eq!(state.draft.loader_version, original.loader_version);
        assert_eq!(state.picker, None);
    }

    #[test]
    fn jvm_argument_badges_wrap_without_hiding_following_fields() {
        let temp = tempfile::tempdir().unwrap();
        let mut config = instance();
        config.jvm_args = [
            "-Xmx4G",
            "-XX:+UseG1GC",
            "-Dexample.first=true",
            "-Dexample.second=true",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect();
        let state = State::new(&config, temp.path());

        let lines = tagged_field_lines(
            &state,
            6,
            "JVM args",
            &state.draft.jvm_args,
            "no arguments",
            48,
        );

        assert!(lines.len() > 1);
        assert_eq!(tagged_row_count(&state.draft.jvm_args, 48), lines.len());
        assert!(lines.iter().any(|line| line.to_string().contains("second")));
    }

    #[test]
    fn environment_variables_are_parsed_and_validated() {
        assert_eq!(
            parse_environment("MESA_LOADER_DRIVER_OVERRIDE=zink FOO=bar=baz")
                .unwrap()
                .get("FOO")
                .map(String::as_str),
            Some("bar=baz")
        );
        assert!(parse_environment("MISSING_VALUE").is_err());
        assert!(parse_environment("FOO=one FOO=two").is_err());
    }

    #[test]
    fn window_mode_and_commands_autosave() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());
        state.selected = 8;
        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Enter)),
            Action::Save(..)
        ));
        assert_eq!(state.draft.window_mode, WindowMode::Fullscreen);

        let saved = state.draft.clone();
        state.mark_saved(&saved, state.desktop);
        state.selected = 12;
        state.editing = Some(settings_text_area(vec!["echo ready".to_owned()]));
        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Enter)),
            Action::Save(..)
        ));
        let saved = state.draft.clone();
        state.mark_saved(&saved, state.desktop);
        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Char(' '))),
            Action::Save(..)
        ));
        assert!(state.draft.pre_launch_command.enabled);

        let saved = state.draft.clone();
        state.mark_saved(&saved, state.desktop);
        state.editing = Some(settings_text_area(vec![String::new()]));
        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Enter)),
            Action::Save(..)
        ));
        assert!(state.draft.pre_launch_command.command.is_empty());
        assert!(!state.draft.pre_launch_command.enabled);
    }

    #[test]
    fn preferred_account_picker_uses_accounts_and_main_row_auto() {
        let temp = tempfile::tempdir().unwrap();
        let accounts = vec![Account {
            uuid: "account-id".to_owned(),
            username: "Player".to_owned(),
            account_type: AccountType::Microsoft,
            active: true,
            refresh_token: None,
            cached_mc_token: None,
            cached_mc_token_expires_at: None,
        }];
        let mut state = State::with_accounts(&instance(), temp.path(), accounts);
        state.selected = 10;
        state.begin_edit();
        assert_eq!(state.choice_index, 0);
        assert!(
            !state
                .choice_values()
                .iter()
                .any(|value| value == "Current active account")
        );
        state.handle_choice_key(&KeyEvent::from(KeyCode::Enter));
        assert_eq!(state.draft.preferred_account.as_deref(), Some("account-id"));

        state.toggle_auto_account();
        assert_eq!(state.draft.preferred_account, None);

        state.draft.preferred_account = Some("removed-account".to_owned());
        state.toggle_auto_account();
        assert_eq!(state.draft.preferred_account.as_deref(), Some("account-id"));
    }

    #[test]
    fn command_control_uses_one_compact_checkbox_row() {
        assert_eq!(section_line("Commands").to_string(), "  Commands");
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());
        state.selected = 12;

        let lines = command_field_lines(&state, 12, "Pre-launch");
        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0].to_string().trim(),
            "▌ [ ] Pre-launch      no command"
        );
        assert!(!lines[0].to_string().contains("disabled"));

        state.draft.pre_launch_command.command = "echo ready".to_owned();
        state.draft.pre_launch_command.enabled = true;
        let enabled = command_field_lines(&state, 12, "Pre-launch");
        assert!(enabled[0].to_string().contains("[✓] Pre-launch"));
        assert!(enabled[0].to_string().ends_with("echo ready"));
    }

    #[test]
    fn java_and_custom_glfw_values_include_their_titles() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());
        state.draft.java_path = Some("/custom/java".to_owned());
        state.draft.glfw_path = Some("/usr/lib/libglfw.so.3.5".to_owned());

        assert_eq!(state.display_value(3), "Java  /custom/java");
        assert_eq!(state.display_value(14), "GLFW 3.5  /usr/lib/libglfw.so.3.5");
    }

    #[test]
    fn glfw_picker_offers_bundled_and_custom_modes() {
        let temp = tempfile::tempdir().unwrap();
        let mut state = State::new(&instance(), temp.path());
        state.selected = 14;
        state.begin_edit();
        assert_eq!(state.choice_picker, Some(ChoicePicker::Glfw));
        assert!(
            state
                .choice_values()
                .iter()
                .any(|value| value.contains("GLFW"))
        );
        state.handle_choice_key(&KeyEvent::from(KeyCode::Esc));
        state.handle_key(&KeyEvent::from(KeyCode::Char('c')));
        assert!(state.editing.is_some());
    }
}
