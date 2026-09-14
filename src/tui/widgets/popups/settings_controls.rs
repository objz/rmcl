// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// Reusable interactive controls shared by instance and launcher settings.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{LineGauge, ListItem, Paragraph},
};
use ratatui_textarea::{CursorMove, TextArea};

use crate::{
    config::{settings::DEFAULT_RESOLUTION, theme::THEME},
    instance::{
        java::JavaInstallation,
        models::{WindowMode, memory_kib},
    },
    tui::widgets::{popups::LoadState, status_badge},
};

const MEMORY_STEPS: [&str; 12] = [
    "512M", "1G", "2G", "3G", "4G", "6G", "8G", "12G", "16G", "24G", "32G", "64G",
];

pub(crate) fn toggle_window_mode(mode: WindowMode) -> WindowMode {
    match mode {
        WindowMode::Windowed => WindowMode::Fullscreen,
        WindowMode::Fullscreen => WindowMode::Windowed,
    }
}

pub(crate) fn window_mode_title(mode: WindowMode) -> &'static str {
    match mode {
        WindowMode::Windowed => "Windowed",
        WindowMode::Fullscreen => "Fullscreen",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsPickerBadge {
    Auto,
    Bundled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SettingsPickerOption {
    pub key: String,
    pub title: String,
    pub detail: Option<String>,
    pub leading: Option<String>,
    pub active: bool,
    pub badge: Option<SettingsPickerBadge>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingsPickerAction {
    None,
    Back,
    Select,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SettingsPicker {
    options: Vec<SettingsPickerOption>,
    selected: usize,
}

impl SettingsPicker {
    pub(crate) fn reset(&mut self) {
        self.options.clear();
        self.selected = 0;
    }

    pub(crate) fn sync(&mut self, options: Vec<SettingsPickerOption>, preferred: Option<&str>) {
        let selected_key = self
            .options
            .get(self.selected)
            .map(|option| option.key.clone())
            .or_else(|| preferred.map(str::to_owned));
        self.options = options;
        self.selected = selected_key
            .as_deref()
            .and_then(|key| self.options.iter().position(|option| option.key == key))
            .unwrap_or(0);
    }

    pub(crate) fn handle_key(&mut self, key: &KeyEvent) -> SettingsPickerAction {
        match key.code {
            KeyCode::Esc | KeyCode::Char('h') | KeyCode::Left => SettingsPickerAction::Back,
            KeyCode::Char('j') | KeyCode::Down if !self.options.is_empty() => {
                self.selected = (self.selected + 1).min(self.options.len() - 1);
                SettingsPickerAction::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                SettingsPickerAction::None
            }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right if !self.options.is_empty() => {
                SettingsPickerAction::Select
            }
            _ => SettingsPickerAction::None,
        }
    }

    pub(crate) fn selected(&self) -> usize {
        self.selected
    }

    pub(crate) fn selected_key(&self) -> Option<&str> {
        self.options
            .get(self.selected)
            .map(|option| option.key.as_str())
    }

    pub(crate) fn labels(&self) -> Vec<String> {
        self.options
            .iter()
            .map(|option| {
                option.detail.as_ref().map_or_else(
                    || option.title.clone(),
                    |detail| format!("{}  {detail}", option.title),
                )
            })
            .collect()
    }

    pub(crate) fn items(&self) -> Vec<ListItem<'static>> {
        let theme = THEME.as_ref();
        self.options
            .iter()
            .enumerate()
            .map(|(index, option)| {
                let selected = index == self.selected;
                let title_style = if selected {
                    Style::default()
                        .fg(theme.accent())
                        .add_modifier(Modifier::BOLD)
                } else if option.active {
                    Style::default()
                        .fg(theme.text())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text())
                };
                let mut spans = Vec::new();
                if let Some(leading) = &option.leading {
                    spans.push(Span::styled(
                        leading.clone(),
                        Style::default().fg(theme.success()),
                    ));
                }
                spans.push(Span::styled(option.title.clone(), title_style));
                if let Some(detail) = &option.detail {
                    spans.push(Span::styled(
                        format!("  {detail}"),
                        Style::default()
                            .fg(if selected {
                                theme.accent()
                            } else {
                                theme.text_dim()
                            })
                            .add_modifier(if selected {
                                Modifier::BOLD
                            } else {
                                Modifier::empty()
                            }),
                    ));
                }
                if let Some(badge) = option.badge {
                    spans.extend([
                        Span::raw("  "),
                        status_badge(
                            match badge {
                                SettingsPickerBadge::Auto => "Auto",
                                SettingsPickerBadge::Bundled => "Bundled",
                            },
                            theme.success(),
                        ),
                    ]);
                }
                ListItem::new(Line::from(spans))
            })
            .collect()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct JavaPicker {
    load: Arc<Mutex<LoadState<Vec<JavaInstallation>>>>,
    current: Option<String>,
    detected: String,
    picker: SettingsPicker,
    cache_path: Option<PathBuf>,
    refresh_started: bool,
    generation: Arc<AtomicU64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum JavaChoice {
    Installation(String),
}

#[derive(Debug, Clone)]
pub(crate) struct GlfwPicker {
    load: Arc<Mutex<LoadState<Vec<GlfwInstallation>>>>,
    current: Option<String>,
    bundled_version: Option<String>,
    picker: SettingsPicker,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GlfwChoice {
    Bundled,
    System(String),
}

#[derive(Debug, Clone)]
struct GlfwInstallation {
    path: PathBuf,
    version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DisplayResolution {
    pub width: u32,
    pub height: u32,
    pub name: String,
    pub primary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResolutionChoice {
    Display(DisplayResolution),
    Preset(u32, u32),
    Configured(u32, u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResolutionPickerAction {
    None,
    Back,
    Default,
    Select,
}

pub(crate) fn handle_resolution_picker_key(
    selected: &mut usize,
    count: usize,
    key: &KeyEvent,
) -> ResolutionPickerAction {
    match key.code {
        KeyCode::Esc | KeyCode::Char('h') | KeyCode::Left => ResolutionPickerAction::Back,
        KeyCode::Char('d') => ResolutionPickerAction::Default,
        KeyCode::Char('j') | KeyCode::Down if count > 0 => {
            *selected = (*selected + 1).min(count - 1);
            ResolutionPickerAction::None
        }
        KeyCode::Char('k') | KeyCode::Up => {
            *selected = selected.saturating_sub(1);
            ResolutionPickerAction::None
        }
        KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => ResolutionPickerAction::Select,
        _ => ResolutionPickerAction::None,
    }
}

impl ResolutionChoice {
    pub(crate) fn resolution(&self) -> Option<(u32, u32)> {
        match self {
            Self::Display(display) => Some((display.width, display.height)),
            Self::Preset(width, height) | Self::Configured(width, height) => {
                Some((*width, *height))
            }
        }
    }

    pub(crate) fn label(&self) -> String {
        self.resolution()
            .map(|(width, height)| format!("{width}x{height}"))
            .unwrap_or_default()
    }
}

impl JavaPicker {
    pub(crate) fn new() -> Self {
        Self::with_auto_path(crate::instance::java::detect_java_path())
    }

    pub(crate) fn with_auto_path(detected: String) -> Self {
        Self::with_cache(detected, None)
    }

    pub(crate) fn with_cache(detected: String, cache_path: Option<PathBuf>) -> Self {
        let cached = cache_path
            .as_deref()
            .and_then(crate::instance::java::load_installation_cache);
        Self {
            load: Arc::new(Mutex::new(
                cached.map_or(LoadState::Idle, LoadState::Loaded),
            )),
            current: None,
            detected,
            picker: SettingsPicker::default(),
            cache_path,
            refresh_started: false,
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    pub(crate) fn set_current(&mut self, current: Option<&str>) {
        self.current = current.map(str::to_owned);
        self.picker.reset();
    }

    pub(crate) fn open(&mut self, current: Option<&str>) {
        self.set_current(current);
        if self.refresh_started {
            return;
        }
        self.refresh_started = true;
        let mut load = self
            .load
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(*load, LoadState::Idle | LoadState::Error(_)) {
            *load = LoadState::Loading;
        }
        drop(load);
        let target = self.load.clone();
        let cache_path = self.cache_path.clone();
        let refresh_generation = self.generation.load(Ordering::Relaxed);
        let generation = self.generation.clone();
        let selected_paths = [Some(self.detected.clone()), self.current.clone()];
        let discover = move || {
            let mut installations = crate::instance::java::discover_installations();
            for path in selected_paths.into_iter().flatten() {
                if installations.iter().any(|installation| {
                    same_executable(&installation.path.to_string_lossy(), &path)
                }) {
                    continue;
                }
                if let Some(installation) =
                    crate::instance::java::inspect_installation(Path::new(&path))
                {
                    installations.push(installation);
                }
            }
            let mut load = target
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if generation.load(Ordering::Relaxed) != refresh_generation {
                return;
            }
            if let Some(cache_path) = cache_path
                && let Err(error) =
                    crate::instance::java::save_installation_cache(&cache_path, &installations)
            {
                tracing::debug!("Could not cache Java installations: {error}");
            }
            *load = LoadState::Loaded(installations);
            drop(load);
            crate::feedback::request_redraw();
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn_blocking(discover);
        } else {
            self.refresh_started = false;
            let mut load = self
                .load
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if matches!(*load, LoadState::Loading) {
                *load = LoadState::Idle;
            }
        }
    }

    pub(crate) fn choices(&self) -> Vec<JavaChoice> {
        let mut paths = Vec::new();
        if let LoadState::Loaded(installations) = &*self
            .load
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            paths.extend(
                installations
                    .iter()
                    .map(|installation| installation.path.to_string_lossy().into_owned()),
            );
        }
        if let Some(index) = paths
            .iter()
            .position(|path| same_executable(path, &self.detected))
        {
            paths[index].clone_from(&self.detected);
        } else {
            paths.insert(0, self.detected.clone());
        }
        if let Some(current) = &self.current {
            if let Some(index) = paths.iter().position(|path| same_executable(path, current)) {
                paths[index].clone_from(current);
            } else {
                paths.push(current.clone());
            }
        }
        paths.into_iter().map(JavaChoice::Installation).collect()
    }

    pub(crate) fn labels(&self) -> Vec<String> {
        self.picker.labels()
    }

    fn options(&self) -> Vec<SettingsPickerOption> {
        let installations = match &*self
            .load
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            LoadState::Loaded(installations) => Some(installations.clone()),
            _ => None,
        };
        self.choices()
            .into_iter()
            .map(|choice| match choice {
                JavaChoice::Installation(path) => {
                    let installation = installations.as_ref().and_then(|items| {
                        items
                            .iter()
                            .find(|item| same_executable(&item.path.to_string_lossy(), &path))
                    });
                    let title = installation
                        .and_then(|installation| installation.version.as_deref())
                        .map_or_else(|| "Java".to_owned(), java_title);
                    SettingsPickerOption {
                        key: path.clone(),
                        title,
                        detail: Some(path.clone()),
                        leading: None,
                        active: false,
                        badge: (self.current.is_none() && path == self.detected)
                            .then_some(SettingsPickerBadge::Auto),
                    }
                }
            })
            .collect()
    }

    pub(crate) fn take_status(&mut self) -> Option<Result<&'static str, String>> {
        let mut load = self
            .load
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &*load {
            LoadState::Idle => None,
            LoadState::Loading => Some(Ok("Detecting installed Java runtimes…")),
            LoadState::Loaded(installations) if installations.is_empty() => {
                Some(Ok("No additional Java runtimes found."))
            }
            LoadState::Loaded(_) => None,
            LoadState::Error(error) => {
                let error = error.clone();
                *load = LoadState::Loaded(Vec::new());
                Some(Err(error))
            }
        }
    }

    pub(crate) fn initialize(&mut self) {
        let preferred = self
            .current
            .clone()
            .unwrap_or_else(|| self.detected.clone());
        self.picker.sync(self.options(), Some(&preferred));
    }

    pub(crate) fn selected_choice(&self) -> JavaChoice {
        JavaChoice::Installation(
            self.picker
                .selected_key()
                .unwrap_or(&self.detected)
                .to_owned(),
        )
    }

    pub(crate) fn selection(&self) -> &SettingsPicker {
        &self.picker
    }

    pub(crate) fn selection_mut(&mut self) -> &mut SettingsPicker {
        &mut self.picker
    }

    pub(crate) fn detected_path(&self) -> &str {
        &self.detected
    }

    pub(crate) fn display_label(&self, path: &str) -> String {
        let version = match &*self
            .load
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            LoadState::Loaded(installations) => installations
                .iter()
                .find(|installation| same_executable(&installation.path.to_string_lossy(), path))
                .and_then(|installation| installation.version.clone()),
            _ => None,
        };
        java_runtime_label(path, version.as_deref())
    }

    pub(crate) fn automatic_change(&self, current: &str) -> bool {
        !same_executable(current, &self.detected)
    }

    pub(crate) fn invalidate_cache(&mut self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
        let mut load = self
            .load
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(cache_path) = &self.cache_path {
            let _ = std::fs::remove_file(cache_path);
        }
        *load = LoadState::Idle;
        self.picker.reset();
        self.refresh_started = false;
    }
}

impl Default for JavaPicker {
    fn default() -> Self {
        Self::new()
    }
}

impl GlfwPicker {
    pub(crate) fn new() -> Self {
        Self::with_bundled_version(None)
    }

    pub(crate) fn with_bundled_version(bundled_version: Option<String>) -> Self {
        Self {
            load: Arc::new(Mutex::new(LoadState::Idle)),
            current: None,
            bundled_version,
            picker: SettingsPicker::default(),
        }
    }

    pub(crate) fn open(&mut self, current: Option<&str>) {
        self.current = current.map(str::to_owned);
        self.picker.reset();
        let mut load = self
            .load
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !matches!(*load, LoadState::Idle | LoadState::Error(_)) {
            return;
        }
        *load = LoadState::Loading;
        drop(load);
        let target = self.load.clone();
        let discover = move || {
            let installations = discover_glfw_installations();
            *target
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                LoadState::Loaded(installations);
            crate::feedback::request_redraw();
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn_blocking(discover);
        } else {
            *self
                .load
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = LoadState::Idle;
        }
    }

    pub(crate) fn set_bundled_version(&mut self, version: Option<String>) {
        self.bundled_version = version;
    }

    pub(crate) fn choices(&self) -> Vec<GlfwChoice> {
        let mut choices = vec![GlfwChoice::Bundled];
        if let LoadState::Loaded(installations) = &*self
            .load
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            choices.extend(installations.iter().map(|installation| {
                GlfwChoice::System(installation.path.to_string_lossy().into_owned())
            }));
        }
        if let Some(current) = &self.current {
            if let Some(choice) = choices.iter_mut().find(|choice| {
                matches!(choice, GlfwChoice::System(path) if same_executable(path, current))
            }) {
                *choice = GlfwChoice::System(current.clone());
            } else {
                choices.push(GlfwChoice::System(current.clone()));
            }
        }
        choices
    }

    pub(crate) fn initialize(&mut self) {
        let preferred = self.current.as_deref().unwrap_or(BUNDLED_GLFW_KEY);
        self.picker.sync(self.options(), Some(preferred));
    }

    pub(crate) fn selected_choice(&self) -> GlfwChoice {
        match self.picker.selected_key() {
            Some(BUNDLED_GLFW_KEY) | None => GlfwChoice::Bundled,
            Some(path) => GlfwChoice::System(path.to_owned()),
        }
    }

    pub(crate) fn labels(&self) -> Vec<String> {
        self.picker.labels()
    }

    pub(crate) fn bundled_label(&self) -> String {
        self.bundled_version.as_ref().map_or_else(
            || "Minecraft GLFW".to_owned(),
            |version| format!("LWJGL GLFW {version}"),
        )
    }

    pub(crate) fn display_label(&self, path: Option<&str>) -> String {
        let Some(path) = path else {
            return self.bundled_label();
        };
        let detected_version = match &*self
            .load
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            LoadState::Loaded(installations) => installations
                .iter()
                .find(|installation| same_executable(&installation.path.to_string_lossy(), path))
                .and_then(|installation| installation.version.clone()),
            _ => None,
        };
        detected_version
            .or_else(|| glfw_version_from_path(Path::new(path)))
            .map_or_else(
                || format!("System GLFW  {path}"),
                |version| format!("GLFW {version}  {path}"),
            )
    }

    fn options(&self) -> Vec<SettingsPickerOption> {
        let installations = match &*self
            .load
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            LoadState::Loaded(installations) => installations.clone(),
            _ => Vec::new(),
        };
        self.choices()
            .into_iter()
            .map(|choice| match choice {
                GlfwChoice::Bundled => SettingsPickerOption {
                    key: BUNDLED_GLFW_KEY.to_owned(),
                    title: self.bundled_label(),
                    detail: None,
                    leading: None,
                    active: false,
                    badge: Some(SettingsPickerBadge::Bundled),
                },
                GlfwChoice::System(path) => {
                    let version = installations
                        .iter()
                        .find(|installation| {
                            same_executable(&installation.path.to_string_lossy(), &path)
                        })
                        .and_then(|installation| installation.version.as_deref());
                    SettingsPickerOption {
                        key: path.clone(),
                        title: version.map_or_else(
                            || "System GLFW".to_owned(),
                            |version| format!("GLFW {version}"),
                        ),
                        detail: Some(path),
                        leading: None,
                        active: false,
                        badge: None,
                    }
                }
            })
            .collect()
    }

    pub(crate) fn selection(&self) -> &SettingsPicker {
        &self.picker
    }

    pub(crate) fn selection_mut(&mut self) -> &mut SettingsPicker {
        &mut self.picker
    }

    pub(crate) fn take_status(&mut self) -> Option<&'static str> {
        match &*self
            .load
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            LoadState::Loading => Some("Detecting installed GLFW libraries…"),
            LoadState::Loaded(installations) if installations.is_empty() => {
                Some("No system GLFW libraries found; bundled remains available.")
            }
            _ => None,
        }
    }
}

const BUNDLED_GLFW_KEY: &str = "\0bundled-glfw";

impl Default for GlfwPicker {
    fn default() -> Self {
        Self::new()
    }
}

pub(crate) fn bundled_glfw_version(meta_dir: &Path, game_version: &str) -> Option<String> {
    let path = crate::storage::MetadataPaths::new(meta_dir)
        .versions()
        .join(game_version)
        .join("meta.json");
    let profile: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    profile
        .get("libraries")?
        .as_array()?
        .iter()
        .filter_map(|library| library.get("name")?.as_str())
        .find_map(|coordinate| {
            let mut parts = coordinate.split(':');
            match (parts.next(), parts.next(), parts.next()) {
                (Some("org.lwjgl"), Some("lwjgl-glfw"), Some(version)) => Some(version.to_owned()),
                _ => None,
            }
        })
}

fn discover_glfw_installations() -> Vec<GlfwInstallation> {
    let mut directories = Vec::<PathBuf>::new();
    for variable in ["LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "PATH"] {
        if let Some(value) = std::env::var_os(variable) {
            directories.extend(std::env::split_paths(&value));
        }
    }
    directories.extend(
        [
            "/usr/lib",
            "/usr/lib64",
            "/usr/local/lib",
            "/lib",
            "/lib64",
            "/opt/homebrew/lib",
            "/opt/local/lib",
        ]
        .into_iter()
        .map(PathBuf::from),
    );

    let mut nested = Vec::new();
    for directory in &directories {
        if let Ok(entries) = std::fs::read_dir(directory) {
            nested.extend(
                entries
                    .flatten()
                    .map(|entry| entry.path())
                    .filter(|path| path.is_dir()),
            );
        }
    }
    directories.extend(nested);

    let mut seen = BTreeSet::new();
    let mut installations = Vec::new();
    for directory in directories {
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            if !is_glfw_library(&path) {
                continue;
            }
            let canonical = std::fs::canonicalize(&path).unwrap_or(path);
            if seen.insert(canonical.clone()) {
                installations.push(GlfwInstallation {
                    version: glfw_version_from_path(&canonical),
                    path: canonical,
                });
            }
        }
    }
    installations.sort_by(|left, right| left.path.cmp(&right.path));
    installations
}

fn is_glfw_library(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    is_glfw_library_name(name)
}

fn is_glfw_library_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let versioned_so = name.strip_prefix("libglfw.so.").is_some_and(|version| {
        version.split('.').all(|part| {
            !part.is_empty() && part.chars().all(|character| character.is_ascii_digit())
        })
    });
    name == "libglfw.so"
        || versioned_so
        || name.starts_with("libglfw.") && name.ends_with(".dylib")
        || name.starts_with("glfw") && name.ends_with(".dll")
}

fn glfw_version_from_path(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    if let Some((_, suffix)) = name.rsplit_once(".so.") {
        return (!suffix.is_empty()).then(|| suffix.to_owned());
    }
    let stem = name
        .strip_prefix("libglfw.")
        .and_then(|name| name.strip_suffix(".dylib"))
        .or_else(|| {
            name.strip_prefix("glfw")
                .and_then(|name| name.strip_suffix(".dll"))
        })?;
    (!stem.is_empty()).then(|| stem.trim_start_matches(['-', '.']).to_owned())
}

fn same_executable(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    std::fs::canonicalize(left)
        .ok()
        .zip(std::fs::canonicalize(right).ok())
        .is_some_and(|(left, right)| left == right)
}

fn java_runtime_label(path: &str, version: Option<&str>) -> String {
    version.map_or_else(
        || format!("Java  {path}"),
        |version| format!("{}  {path}", java_title(version)),
    )
}

fn java_title(version: &str) -> String {
    let major = crate::instance::java::java_major(Some(version));
    if major == 0 {
        "Java".to_owned()
    } else {
        format!("Java {major}")
    }
}

pub(crate) fn adjust_memory(value: &str, forward: bool) -> String {
    let current = memory_kib(value).unwrap_or_default();
    let exact = MEMORY_STEPS.iter().position(|step| *step == value);
    let index = match (exact, forward) {
        (Some(index), true) => (index + 1).min(MEMORY_STEPS.len() - 1),
        (Some(index), false) => index.saturating_sub(1),
        (None, true) => MEMORY_STEPS
            .iter()
            .position(|step| memory_kib(step).is_some_and(|amount| amount > current))
            .unwrap_or(MEMORY_STEPS.len() - 1),
        (None, false) => MEMORY_STEPS
            .iter()
            .rposition(|step| memory_kib(step).is_some_and(|amount| amount < current))
            .unwrap_or(0),
    };
    MEMORY_STEPS[index].to_owned()
}

pub(crate) fn render_memory_gauge(
    frame: &mut Frame,
    area: Rect,
    value: &str,
    label: String,
    selected: bool,
) {
    let theme = THEME.as_ref();
    let current = memory_kib(value).unwrap_or_default();
    let index = MEMORY_STEPS
        .iter()
        .position(|step| memory_kib(step).is_some_and(|amount| current <= amount))
        .unwrap_or(MEMORY_STEPS.len() - 1);
    let ratio = (index + 1) as f64 / MEMORY_STEPS.len() as f64;
    let value_width = 17.min(area.width);
    let value_area = Rect {
        width: value_width,
        ..area
    };
    let line_area = Rect {
        x: area.x.saturating_add(value_width).saturating_add(1),
        width: area.width.saturating_sub(value_width.saturating_add(1)),
        ..area
    };
    let value_style = if selected {
        Style::default()
            .fg(theme.accent())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text())
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(label, value_style))),
        value_area,
    );

    let gauge = LineGauge::default()
        .ratio(ratio)
        .label("")
        .filled_symbol("━")
        .unfilled_symbol("─")
        .filled_style(Style::default().fg(if selected {
            theme.accent()
        } else {
            theme.text_dim()
        }))
        .unfilled_style(Style::default().fg(theme.border()));
    frame.render_widget(gauge, line_area);
    if line_area.width > 0 {
        let thumb_offset = ((line_area.width.saturating_sub(1)) as f64 * ratio).round() as u16;
        frame.render_widget(
            Paragraph::new(Span::styled(
                if thumb_offset + 1 < line_area.width {
                    "◆ "
                } else {
                    "◆"
                },
                Style::default().fg(if selected {
                    theme.accent()
                } else {
                    theme.text_dim()
                }),
            )),
            Rect {
                x: line_area.x.saturating_add(thumb_offset),
                width: 2.min(line_area.width.saturating_sub(thumb_offset)),
                ..line_area
            },
        );
    }
}

