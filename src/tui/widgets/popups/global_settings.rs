// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// modal editor for launcher-wide settings.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, ListItem, Paragraph},
};
use ratatui_textarea::TextArea;

use crate::{
    config::{
        Config,
        settings::{ContentProvider, DEFAULT_RESOLUTION, ImageProtocol, ShortcutHintScope},
        theme::{BORDER_STYLE, BorderStyle, THEME, ThemeConfig},
    },
    instance::models::{memory_kib, normalize_memory_value, parse_resolution},
    tui::widgets::popups::settings_controls::{
        DisplayResolution, JavaChoice, JavaPicker, ResolutionChoice, ResolutionPickerAction,
        SettingsPicker, SettingsPickerAction, SettingsPickerOption, adjust_memory, auto_label,
        default_label, default_resolution, display_resolutions, environment_labels,
        format_tag_values, handle_resolution_picker_key, handle_text_area_input,
        is_default_resolution, parse_environment, parse_tag_values, render_memory_gauge,
        render_settings_picker, resolution_choices, resolution_items, settings_text_area,
        tagged_row_count, tagged_value_lines, toggle_window_mode, window_mode_title,
    },
    tui::widgets::status_badge,
};

const FIELD_ORDER: [usize; 25] = [
    0, 1, 2, 24, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23,
];
const MODRINTH_COLOR: Color = Color::Rgb(0x1B, 0xD9, 0x6A);
const CURSEFORGE_COLOR: Color = Color::Rgb(0xF1, 0x64, 0x36);
const PROVIDER_BADGE_TEXT: Color = Color::Rgb(0x10, 0x10, 0x10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChoicePicker {
    ImageProtocol,
    Resolution,
    ShortcutHints,
}

pub struct State {
    pub config: Config,
    pub theme: ThemeConfig,
    selected: usize,
    editing: Option<TextArea<'static>>,
    error: Option<String>,
    save_pending: bool,
    themes: Vec<String>,
    theme_picker: bool,
    theme_index: usize,
    java_picker_open: bool,
    java_picker: JavaPicker,
    choice_picker: Option<ChoicePicker>,
    settings_picker: SettingsPicker,
    choice_index: usize,
    display_resolutions: Vec<DisplayResolution>,
    detected_image_protocol: &'static str,
}

pub enum Action {
    None,
    Save(Box<Config>),
    Error(String),
    ConfirmJavaAuto,
    ClearCache,
    OpenRaw(std::path::PathBuf),
    Close,
}

impl State {
    pub fn new() -> Self {
        Self::with_detected_image_protocol(ratatui_image::picker::ProtocolType::Halfblocks)
    }

    pub fn with_detected_image_protocol(
        detected_image_protocol: ratatui_image::picker::ProtocolType,
    ) -> Self {
        let theme = crate::config::theme::current_theme_config();
        let themes = available_themes();
        let theme_index = themes
            .iter()
            .position(|candidate| candidate == &theme.theme)
            .unwrap_or(0);
        let runtime_config = crate::config::SETTINGS.read().clone();
        let config_path = crate::config::get_config_path().join("config.toml");
        let config = crate::config::load_config(&config_path).unwrap_or_else(|error| {
            tracing::warn!("Failed to load persisted launcher settings: {error}");
            runtime_config.clone()
        });
        let java_cache =
            crate::storage::MetadataPaths::new(runtime_config.paths.resolve_meta_dir())
                .java_installations();
        let mut java_picker =
            JavaPicker::with_cache(crate::instance::java::detect_java_path(), Some(java_cache));
        java_picker.set_current(config.paths.java_path.as_deref());
        Self {
            config,
            theme,
            selected: 0,
            editing: None,
            error: None,
            save_pending: false,
            themes,
            theme_picker: false,
            theme_index,
            java_picker_open: false,
            java_picker,
            choice_picker: None,
            settings_picker: SettingsPicker::default(),
            choice_index: 0,
            display_resolutions: display_resolutions(),
            detected_image_protocol: match detected_image_protocol {
                ratatui_image::picker::ProtocolType::Halfblocks => "Halfblocks",
                ratatui_image::picker::ProtocolType::Sixel => "Sixel",
                ratatui_image::picker::ProtocolType::Kitty => "Kitty",
                ratatui_image::picker::ProtocolType::Iterm2 => "iTerm2",
            },
        }
    }

    fn value(&self, field: usize) -> String {
        match field {
            0 => self.theme.theme.clone(),
            1 => format!("{:?}", self.theme.border_style).to_lowercase(),
            2 => self.config.ui.image_protocol.to_string(),
            3 => self.config.defaults.memory_min.clone(),
            4 => self.config.defaults.memory_max.clone(),
            5 => self.config.paths.java_path.clone().unwrap_or_default(),
            6 => self.config.defaults.window_mode.to_string(),
            7 => self
                .config
                .defaults
                .resolution
                .map(|(width, height)| format!("{width}x{height}"))
                .unwrap_or_default(),
            8 => format_tag_values(&self.config.defaults.jvm_args),
            9 => format_tag_values(&environment_labels(&self.config.defaults.environment)),
            10 => self.config.content.preferred_provider.to_string(),
            11 => status(self.config.content.preferred_provider_only),
            12 => status(self.config.content.ask_on_provider_conflict),
            13 => status(self.config.general.check_modpack_updates),
            14 => status(self.config.general.check_content_updates),
            15 => self.config.content.unmatched_retry_hours.to_string(),
            16 => self.config.content.max_fingerprint_size_mib.to_string(),
            17 => self.config.paths.instances_dir.clone(),
            18 => self.config.paths.meta_dir.clone(),
            19 => self.config.ui.error_auto_dismiss_ms.to_string(),
            20 => self.config.ui.error_slide_start_ms.to_string(),
            21 => self.config.ui.error_fly_out_ms.to_string(),
            22 => self.config.ui.max_error_events.to_string(),
            24 => shortcut_hint_mode(&self.config.ui).to_owned(),
            _ => String::new(),
        }
    }

    fn display_value(&self, field: usize) -> String {
        match field {
            1 => border_style_title(&self.theme.border_style).to_owned(),
            2 => image_protocol_title(self.config.ui.image_protocol).to_owned(),
            5 => self.java_picker.display_label(
                self.config
                    .paths
                    .java_path
                    .as_deref()
                    .filter(|path| !path.is_empty())
                    .unwrap_or_else(|| self.java_picker.detected_path()),
            ),
            6 => window_mode_title(self.config.defaults.window_mode).to_owned(),
            7 if self.config.defaults.resolution.is_none() => {
                format!("{}x{}", DEFAULT_RESOLUTION.0, DEFAULT_RESOLUTION.1)
            }
            16 if self.config.content.max_fingerprint_size_mib == 0 => "unlimited".to_owned(),
            23 => "provider and Java metadata".to_owned(),
            _ => self.value(field),
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
            3 | 4 if normalize_memory_value(value).is_none() => invalid(
                self,
                "Use a positive memory value ending in K, M, or G.".to_owned(),
            ),
            3 => {
                let value = normalize_memory_value(value).unwrap();
                self.config.defaults.memory_min = value.clone();
                if memory_kib(&value) > memory_kib(&self.config.defaults.memory_max) {
                    self.config.defaults.memory_max = value;
                }
                self.save_pending = true;
            }
            4 => {
                let value = normalize_memory_value(value).unwrap();
                self.config.defaults.memory_max = value.clone();
                if memory_kib(&value) < memory_kib(&self.config.defaults.memory_min) {
                    self.config.defaults.memory_min = value;
                }
                self.save_pending = true;
            }
            5 => {
                self.config.paths.java_path = (!value.is_empty()).then(|| value.to_owned());
                self.save_pending = true;
            }
            7 if value.is_empty() => {
                self.config.defaults.resolution = None;
                self.save_pending = true;
            }
            7 => match parse_resolution(value) {
                Ok(resolution) => {
                    self.config.defaults.resolution = Some(resolution);
                    self.save_pending = true;
                }
                Err(error) => invalid(self, error),
            },
            8 => match parse_tag_values(value) {
                Ok(arguments) => {
                    self.config.defaults.jvm_args = arguments;
                    self.save_pending = true;
                }
                Err(error) => invalid(self, error),
            },
            9 => match parse_environment(value) {
                Ok(environment) => {
                    self.config.defaults.environment = environment;
                    self.save_pending = true;
                }
                Err(error) => invalid(self, error),
            },
            15 => match value.parse::<u64>() {
                Ok(hours) => {
                    self.config.content.unmatched_retry_hours = hours;
                    self.save_pending = true;
                }
                Err(_) => invalid(self, "Use a non-negative number of hours.".to_owned()),
            },
            16 => match value.parse::<u64>() {
                Ok(size) => {
                    self.config.content.max_fingerprint_size_mib = size;
                    self.save_pending = true;
                }
                Err(_) => invalid(self, "Use a non-negative size in MiB.".to_owned()),
            },
            17 | 18 if value.is_empty() => {
                invalid(self, "Storage paths cannot be empty.".to_owned());
            }
            17 => {
                self.config.paths.instances_dir = value.to_owned();
                self.save_pending = true;
            }
            18 => {
                self.config.paths.meta_dir = value.to_owned();
                self.save_pending = true;
            }
            19..=21 => match value.parse::<u64>() {
                Ok(milliseconds) if milliseconds > 0 => {
                    match self.selected {
                        19 => self.config.ui.error_auto_dismiss_ms = milliseconds,
                        20 => self.config.ui.error_slide_start_ms = milliseconds,
                        21 => self.config.ui.error_fly_out_ms = milliseconds,
                        _ => {}
                    }
                    self.save_pending = true;
                }
                _ => invalid(self, "Use a positive duration in milliseconds.".to_owned()),
            },
            22 => match value.parse::<usize>() {
                Ok(count) if count > 0 => {
                    self.config.ui.max_error_events = count;
                    self.save_pending = true;
                }
                _ => invalid(self, "Keep at least one notification.".to_owned()),
            },
            _ => {}
        }
    }

    fn select_theme(&mut self) {
        let previous = self.theme.theme.clone();
        self.theme.theme = self.themes[self.theme_index].clone();
        self.error = None;
        if self.theme.theme == previous {
            return;
        }
        if let Err(error) = crate::config::theme::apply_theme(
            self.theme.theme.clone(),
            self.theme.border_style.clone(),
        ) {
            self.error = Some(error.to_string());
            self.theme.theme = previous;
            self.theme_index = self
                .themes
                .iter()
                .position(|candidate| candidate == &self.theme.theme)
                .unwrap_or(0);
        }
    }

    fn cycle_theme(&mut self, forward: bool) {
        if self.themes.is_empty() {
            return;
        }
        self.theme_index = if forward {
            (self.theme_index + 1) % self.themes.len()
        } else {
            self.theme_index
                .checked_sub(1)
                .unwrap_or(self.themes.len() - 1)
        };
        self.select_theme();
    }

    fn handle_theme_picker_key(&mut self, key: &KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('h') | KeyCode::Left => {
                self.theme_picker = false;
                self.theme_index = self
                    .themes
                    .iter()
                    .position(|candidate| candidate == &self.theme.theme)
                    .unwrap_or(0);
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.theme_index = (self.theme_index + 1).min(self.themes.len() - 1);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.theme_index = self.theme_index.saturating_sub(1);
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
                self.select_theme();
                if self.error.is_none() {
                    self.theme_picker = false;
                }
            }
            _ => {}
        }
    }

    fn cycle_border(&mut self, forward: bool) {
        let previous = self.theme.border_style.clone();
        self.theme.border_style = match (&self.theme.border_style, forward) {
            (BorderStyle::Rounded, true) | (BorderStyle::Double, false) => BorderStyle::Plain,
            (BorderStyle::Plain, true) | (BorderStyle::Thick, false) => BorderStyle::Double,
            (BorderStyle::Double, true) | (BorderStyle::Rounded, false) => BorderStyle::Thick,
            (BorderStyle::Thick, true) | (BorderStyle::Plain, false) => BorderStyle::Rounded,
        };
        self.error = None;
        if let Err(error) = crate::config::theme::apply_theme(
            self.theme.theme.clone(),
            self.theme.border_style.clone(),
        ) {
            self.error = Some(error.to_string());
            self.theme.border_style = previous;
        }
    }

    fn image_protocol_options(&self) -> Vec<SettingsPickerOption> {
        let option = |key: &str, title: &str, description: &str| SettingsPickerOption {
            key: key.to_owned(),
            title: title.to_owned(),
            detail: Some(description.to_owned()),
            leading: None,
            badge: None,
            active: false,
        };
        [
            (
                "kitty",
                image_protocol_title(ImageProtocol::Kitty),
                "Use Kitty graphics when supported",
            ),
            (
                "iterm2",
                image_protocol_title(ImageProtocol::Iterm2),
                "Use the iTerm2 image protocol when supported",
            ),
            (
                "quadrants",
                image_protocol_title(ImageProtocol::Quadrants),
                "Render images with quadrant characters",
            ),
            (
                "halfblocks",
                image_protocol_title(ImageProtocol::Halfblocks),
                "Render images with half-block characters",
            ),
        ]
        .into_iter()
        .map(|(key, title, description)| option(key, title, description))
        .collect()
    }

    fn shortcut_hint_options(&self) -> Vec<SettingsPickerOption> {
        [
            (ShortcutHintScope::Instances, "instances", "Instances"),
            (ShortcutHintScope::Content, "content", "Content"),
            (ShortcutHintScope::Accounts, "accounts", "Accounts"),
            (ShortcutHintScope::Settings, "settings", "Settings"),
            (ShortcutHintScope::Popups, "popups", "Popups"),
        ]
        .into_iter()
        .map(|(scope, key, title)| {
            let visible = self.config.ui.show_shortcut_hints(scope);
            SettingsPickerOption {
                key: key.to_owned(),
                title: format!("{} {title}", if visible { "●" } else { "○" }),
                detail: None,
                leading: None,
                badge: None,
                active: visible,
            }
        })
        .collect()
    }

    fn open_choice_picker(&mut self, picker: ChoicePicker) {
        match picker {
            ChoicePicker::ImageProtocol => {
                let preferred = self.config.ui.image_protocol.to_string();
                self.settings_picker.reset();
                self.settings_picker
                    .sync(self.image_protocol_options(), Some(&preferred));
            }
            ChoicePicker::Resolution => {
                self.choice_index = self
                    .resolution_choices()
                    .iter()
                    .position(|choice| choice.resolution() == self.config.defaults.resolution)
                    .unwrap_or(0);
            }
            ChoicePicker::ShortcutHints => {
                self.settings_picker.reset();
                self.settings_picker
                    .sync(self.shortcut_hint_options(), None);
            }
        }
        self.choice_picker = Some(picker);
    }

    fn handle_choice_picker_key(&mut self, key: &KeyEvent) {
        if self.choice_picker == Some(ChoicePicker::Resolution) {
            let count = self.resolution_choices().len();
            match handle_resolution_picker_key(&mut self.choice_index, count, key) {
                ResolutionPickerAction::Back => self.choice_picker = None,
                ResolutionPickerAction::Default => {
                    self.apply_default_resolution();
                    self.choice_picker = None;
                }
                ResolutionPickerAction::Select => {
                    if let Some(resolution) = self
                        .resolution_choices()
                        .get(self.choice_index)
                        .and_then(|choice| choice.resolution())
                        && self.config.defaults.resolution != Some(resolution)
                    {
                        self.config.defaults.resolution = Some(resolution);
                        self.save_pending = true;
                    }
                    self.choice_picker = None;
                }
                ResolutionPickerAction::None => {}
            }
            return;
        }
        match self.settings_picker.handle_key(key) {
            SettingsPickerAction::Back => self.choice_picker = None,
            SettingsPickerAction::Select => {
                let Some(picker) = self.choice_picker else {
                    return;
                };
                let Some(value) = self.settings_picker.selected_key().map(str::to_owned) else {
                    return;
                };
                if picker == ChoicePicker::ShortcutHints {
                    let scope = match value.as_str() {
                        "instances" => ShortcutHintScope::Instances,
                        "content" => ShortcutHintScope::Content,
                        "accounts" => ShortcutHintScope::Accounts,
                        "settings" => ShortcutHintScope::Settings,
                        "popups" => ShortcutHintScope::Popups,
                        _ => return,
                    };
                    let visible = self.config.ui.show_shortcut_hints(scope);
                    self.config.ui.set_shortcut_hints(scope, !visible);
                    self.settings_picker
                        .sync(self.shortcut_hint_options(), Some(&value));
                    self.save_pending = true;
                    return;
                }
                if picker == ChoicePicker::ImageProtocol {
                    let protocol = match value.as_str() {
                        "kitty" => ImageProtocol::Kitty,
                        "iterm2" => ImageProtocol::Iterm2,
                        "quadrants" => ImageProtocol::Quadrants,
                        "halfblocks" => ImageProtocol::Halfblocks,
                        _ => ImageProtocol::Kitty,
                    };
                    if self.config.ui.image_protocol != protocol {
                        self.config.ui.image_protocol = protocol;
                        self.save_pending = true;
                    }
                }
                self.choice_picker = None;
            }
            SettingsPickerAction::None => {}
        }
    }

    fn resolution_choices(&self) -> Vec<ResolutionChoice> {
        resolution_choices(self.config.defaults.resolution, &self.display_resolutions)
    }

    fn apply_default_resolution(&mut self) {
        let resolution = Some(default_resolution());
        if self.config.defaults.resolution != resolution {
            self.config.defaults.resolution = resolution;
            self.save_pending = true;
        }
        self.error = None;
    }

    fn enable_auto_image_protocol(&mut self) {
        if self.config.ui.image_protocol != ImageProtocol::Auto {
            self.config.ui.image_protocol = ImageProtocol::Auto;
            self.save_pending = true;
        }
        self.error = None;
    }

    fn cycle_provider(&mut self) {
        self.config.content.preferred_provider = match self.config.content.preferred_provider {
            ContentProvider::Modrinth => ContentProvider::CurseForge,
            ContentProvider::CurseForge => ContentProvider::Modrinth,
        };
        self.save_pending = true;
        self.error = None;
    }

    fn cycle_window_mode(&mut self) {
        self.config.defaults.window_mode = toggle_window_mode(self.config.defaults.window_mode);
        self.save_pending = true;
        self.error = None;
    }

    fn cycle_shortcut_hints(&mut self, forward: bool) {
        let current = shortcut_hint_mode(&self.config.ui);
        let index = ["All", "Popups only", "Hidden"]
            .iter()
            .position(|mode| *mode == current);
        let index = match (index, forward) {
            (Some(2), true) | (Some(0), false) => 0,
            (Some(index), true) => index + 1,
            (Some(index), false) => index - 1,
            (None, true) => 0,
            (None, false) => 2,
        };
        self.config.ui.hidden_shortcut_hints = match index {
            1 => [ShortcutHintScope::Main].into_iter().collect(),
            2 => [ShortcutHintScope::All].into_iter().collect(),
            _ => Default::default(),
        };
        self.save_pending = true;
        self.error = None;
    }

    fn open_java_picker(&mut self) {
        self.java_picker
            .open(self.config.paths.java_path.as_deref());
        self.java_picker.initialize();
        self.java_picker_open = true;
    }

    fn toggle_auto_java(&mut self) -> Action {
        let Some(current) = self.config.paths.java_path.as_deref() else {
            self.config.paths.java_path = Some(self.java_picker.detected_path().to_owned());
            self.save_pending = true;
            self.error = None;
            return Action::None;
        };
        if self.java_picker.automatic_change(current) {
            return Action::ConfirmJavaAuto;
        }
        self.enable_auto_java();
        Action::None
    }

    fn enable_auto_java(&mut self) {
        self.config.paths.java_path = None;
        self.save_pending = true;
        self.error = None;
    }

    pub fn confirm_auto_java(&mut self) -> Action {
        self.enable_auto_java();
        self.save_pending = false;
        Action::Save(Box::new(self.config.clone()))
    }

    pub fn mark_save_failed(&mut self) {
        self.save_pending = true;
    }

    pub fn invalidate_java_cache(&mut self) {
        self.java_picker.invalidate_cache();
    }

    fn handle_java_picker_key(&mut self, key: &KeyEvent) {
        self.java_picker.initialize();
        match self.java_picker.selection_mut().handle_key(key) {
            SettingsPickerAction::Back => self.java_picker_open = false,
            SettingsPickerAction::Select => {
                match self.java_picker.selected_choice() {
                    JavaChoice::Installation(path) => {
                        if self.config.paths.java_path.as_deref() != Some(&path) {
                            self.config.paths.java_path = Some(path);
                            self.save_pending = true;
                        }
                    }
                }
                self.java_picker_open = false;
            }
            SettingsPickerAction::None => {}
        }
    }

    fn adjust_selected_memory(&mut self, forward: bool) {
        let previous_min = self.config.defaults.memory_min.clone();
        let previous_max = self.config.defaults.memory_max.clone();
        let value = if self.selected == 3 {
            &self.config.defaults.memory_min
        } else {
            &self.config.defaults.memory_max
        };
        let value = adjust_memory(value, forward);
        if self.selected == 3 {
            self.config.defaults.memory_min = value.clone();
            if memory_kib(&value) > memory_kib(&self.config.defaults.memory_max) {
                self.config.defaults.memory_max = value;
            }
        } else {
            self.config.defaults.memory_max = value.clone();
            if memory_kib(&value) < memory_kib(&self.config.defaults.memory_min) {
                self.config.defaults.memory_min = value;
            }
        }
        self.save_pending |= self.config.defaults.memory_min != previous_min
            || self.config.defaults.memory_max != previous_max;
        self.error = None;
    }

    fn toggle_selected(&mut self) {
        match self.selected {
            11 => {
                self.config.content.preferred_provider_only =
                    !self.config.content.preferred_provider_only;
            }
            12 => {
                self.config.content.ask_on_provider_conflict =
                    !self.config.content.ask_on_provider_conflict;
            }
            13 => {
                self.config.general.check_modpack_updates =
                    !self.config.general.check_modpack_updates;
            }
            14 => {
                self.config.general.check_content_updates =
                    !self.config.general.check_content_updates;
            }
            _ => return,
        }
        self.save_pending = true;
        self.error = None;
    }

    pub fn handle_key(&mut self, key: &KeyEvent) -> Action {
        let action = self.handle_key_inner(key);
        if !matches!(action, Action::None) {
            return action;
        }
        if let Some(error) = self.error.take() {
            return Action::Error(error);
        }
        if self.save_pending && self.editing.is_none() {
            self.save_pending = false;
            self.config = self.config.clone().normalize();
            return Action::Save(Box::new(self.config.clone()));
        }
        Action::None
    }

    fn handle_key_inner(&mut self, key: &KeyEvent) -> Action {
        if self.choice_picker.is_some() {
            self.handle_choice_picker_key(key);
            return Action::None;
        }
        if self.java_picker_open {
            self.handle_java_picker_key(key);
            return Action::None;
        }
        if self.theme_picker {
            self.handle_theme_picker_key(key);
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
            KeyCode::Char('j') | KeyCode::Down => {
                let position = FIELD_ORDER
                    .iter()
                    .position(|field| *field == self.selected)
                    .unwrap_or(0);
                self.selected = FIELD_ORDER[(position + 1).min(FIELD_ORDER.len() - 1)];
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let position = FIELD_ORDER
                    .iter()
                    .position(|field| *field == self.selected)
                    .unwrap_or(0);
                self.selected = FIELD_ORDER[position.saturating_sub(1)];
            }
            KeyCode::Char('h') | KeyCode::Left if self.selected == 0 => self.cycle_theme(false),
            KeyCode::Char('l') | KeyCode::Right if self.selected == 0 => self.cycle_theme(true),
            KeyCode::Char('h') | KeyCode::Left if self.selected == 1 => self.cycle_border(false),
            KeyCode::Char('l') | KeyCode::Right if self.selected == 1 => self.cycle_border(true),
            KeyCode::Char('h') | KeyCode::Left if matches!(self.selected, 3 | 4) => {
                self.adjust_selected_memory(false);
            }
            KeyCode::Char('l') | KeyCode::Right if matches!(self.selected, 3 | 4) => {
                self.adjust_selected_memory(true);
            }
            KeyCode::Char('h') | KeyCode::Left if self.selected == 6 => self.cycle_window_mode(),
            KeyCode::Char('l') | KeyCode::Right if self.selected == 6 => self.cycle_window_mode(),
            KeyCode::Char('h') | KeyCode::Left if self.selected == 10 => self.cycle_provider(),
            KeyCode::Char('l') | KeyCode::Right if self.selected == 10 => self.cycle_provider(),
            KeyCode::Char('h') | KeyCode::Left if self.selected == 24 => {
                self.cycle_shortcut_hints(false);
            }
            KeyCode::Char('l') | KeyCode::Right if self.selected == 24 => {
                self.cycle_shortcut_hints(true);
            }
            KeyCode::Enter => match self.selected {
                0 => self.theme_picker = true,
                1 => self.cycle_border(true),
                2 => self.open_choice_picker(ChoicePicker::ImageProtocol),
                3 | 4 => {
                    self.editing = Some(settings_text_area(vec![self.value(self.selected)]));
                }
                5 => self.open_java_picker(),
                6 => self.cycle_window_mode(),
                7 => self.open_choice_picker(ChoicePicker::Resolution),
                10 => self.cycle_provider(),
                11..=14 => self.toggle_selected(),
                24 => self.open_choice_picker(ChoicePicker::ShortcutHints),
                23 => return Action::ClearCache,
                field => self.editing = Some(settings_text_area(vec![self.value(field)])),
            },
            KeyCode::Char('a') if self.selected == 2 => self.enable_auto_image_protocol(),
            KeyCode::Char('a') if self.selected == 5 => return self.toggle_auto_java(),
            KeyCode::Char('c') if self.selected == 5 => {
                self.editing = Some(settings_text_area(vec![self.value(5)]));
            }
            KeyCode::Char('c') if self.selected == 7 => {
                self.editing = Some(settings_text_area(vec![self.value(7)]));
            }
            KeyCode::Char('d') if self.selected == 7 => {
                self.apply_default_resolution();
            }
            KeyCode::Char('E') => {
                let file = if self.selected <= 1 {
                    "theme.toml"
                } else {
                    "config.toml"
                };
                return Action::OpenRaw(crate::config::get_config_path().join(file));
            }
            KeyCode::Esc => return Action::Close,
            _ => {}
        }
        Action::None
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

fn available_themes() -> Vec<String> {
    let mut themes: Vec<String> = ratatui_themekit::available_theme_ids()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let theme_dir = crate::config::get_config_path().join("theme");
    if let Ok(entries) = std::fs::read_dir(theme_dir) {
        themes.extend(entries.flatten().filter_map(|entry| {
            let path = entry.path();
            (path.extension().and_then(|ext| ext.to_str()) == Some("toml"))
                .then(|| path.file_stem()?.to_str().map(str::to_owned))
                .flatten()
        }));
    }
    themes.sort();
    themes.dedup();
    let current = crate::config::theme::current_theme_config().theme;
    if !themes.contains(&current) {
        themes.push(current);
        themes.sort();
    }
    themes
}

pub fn popup_rect(area: Rect, state: &State) -> Rect {
    let form_width = (area.width * 86 / 100).saturating_sub(2);
    let form_height = 38
        + tagged_row_count(&state.config.defaults.jvm_args, form_width).saturating_sub(1) as u16
        + tagged_row_count(
            &environment_labels(&state.config.defaults.environment),
            form_width,
        )
        .saturating_sub(1) as u16;
    let height = if state.theme_picker || state.java_picker_open {
        (area.height * 2 / 3).max(10)
    } else if state.choice_picker == Some(ChoicePicker::Resolution) {
        10
    } else if state.choice_picker.is_some() {
        (area.height / 2).max(10)
    } else {
        form_height
    };
    let width = match state.choice_picker {
        Some(ChoicePicker::Resolution) => 64,
        Some(ChoicePicker::ImageProtocol | ChoicePicker::ShortcutHints) => 60,
        None if state.java_picker_open => 72,
        None if state.theme_picker => 52,
        None => 86,
    };
    area.centered(
        ratatui::layout::Constraint::Percentage(width),
        ratatui::layout::Constraint::Length(height.min(area.height)),
    )
}

pub fn render(frame: &mut Frame, area: Rect, state: &mut State) {
    if state.java_picker_open {
        state.java_picker.initialize();
    }
    let theme = THEME.as_ref();
    frame.render_widget(Clear, area);
    let keybinds = if state.editing.is_some() {
        super::keybind_line(&[("Enter", " apply"), ("Esc", " cancel")])
    } else if state.choice_picker == Some(ChoicePicker::Resolution) {
        super::keybind_line(&[("d", " default"), ("h", " back"), ("Enter", " select")])
    } else if state.choice_picker == Some(ChoicePicker::ShortcutHints) {
        super::keybind_line(&[("Enter", " toggle"), ("h", " back")])
    } else if state.java_picker_open || state.theme_picker || state.choice_picker.is_some() {
        super::keybind_line(&[("h", " back"), ("Enter", " select")])
    } else if matches!(state.selected, 3 | 4) {
        super::keybind_line(&[("h/l", " adjust"), ("Enter", " exact"), ("Esc", " back")])
    } else if state.selected == 5 {
        super::keybind_line(&[
            ("Enter", " runtimes"),
            ("a", " auto"),
            ("c", " custom"),
            ("Esc", " back"),
        ])
    } else if state.selected == 2 {
        super::keybind_line(&[
            ("Enter", " protocols"),
            ("a", " auto"),
            ("E", " raw"),
            ("Esc", " back"),
        ])
    } else if matches!(state.selected, 6 | 10) {
        super::keybind_line(&[("h/l", " adjust"), ("Enter", " next"), ("Esc", " back")])
    } else if state.selected == 7 {
        super::keybind_line(&[
            ("Enter", " presets"),
            ("c", " custom"),
            ("d", " default"),
            ("Esc", " back"),
        ])
    } else if matches!(state.selected, 11..=14) {
        super::keybind_line(&[("Enter", " toggle"), ("E", " raw"), ("Esc", " back")])
    } else if state.selected == 23 {
        super::keybind_line(&[("Enter", " clear"), ("Esc", " back")])
    } else if state.selected == 0 {
        super::keybind_line(&[
            ("h/l", " adjust"),
            ("Enter", " themes"),
            ("E", " raw"),
            ("Esc", " back"),
        ])
    } else if state.selected == 1 {
        super::keybind_line(&[
            ("h/l", " adjust"),
            ("Enter", " next"),
            ("E", " raw"),
            ("Esc", " back"),
        ])
    } else if state.selected == 24 {
        super::keybind_line(&[
            ("h/l", " adjust"),
            ("Enter", " customize"),
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
    let title = if state.theme_picker {
        " Theme "
    } else if state.java_picker_open {
        " Java Runtime "
    } else if let Some(picker) = state.choice_picker {
        match picker {
            ChoicePicker::ImageProtocol => " Image Rendering ",
            ChoicePicker::Resolution => " Default Resolution ",
            ChoicePicker::ShortcutHints => " Shortcut Guides ",
        }
    } else {
        " Launcher Settings "
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BORDER_STYLE.to_border_type())
        .border_style(Style::default().fg(theme.text_dim()))
        .style(Style::default().bg(theme.surface()))
        .title_bottom(keybinds.right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if state.theme_picker {
        render_picker(frame, inner, &state.themes, state.theme_index);
        return;
    }
    if state.java_picker_open {
        render_java_picker(frame, inner, state);
        return;
    }
    if state.choice_picker.is_some() {
        if state.choice_picker == Some(ChoicePicker::Resolution) {
            let items = resolution_items(&state.resolution_choices(), state.choice_index);
            super::select_list::render_styled(items, state.choice_index, inner, frame.buffer_mut());
        } else {
            render_settings_picker(&state.settings_picker, inner, frame.buffer_mut());
        }
        return;
    }
    render_settings_list(frame, inner, state);
}

fn render_picker(frame: &mut Frame, area: Rect, values: &[String], selected: usize) {
    let theme = THEME.as_ref();
    let items = values
        .iter()
        .map(|name| {
            ListItem::new(Line::from(Span::styled(
                name.clone(),
                Style::default().fg(theme.text()),
            )))
        })
        .collect();
    super::select_list::render(items, selected, area, frame.buffer_mut());
}

fn render_java_picker(frame: &mut Frame, area: Rect, state: &mut State) {
    let theme = THEME.as_ref();
    let mut list_area = area;
    if let Some(status) = state.java_picker.take_status() {
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
    render_settings_picker(state.java_picker.selection(), list_area, frame.buffer_mut());
}

fn render_settings_list(frame: &mut Frame, area: Rect, state: &State) {
    let theme = THEME.as_ref();
    let sections: [(&str, &[usize]); 6] = [
        ("Appearance", &[0, 1, 2, 24]),
        ("Launch Defaults", &[3, 4, 5, 6, 7, 8, 9]),
        ("Content", &[10, 11, 12, 13, 14, 15, 16]),
        ("Storage", &[17, 18]),
        ("Notifications", &[19, 20, 21, 22]),
        ("Maintenance", &[23]),
    ];
    let jvm_args = &state.config.defaults.jvm_args;
    let environment = environment_labels(&state.config.defaults.environment);
    let mut rows: Vec<(Option<usize>, Vec<Line<'static>>)> = Vec::new();
    for (section_index, (title, fields)) in sections.iter().enumerate() {
        if section_index > 0 {
            rows.push((None, vec![Line::default()]));
        }
        rows.push((None, vec![section_line(title)]));
        for index in *fields {
            let label = field_label(*index);
            let lines = match index {
                8 => tagged_value_lines(
                    global_field_line(state, *index, label).spans,
                    state.selected == *index,
                    state.editing.is_some(),
                    jvm_args,
                    "no arguments",
                    area.width,
                ),
                9 => tagged_value_lines(
                    global_field_line(state, *index, label).spans,
                    state.selected == *index,
                    state.editing.is_some(),
                    &environment,
                    "no variables",
                    area.width,
                ),
                23 => vec![maintenance_line(state)],
                _ => vec![global_field_line(state, *index, label)],
            };
            rows.push((Some(*index), lines));
        }
    }

    let mut row_start = 0u16;
    for (field, lines) in rows {
        let height = lines.len() as u16;
        if row_start >= area.height {
            break;
        }
        let y = area.y.saturating_add(row_start);
        let visible_height = height.min(area.height.saturating_sub(row_start));
        let selected = field == Some(state.selected);
        frame.render_widget(
            Paragraph::new(
                lines
                    .into_iter()
                    .take(visible_height as usize)
                    .collect::<Vec<_>>(),
            )
            .style(Style::default().bg(if selected {
                theme.stripe()
            } else {
                theme.surface()
            })),
            Rect {
                y,
                height: visible_height,
                ..area
            },
        );
        if let Some(field @ (3 | 4)) = field {
            let value = state.value(field);
            render_memory_gauge(
                frame,
                Rect {
                    x: area.x.saturating_add(20),
                    y: area.y.saturating_add(row_start),
                    width: area.width.saturating_sub(21),
                    height: 1,
                },
                &value,
                value.clone(),
                state.selected == field,
            );
        }
        if field == Some(state.selected)
            && let Some(editor) = state.editing.as_ref()
        {
            frame.render_widget(
                editor,
                Rect {
                    x: area.x.saturating_add(20),
                    y: area.y.saturating_add(row_start),
                    width: area.width.saturating_sub(20),
                    height: 1,
                },
            );
        }
        row_start = row_start.saturating_add(height);
    }
}

fn maintenance_line(state: &State) -> Line<'static> {
    let theme = THEME.as_ref();
    let selected = state.selected == 23;
    Line::from(vec![
        Span::styled(
            if selected { "▌ " } else { "  " },
            Style::default().fg(theme.accent()),
        ),
        Span::styled(
            "Clear caches",
            Style::default()
                .fg(if selected {
                    theme.accent()
                } else {
                    theme.warning()
                })
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "   Rebuild provider and Java metadata",
            Style::default().fg(if selected {
                theme.text()
            } else {
                theme.text_dim()
            }),
        ),
    ])
}

fn section_line(title: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("  {title}"),
        Style::default()
            .fg(THEME.as_ref().text())
            .add_modifier(Modifier::BOLD),
    ))
}

fn field_label(index: usize) -> &'static str {
    match index {
        0 => "Theme",
        1 => "Border style",
        2 => "Image rendering",
        3 => "Memory min",
        4 => "Memory max",
        5 => "Java",
        6 => "Window mode",
        7 => "Resolution",
        8 => "JVM arguments",
        9 => "Environment",
        10 => "Preferred provider",
        11 => "Provider only",
        12 => "Ask on conflict",
        13 => "Modpack updates",
        14 => "Content updates",
        15 => "Retry hours",
        16 => "Fingerprint MiB",
        17 => "Instances",
        18 => "Metadata",
        19 => "Dismiss ms",
        20 => "Slide start ms",
        21 => "Fly-out ms",
        22 => "Max notifications",
        23 => "",
        24 => "Shortcut guides",
        _ => "",
    }
}

fn global_field_line(state: &State, index: usize, label: &str) -> Line<'static> {
    let theme = THEME.as_ref();
    let selected = index == state.selected;
    let editing = selected && state.editing.is_some();
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
            if editing || matches!(index, 3 | 4 | 8 | 9 | 10) {
                String::new()
            } else if index == 1 {
                format!(
                    "{}  {}",
                    border_preview(&state.theme.border_style),
                    state.display_value(index)
                )
            } else if index == 2 && state.config.ui.image_protocol == ImageProtocol::Auto {
                state.detected_image_protocol.to_owned()
            } else {
                state.display_value(index)
            },
            Style::default()
                .fg(if selected {
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
    if index == 5 && state.config.paths.java_path.is_none() && !editing {
        spans.extend([Span::raw("  "), auto_label()]);
    }
    if index == 2 && state.config.ui.image_protocol == ImageProtocol::Auto && !editing {
        spans.extend([Span::raw("  "), auto_label()]);
    }
    if index == 7 && !editing && is_default_resolution(state.config.defaults.resolution) {
        spans.extend([Span::raw("  "), default_label()]);
    }
    if index == 10 && !editing {
        spans.push(Span::raw("  "));
        spans.push(provider_badge(state.config.content.preferred_provider));
    }
    if restart_required_for(state, index) {
        spans.extend([
            Span::raw("  "),
            status_badge("Restart", THEME.as_ref().warning()),
        ]);
    }
    Line::from(spans).style(Style::default().bg(if selected {
        theme.stripe()
    } else {
        theme.surface()
    }))
}

fn border_preview(style: &BorderStyle) -> &'static str {
    match style {
        BorderStyle::Plain => "┌─",
        BorderStyle::Rounded => "╭─",
        BorderStyle::Double => "╔═",
        BorderStyle::Thick => "┏━",
    }
}

fn provider_badge(provider: ContentProvider) -> Span<'static> {
    let (label, color) = match provider {
        ContentProvider::Modrinth => ("Modrinth", MODRINTH_COLOR),
        ContentProvider::CurseForge => ("CurseForge", CURSEFORGE_COLOR),
    };
    Span::styled(
        format!(" {label} "),
        Style::default()
            .fg(PROVIDER_BADGE_TEXT)
            .bg(color)
            .add_modifier(Modifier::BOLD),
    )
}

fn border_style_title(style: &BorderStyle) -> &'static str {
    match style {
        BorderStyle::Plain => "Plain",
        BorderStyle::Rounded => "Rounded",
        BorderStyle::Double => "Double",
        BorderStyle::Thick => "Thick",
    }
}

fn image_protocol_title(protocol: ImageProtocol) -> &'static str {
    match protocol {
        ImageProtocol::Auto => "Auto",
        ImageProtocol::Halfblocks => "Halfblocks",
        ImageProtocol::Quadrants => "Quadrants",
        ImageProtocol::Kitty => "Kitty",
        ImageProtocol::Iterm2 => "iTerm2",
    }
}

fn restart_required_for(state: &State, index: usize) -> bool {
    let current = crate::config::SETTINGS.read();
    match index {
        2 => state.config.ui.image_protocol != current.ui.image_protocol,
        17 => state.config.paths.instances_dir != current.paths.instances_dir,
        18 => state.config.paths.meta_dir != current.paths.meta_dir,
        _ => false,
    }
}

fn status(enabled: bool) -> String {
    if enabled { "enabled" } else { "disabled" }.to_owned()
}

fn shortcut_hint_mode(ui: &crate::config::settings::Ui) -> &'static str {
    let visible = ShortcutHintScope::AREAS.map(|scope| ui.show_shortcut_hints(scope));
    match visible {
        [true, true, true, true, true] => "All",
        [false, false, false, false, true] => "Popups only",
        [false, false, false, false, false] => "Hidden",
        _ => "Custom",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance::models::WindowMode;

    #[test]
    fn launcher_memory_uses_slider_and_java_uses_picker() {
        let mut state = State::new();
        state.selected = 3;
        let original = state.config.defaults.memory_min.clone();
        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Char('l'))),
            Action::Save(..)
        ));
        assert_ne!(state.config.defaults.memory_min, original);
        assert!(state.editing.is_none());

        state.handle_key(&KeyEvent::from(KeyCode::Enter));
        assert!(state.editing.is_some());

        state.editing = None;
        state.selected = 5;
        state.handle_key(&KeyEvent::from(KeyCode::Enter));
        assert!(state.java_picker_open);
    }

    #[test]
    fn java_auto_mode_toggles_to_and_from_the_detected_path() {
        let mut state = State::new();
        state.selected = 5;
        state.config.paths.java_path = None;

        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Char('a'))),
            Action::Save(..)
        ));
        assert_eq!(
            state.config.paths.java_path.as_deref(),
            Some(state.java_picker.detected_path())
        );

        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Char('a'))),
            Action::Save(..)
        ));
        assert_eq!(state.config.paths.java_path, None);
    }

    #[test]
    fn java_picker_does_not_toggle_auto_mode() {
        let mut state = State::new();
        state.selected = 5;
        state.config.paths.java_path = Some("/custom/java".to_owned());
        state.open_java_picker();

        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Char('a'))),
            Action::None
        ));
        assert!(state.java_picker_open);
        assert_eq!(
            state.config.paths.java_path.as_deref(),
            Some("/custom/java")
        );
    }

    #[test]
    fn changing_runtime_to_auto_requests_confirmation() {
        let mut state = State::new();
        state.selected = 5;
        state.config.paths.java_path = Some("/custom/java".to_owned());

        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Char('a'))),
            Action::ConfirmJavaAuto
        ));
        assert_eq!(
            state.config.paths.java_path.as_deref(),
            Some("/custom/java")
        );
    }

    #[test]
    fn launcher_choices_toggles_and_maintenance_are_interactive() {
        let mut state = State::new();
        state.selected = 2;
        state.handle_key(&KeyEvent::from(KeyCode::Enter));
        assert_eq!(state.choice_picker, Some(ChoicePicker::ImageProtocol));
        assert!(
            !state
                .settings_picker
                .labels()
                .iter()
                .any(|label| label == "Auto")
        );
        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Enter)),
            Action::Save(..)
        ));
        assert_eq!(state.config.ui.image_protocol, ImageProtocol::Kitty);
        assert_eq!(state.display_value(2), "Kitty");
        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Char('a'))),
            Action::Save(..)
        ));
        assert_eq!(state.config.ui.image_protocol, ImageProtocol::Auto);

        state.selected = 10;
        let provider = state.config.content.preferred_provider;
        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Enter)),
            Action::Save(..)
        ));
        assert!(state.choice_picker.is_none());
        assert_ne!(state.config.content.preferred_provider, provider);

        state.selected = 11;
        let previous = state.config.content.preferred_provider_only;
        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Enter)),
            Action::Save(..)
        ));
        assert_eq!(state.config.content.preferred_provider_only, !previous);

        state.selected = 23;
        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Enter)),
            Action::ClearCache
        ));
    }

    #[test]
    fn choice_values_use_their_display_titles() {
        let mut state = State::new();
        state.config.ui.image_protocol = ImageProtocol::Halfblocks;
        state.config.defaults.window_mode = WindowMode::Fullscreen;

        assert_eq!(
            state.display_value(1),
            border_style_title(&state.theme.border_style)
        );
        assert_eq!(state.display_value(2), "Halfblocks");
        assert_eq!(state.display_value(6), "Fullscreen");
    }

    #[test]
    fn provider_badges_use_fixed_brand_colors() {
        assert_eq!(
            provider_badge(ContentProvider::Modrinth).style.bg,
            Some(MODRINTH_COLOR)
        );
        assert_eq!(
            provider_badge(ContentProvider::CurseForge).style.bg,
            Some(CURSEFORGE_COLOR)
        );
    }

    #[test]
    fn launcher_jvm_and_environment_defaults_share_tag_editing() {
        let mut state = State::new();
        state.selected = 8;
        state.editing = Some(settings_text_area(vec!["-Xfoo -Xbar".to_owned()]));
        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Enter)),
            Action::Save(..)
        ));
        assert_eq!(state.config.defaults.jvm_args, ["-Xfoo", "-Xbar"]);

        state.selected = 9;
        state.editing = Some(settings_text_area(vec!["FOO=bar BAZ=qux".to_owned()]));
        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Enter)),
            Action::Save(..)
        ));
        assert_eq!(
            state.config.defaults.environment.get("FOO"),
            Some(&"bar".to_owned())
        );
    }

    #[test]
    fn launcher_resolution_reuses_display_and_preset_choices() {
        let mut state = State::new();
        state.config.defaults.resolution = None;
        assert_eq!(state.display_value(7), "854x480");
        state.display_resolutions = vec![DisplayResolution {
            width: 2560,
            height: 1440,
            name: "DP-4".to_owned(),
            primary: true,
        }];
        state.selected = 7;

        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Char('d'))),
            Action::Save(..)
        ));
        assert_eq!(state.config.defaults.resolution, Some((854, 480)));
        assert!(
            global_field_line(&state, 7, field_label(7))
                .to_string()
                .contains("Default")
        );
        state.handle_key(&KeyEvent::from(KeyCode::Enter));
        assert_eq!(state.choice_picker, Some(ChoicePicker::Resolution));
        assert!(matches!(
            state.resolution_choices().first(),
            Some(ResolutionChoice::Display(display)) if display.name == "DP-4"
        ));
    }

    #[test]
    fn appearance_rows_use_inline_controls() {
        let mut state = State::new();
        assert!(matches!(
            border_preview(&state.theme.border_style),
            "┌─" | "╭─" | "╔═" | "┏━"
        ));

        state.selected = 0;
        state.handle_key(&KeyEvent::from(KeyCode::Enter));
        assert!(state.theme_picker);
        let active_theme = state.theme.theme.clone();
        state.handle_key(&KeyEvent::from(KeyCode::Down));
        state.handle_key(&KeyEvent::from(KeyCode::Esc));
        assert!(!state.theme_picker);
        assert_eq!(state.themes[state.theme_index], active_theme);
    }

    #[test]
    fn shortcut_guides_offer_presets_and_selective_toggles() {
        let mut state = State::new();
        state.config.ui.hidden_shortcut_hints.clear();
        state.selected = 2;
        state.handle_key(&KeyEvent::from(KeyCode::Down));
        assert_eq!(state.selected, 24);

        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Right)),
            Action::Save(..)
        ));
        assert_eq!(shortcut_hint_mode(&state.config.ui), "Popups only");

        state.handle_key(&KeyEvent::from(KeyCode::Enter));
        assert_eq!(state.choice_picker, Some(ChoicePicker::ShortcutHints));
        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Enter)),
            Action::Save(..)
        ));
        assert_eq!(shortcut_hint_mode(&state.config.ui), "Custom");
        assert!(
            state
                .config
                .ui
                .show_shortcut_hints(ShortcutHintScope::Instances)
        );
    }

    #[test]
    fn failed_launcher_save_remains_pending() {
        let mut state = State::new();
        state.config.defaults.jvm_args.push("-Xretry".to_owned());
        state.mark_save_failed();

        assert!(matches!(
            state.handle_key(&KeyEvent::from(KeyCode::Down)),
            Action::Save(..)
        ));
    }

    #[test]
    fn expanded_form_keeps_all_sections_visible_without_scrolling() {
        let mut state =
            State::with_detected_image_protocol(ratatui_image::picker::ProtocolType::Kitty);
        state.selected = 23;
        state.config.defaults.jvm_args = vec!["-Xfoo".to_owned()];
        state
            .config
            .defaults
            .environment
            .insert("FOO".to_owned(), "bar".to_owned());
        let backend = ratatui::backend::TestBackend::new(120, 45);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                let area = popup_rect(frame.area(), &state);
                assert!(area.height >= 37);
                render(frame, area, &mut state);
            })
            .unwrap();

        let screen = terminal.backend().to_string();
        assert!(screen.contains("Appearance"));
        assert!(screen.contains("Maintenance"));
        assert!(screen.contains("Kitty"));
        assert!(screen.contains("Auto"));
        assert!(screen.contains("Clear caches"));
        assert!(screen.contains("Rebuild provider and Java metadata"));
        assert!(
            ["┌─", "╭─", "╔═", "┏━"]
                .iter()
                .any(|preview| screen.contains(preview))
        );
        assert_eq!(screen.matches("-Xfoo").count(), 1);
        assert_eq!(screen.matches("FOO=bar").count(), 1);
    }
}