pub(crate) fn handle_text_area_input(input: &mut TextArea<'_>, key: &KeyEvent) {
    if key.code == KeyCode::Backspace && key.modifiers.contains(KeyModifiers::CONTROL) {
        input.delete_word();
    } else {
        input.input(*key);
    }
}

pub(crate) fn settings_text_area(lines: Vec<String>) -> TextArea<'static> {
    let theme = THEME.as_ref();
    let lines = lines
        .into_iter()
        .flat_map(|line| line.split('\n').map(str::to_owned).collect::<Vec<String>>())
        .collect::<Vec<_>>();
    let mut editor = TextArea::new(if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    });
    editor.set_style(Style::default().fg(theme.text()).bg(theme.surface()));
    editor.set_cursor_line_style(Style::default());
    editor.set_cursor_style(Style::default().fg(theme.background()).bg(theme.accent()));
    editor.move_cursor(CursorMove::Bottom);
    editor.move_cursor(CursorMove::End);
    editor
}

pub(crate) fn parse_tag_values(input: &str) -> Result<Vec<String>, String> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Quote {
        None,
        Single,
        Double,
    }

    let mut values = Vec::new();
    let mut current = String::new();
    let mut quote = Quote::None;
    let mut started = false;
    let mut characters = input.chars().peekable();
    while let Some(character) = characters.next() {
        match (quote, character) {
            (Quote::None, character) if character.is_whitespace() => {
                if started {
                    values.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (Quote::None, '\'') => {
                quote = Quote::Single;
                started = true;
            }
            (Quote::None, '"') => {
                quote = Quote::Double;
                started = true;
            }
            (Quote::Single, '\'') => quote = Quote::None,
            (Quote::Double, '"') => quote = Quote::None,
            (Quote::Double, '\\') if matches!(characters.peek(), Some('"' | '\\')) => {
                current.push(characters.next().unwrap_or_default());
                started = true;
            }
            (_, character) => {
                current.push(character);
                started = true;
            }
        }
    }
    if quote != Quote::None {
        return Err("Quoted value is missing its closing quote.".to_owned());
    }
    if started {
        values.push(current);
    }
    let mut seen = BTreeSet::new();
    values.retain(|value| seen.insert(value.clone()));
    Ok(values)
}

pub(crate) fn format_tag_values(values: &[String]) -> String {
    values
        .iter()
        .map(|value| {
            if !value.is_empty()
                && !value
                    .chars()
                    .any(|character| character.is_whitespace() || matches!(character, '\'' | '"'))
            {
                value.clone()
            } else {
                format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn parse_environment(value: &str) -> Result<BTreeMap<String, String>, String> {
    let mut environment = BTreeMap::new();
    for assignment in parse_tag_values(value)? {
        let Some((key, value)) = assignment.split_once('=') else {
            return Err(format!(
                "Environment variable '{assignment}' must use KEY=value."
            ));
        };
        if key.is_empty() || key.contains('\0') || value.contains('\0') {
            return Err("Environment variable names cannot be empty.".to_owned());
        }
        if environment
            .insert(key.to_owned(), value.to_owned())
            .is_some()
        {
            return Err(format!("Environment variable '{key}' is repeated."));
        }
    }
    Ok(environment)
}

pub(crate) fn environment_labels(environment: &BTreeMap<String, String>) -> Vec<String> {
    environment
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect()
}

pub(crate) fn tagged_value_lines(
    mut prefix: Vec<Span<'static>>,
    selected: bool,
    editing: bool,
    values: &[String],
    empty: &str,
    width: u16,
) -> Vec<Line<'static>> {
    let theme = THEME.as_ref();
    if selected && editing {
        return vec![Line::from(prefix)];
    }
    if values.is_empty() {
        prefix.push(Span::styled(
            empty.to_owned(),
            Style::default().fg(theme.text_dim()),
        ));
        return vec![Line::from(prefix)];
    }

    let available = width.saturating_sub(20) as usize;
    let mut lines = Vec::new();
    let mut spans = prefix;
    let mut used = 0usize;
    for value in values {
        let badge_width = value.chars().count() + 2;
        let separator = usize::from(used > 0);
        if used > 0 && used + separator + badge_width > available {
            lines.push(Line::from(spans));
            spans = vec![Span::raw(" ".repeat(20))];
            used = 0;
        }
        if used > 0 {
            spans.push(Span::raw(" "));
            used += 1;
        }
        spans.push(Span::styled(
            format!(" {value} "),
            Style::default()
                .fg(if selected {
                    theme.accent()
                } else {
                    theme.text()
                })
                .bg(theme.background())
                .add_modifier(Modifier::BOLD),
        ));
        used += badge_width;
    }
    lines.push(Line::from(spans));
    lines
}

pub(crate) fn tagged_row_count(values: &[String], width: u16) -> usize {
    if values.is_empty() {
        return 1;
    }
    let available = width.saturating_sub(20) as usize;
    let mut rows = 1;
    let mut used = 0usize;
    for value in values {
        let badge_width = value.chars().count() + 2;
        let separator = usize::from(used > 0);
        if used > 0 && used + separator + badge_width > available {
            rows += 1;
            used = 0;
        }
        used += usize::from(used > 0) + badge_width;
    }
    rows
}

pub(crate) fn auto_label() -> Span<'static> {
    status_badge("Auto", THEME.as_ref().success())
}

pub(crate) fn bundled_label() -> Span<'static> {
    status_badge("Bundled", THEME.as_ref().success())
}

pub(crate) fn default_label() -> Span<'static> {
    status_badge("Default", THEME.as_ref().warning())
}

pub(crate) fn render_settings_picker(
    picker: &SettingsPicker,
    area: Rect,
    buffer: &mut ratatui::buffer::Buffer,
) {
    super::select_list::render_styled(picker.items(), picker.selected(), area, buffer);
}

pub(crate) fn display_resolutions() -> Vec<DisplayResolution> {
    let Ok(displays) = display_info::DisplayInfo::all() else {
        return Vec::new();
    };
    let mut resolutions = displays
        .into_iter()
        .filter(|display| display.width > 0 && display.height > 0)
        .map(|display| {
            let name = if display.name.is_empty() {
                display.friendly_name
            } else {
                display.name
            };
            DisplayResolution {
                width: display.width,
                height: display.height,
                name,
                primary: display.is_primary,
            }
        })
        .collect::<Vec<_>>();
    resolutions.sort_by_key(|resolution| !resolution.primary);
    resolutions
}

pub(crate) fn default_resolution() -> (u32, u32) {
    DEFAULT_RESOLUTION
}

pub(crate) fn is_default_resolution(resolution: Option<(u32, u32)>) -> bool {
    resolution == Some(default_resolution())
}

pub(crate) fn resolution_choices(
    current: Option<(u32, u32)>,
    displays: &[DisplayResolution],
) -> Vec<ResolutionChoice> {
    let mut choices = Vec::new();
    choices.extend(displays.iter().cloned().map(ResolutionChoice::Display));
    for (width, height) in [
        DEFAULT_RESOLUTION,
        (1280, 720),
        (1600, 900),
        (1920, 1080),
        (2560, 1440),
        (3840, 2160),
    ] {
        if !choices
            .iter()
            .any(|choice| choice.resolution() == Some((width, height)))
        {
            choices.push(ResolutionChoice::Preset(width, height));
        }
    }
    if let Some((width, height)) = current
        && !choices
            .iter()
            .any(|choice| choice.resolution() == Some((width, height)))
    {
        choices.push(ResolutionChoice::Configured(width, height));
    }
    choices
}

pub(crate) fn resolution_items(
    choices: &[ResolutionChoice],
    selected: usize,
) -> Vec<ListItem<'static>> {
    let theme = THEME.as_ref();
    choices
        .iter()
        .enumerate()
        .map(|(index, choice)| {
            let mut spans = vec![Span::styled(
                choice.label(),
                Style::default().fg(if index == selected {
                    theme.accent()
                } else {
                    theme.text()
                }),
            )];
            if is_default_resolution(choice.resolution()) {
                spans.extend([Span::raw("  "), default_label()]);
            }
            match choice {
                ResolutionChoice::Preset(_, _) => {}
                ResolutionChoice::Display(display) => {
                    if !display.name.is_empty() {
                        spans.extend([
                            Span::raw("  "),
                            Span::styled(
                                format!(" {} ", display.name),
                                Style::default()
                                    .fg(if display.primary {
                                        theme.success()
                                    } else {
                                        theme.info()
                                    })
                                    .bg(theme.stripe()),
                            ),
                        ]);
                    }
                }
                ResolutionChoice::Configured(_, _) => {
                    spans.push(Span::styled(
                        "  configured",
                        Style::default().fg(theme.text_dim()),
                    ));
                }
            }
            ListItem::new(Line::from(spans))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_steps_move_and_clamp() {
        assert_eq!(adjust_memory("2G", true), "3G");
        assert_eq!(adjust_memory("2G", false), "1G");
        assert_eq!(adjust_memory("64G", true), "64G");
        assert_eq!(adjust_memory("512M", false), "512M");
    }

    #[test]
    fn minecraft_window_size_is_the_default_resolution() {
        assert_eq!(default_resolution(), (854, 480));
        assert!(is_default_resolution(Some((854, 480))));
        assert!(!is_default_resolution(Some((1440, 2560))));
        assert_eq!(default_label().style.bg, Some(THEME.as_ref().warning()));

        let choices = resolution_choices(Some((854, 480)), &[]);
        let backend = ratatui::backend::TestBackend::new(40, 6);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    ratatui::widgets::List::new(resolution_items(&choices, 0)),
                    frame.area(),
                );
            })
            .unwrap();
        assert!(terminal.backend().to_string().contains("Default"));
    }

    #[test]
    fn java_picker_preserves_semantic_selection_when_results_arrive() {
        let mut picker = JavaPicker::new();
        picker.current = Some("/opt/jdk/bin/java".to_owned());
        picker.initialize();
        *picker.load.lock().unwrap() = LoadState::Loaded(vec![JavaInstallation {
            path: "/opt/jdk/bin/java".into(),
            version: Some("21".to_owned()),
        }]);

        picker.initialize();

        assert_eq!(
            picker.selected_choice(),
            JavaChoice::Installation("/opt/jdk/bin/java".to_owned())
        );
        assert_eq!(
            picker.display_label("/opt/jdk/bin/java"),
            "Java 21  /opt/jdk/bin/java"
        );
    }

    #[test]
    fn setting_java_value_does_not_start_discovery() {
        let mut picker = JavaPicker::with_auto_path("/auto/java".to_owned());

        picker.set_current(Some("/custom/java"));

        assert!(!picker.refresh_started);
        assert!(matches!(
            *picker
                .load
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            LoadState::Idle
        ));

        picker.open(Some("/custom/java"));
        assert!(!picker.refresh_started);
        assert!(matches!(
            *picker
                .load
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            LoadState::Idle
        ));
    }

    #[test]
    fn invalidating_java_picker_drops_memory_and_disk_cache() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache/java/installations.json");
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        std::fs::write(&cache, "[]").unwrap();
        let mut picker = JavaPicker::with_cache("/auto/java".to_owned(), Some(cache.clone()));
        *picker.load.lock().unwrap() = LoadState::Loaded(vec![JavaInstallation {
            path: "/cached/java".into(),
            version: Some("21".to_owned()),
        }]);
        picker.refresh_started = true;

        picker.invalidate_cache();

        assert!(!cache.exists());
        assert!(!picker.refresh_started);
        assert!(matches!(*picker.load.lock().unwrap(), LoadState::Idle));
    }

    #[test]
    fn java_title_is_minimal_and_uses_the_major_version() {
        let picker = JavaPicker::with_auto_path("/opt/jdk-25/bin/java".to_owned());
        assert_eq!(
            picker.display_label("/opt/jdk-25/bin/java"),
            "Java  /opt/jdk-25/bin/java"
        );
        *picker.load.lock().unwrap() = LoadState::Loaded(vec![JavaInstallation {
            path: "/opt/jdk-25/bin/java".into(),
            version: Some("25.0.1".to_owned()),
        }]);
        assert_eq!(
            picker.display_label("/opt/jdk-25/bin/java"),
            "Java 25  /opt/jdk-25/bin/java"
        );
        assert_eq!(java_title("1.8.0_412"), "Java 8");
        assert_eq!(java_title("21-ea"), "Java 21");
    }

    #[test]
    fn automatic_java_compares_selected_executables() {
        let picker = JavaPicker::with_auto_path("/auto/java".to_owned());
        assert!(!picker.automatic_change("/auto/java"));
        assert!(picker.automatic_change("/current/java"));
        assert!(picker.automatic_change("/other/java"));
    }

    #[test]
    fn settings_picker_shares_navigation_and_preserves_selection() {
        let option = |key: &str| SettingsPickerOption {
            key: key.to_owned(),
            title: key.to_owned(),
            detail: None,
            leading: None,
            active: false,
            badge: None,
        };
        let mut picker = SettingsPicker::default();
        picker.sync(vec![option("one"), option("two")], Some("one"));
        assert_eq!(
            picker.handle_key(&KeyEvent::from(KeyCode::Char('j'))),
            SettingsPickerAction::None
        );
        assert_eq!(picker.selected_key(), Some("two"));

        picker.sync(
            vec![option("zero"), option("one"), option("two")],
            Some("one"),
        );
        assert_eq!(picker.selected_key(), Some("two"));
        assert_eq!(
            picker.handle_key(&KeyEvent::from(KeyCode::Enter)),
            SettingsPickerAction::Select
        );
        assert_eq!(
            picker.handle_key(&KeyEvent::from(KeyCode::Right)),
            SettingsPickerAction::Select
        );
    }

    #[test]
    fn right_arrow_selects_resolution_instead_of_moving_down() {
        let mut selected = 0;
        assert_eq!(
            handle_resolution_picker_key(&mut selected, 3, &KeyEvent::from(KeyCode::Right)),
            ResolutionPickerAction::Select
        );
        assert_eq!(selected, 0);
    }

    #[test]
    fn bundled_glfw_version_comes_from_minecraft_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let versions = crate::storage::MetadataPaths::new(temp.path()).versions();
        std::fs::create_dir_all(versions.join("1.21.1")).unwrap();
        std::fs::write(
            versions.join("1.21.1/meta.json"),
            r#"{"libraries":[{"name":"org.lwjgl:lwjgl-glfw:3.3.3"}]}"#,
        )
        .unwrap();

        assert_eq!(
            bundled_glfw_version(temp.path(), "1.21.1").as_deref(),
            Some("3.3.3")
        );
        let mut picker = GlfwPicker::with_bundled_version(Some("3.3.3".to_owned()));
        picker.initialize();
        assert_eq!(picker.labels()[0], "LWJGL GLFW 3.3.3");
        assert_eq!(
            picker.selection().options[0].badge,
            Some(SettingsPickerBadge::Bundled)
        );
    }

    #[test]
    fn glfw_library_names_are_recognized() {
        assert!(is_glfw_library_name("libglfw.so.3"));
        assert!(is_glfw_library_name("libglfw.3.dylib"));
        assert!(is_glfw_library_name("glfw3.dll"));
        assert!(!is_glfw_library_name("libglfw.so.backup"));
        assert!(!is_glfw_library_name("libGL.so"));
        assert_eq!(
            glfw_version_from_path(Path::new("/usr/lib/libglfw.so.3.4")).as_deref(),
            Some("3.4")
        );
    }

    #[test]
    fn settings_text_input_deletes_the_previous_word() {
        let mut input = TextArea::from(["one two"]);
        input.move_cursor(ratatui_textarea::CursorMove::End);

        handle_text_area_input(
            &mut input,
            &KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL),
        );

        assert_eq!(input.lines(), ["one "]);
    }

    #[test]
    fn tag_values_with_spaces_and_quotes_round_trip() {
        let values = vec![
            "-Xmx2G".to_owned(),
            "-Dlabel=hello world".to_owned(),
            r"C:\Program Files\Java".to_owned(),
            "-Dquote=\"value\"".to_owned(),
            String::new(),
        ];

        let formatted = format_tag_values(&values);

        assert_eq!(parse_tag_values(&formatted).unwrap(), values);
        assert!(parse_tag_values("\"unterminated").is_err());
    }

    #[test]
    fn tag_values_drop_exact_duplicates_without_reordering() {
        assert_eq!(
            parse_tag_values("-Xmx2G '-Dlabel=hello world' -Xmx2G").unwrap(),
            ["-Xmx2G", "-Dlabel=hello world"]
        );
    }

    #[test]
    fn environment_values_can_contain_spaces() {
        let environment =
            parse_environment(r#"LABEL="hello world" PATH="C:\Program Files""#).unwrap();

        assert_eq!(
            environment.get("LABEL").map(String::as_str),
            Some("hello world")
        );
        assert_eq!(
            environment.get("PATH").map(String::as_str),
            Some(r"C:\Program Files")
        );
        assert!(parse_environment("=missing").is_err());
        assert!(parse_environment("KEY=one KEY=two").is_err());
    }

    #[test]
    fn memory_thumb_clears_the_first_unfilled_cell() {
        let backend = ratatui::backend::TestBackend::new(40, 1);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_memory_gauge(frame, frame.area(), "8G", "8G".to_owned(), true))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let thumb = (0..40)
            .find(|x| buffer[(*x, 0)].symbol() == "◆")
            .expect("slider thumb");

        assert_eq!(buffer[(thumb + 1, 0)].symbol(), " ");
    }

    #[test]
    fn memory_value_uses_the_existing_row_background() {
        let backend = ratatui::backend::TestBackend::new(40, 1);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        let surface = THEME.as_ref().surface();
        terminal
            .draw(|frame| {
                frame.render_widget(
                    ratatui::widgets::Block::default().style(Style::default().bg(surface)),
                    frame.area(),
                );
                render_memory_gauge(frame, frame.area(), "6G", "6G".to_owned(), false);
            })
            .unwrap();

        assert_eq!(terminal.backend().buffer()[(1, 0)].bg, surface);
    }
}
