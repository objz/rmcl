// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// all the config structs that map to sections in config.toml.
// everything has sane defaults so a blank file (or no file) still works.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

use crate::instance::models::{WindowMode, memory_kib, normalize_memory_value};

pub const DEFAULT_RESOLUTION: (u32, u32) = (854, 480);

#[derive(Debug, Serialize, Deserialize, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImageProtocol {
    #[default]
    Auto,
    Halfblocks,
    Quadrants,
    Kitty,
    Iterm2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShortcutHintScope {
    All,
    Main,
    Instances,
    Content,
    Accounts,
    Settings,
    Popups,
}

impl ShortcutHintScope {
    pub const AREAS: [Self; 5] = [
        Self::Instances,
        Self::Content,
        Self::Accounts,
        Self::Settings,
        Self::Popups,
    ];
}

impl fmt::Display for ImageProtocol {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Auto => formatter.write_str("auto"),
            Self::Halfblocks => formatter.write_str("halfblocks"),
            Self::Quadrants => formatter.write_str("quadrants"),
            Self::Kitty => formatter.write_str("kitty"),
            Self::Iterm2 => formatter.write_str("iterm2"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct General {
    #[serde(default = "default_true")]
    pub check_modpack_updates: bool,
    #[serde(default = "default_true")]
    pub check_content_updates: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            check_modpack_updates: true,
            check_content_updates: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentProvider {
    #[default]
    Modrinth,
    CurseForge,
}

impl fmt::Display for ContentProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Modrinth => formatter.write_str("Modrinth"),
            Self::CurseForge => formatter.write_str("CurseForge"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Content {
    #[serde(default = "default_true")]
    pub ask_on_provider_conflict: bool,
    #[serde(default, deserialize_with = "deserialize_content_provider")]
    pub preferred_provider: ContentProvider,
    #[serde(default)]
    pub preferred_provider_only: bool,
    #[serde(default = "default_unmatched_retry_hours")]
    pub unmatched_retry_hours: u64,
    #[serde(default = "default_max_fingerprint_size_mib")]
    pub max_fingerprint_size_mib: u64,
}

fn default_true() -> bool {
    true
}

fn deserialize_content_provider<'de, D>(deserializer: D) -> Result<ContentProvider, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    Ok(if value.eq_ignore_ascii_case("curseforge") {
        ContentProvider::CurseForge
    } else {
        ContentProvider::Modrinth
    })
}

fn default_unmatched_retry_hours() -> u64 {
    24
}

fn default_max_fingerprint_size_mib() -> u64 {
    512
}

impl Default for Content {
    fn default() -> Self {
        Self {
            ask_on_provider_conflict: true,
            preferred_provider: ContentProvider::default(),
            preferred_provider_only: false,
            unmatched_retry_hours: default_unmatched_retry_hours(),
            max_fingerprint_size_mib: default_max_fingerprint_size_mib(),
        }
    }
}

impl Content {
    pub fn preferred_provider(&self) -> &str {
        self.preferred_provider_with_curseforge(crate::net::curseforge::api_key().is_some())
    }

    pub fn discovery_provider_enabled(&self, provider: &str) -> bool {
        self.discovery_provider_enabled_with_curseforge(
            provider,
            crate::net::curseforge::api_key().is_some(),
        )
    }

    pub fn discovery_provider_label(&self) -> &'static str {
        self.discovery_provider_label_with_curseforge(crate::net::curseforge::api_key().is_some())
    }

    fn preferred_provider_with_curseforge(&self, curseforge_available: bool) -> &'static str {
        if self.preferred_provider == ContentProvider::CurseForge && curseforge_available {
            "curseforge"
        } else {
            "modrinth"
        }
    }

    fn discovery_provider_enabled_with_curseforge(
        &self,
        provider: &str,
        curseforge_available: bool,
    ) -> bool {
        match provider {
            "modrinth" => {
                !self.preferred_provider_only
                    || self.preferred_provider_with_curseforge(curseforge_available) == "modrinth"
            }
            "curseforge" => {
                curseforge_available
                    && (!self.preferred_provider_only
                        || self.preferred_provider_with_curseforge(curseforge_available)
                            == "curseforge")
            }
            _ => false,
        }
    }

    fn discovery_provider_label_with_curseforge(&self, curseforge_available: bool) -> &'static str {
        match (
            self.discovery_provider_enabled_with_curseforge("modrinth", curseforge_available),
            self.discovery_provider_enabled_with_curseforge("curseforge", curseforge_available),
        ) {
            (true, true) => "providers",
            (false, true) => "CurseForge",
            _ => "Modrinth",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Paths {
    #[serde(default = "default_instances_dir")]
    #[serde(skip_serializing_if = "is_default_instances_dir")]
    pub instances_dir: String,
    #[serde(default = "default_meta_dir")]
    #[serde(skip_serializing_if = "is_default_meta_dir")]
    pub meta_dir: String,
    #[serde(default)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub java_path: Option<String>,
}

fn default_instances_dir() -> String {
    dirs_next::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rmcl")
        .join("instances")
        .to_string_lossy()
        .into_owned()
}

fn is_default_instances_dir(path: &str) -> bool {
    path == default_instances_dir()
}

fn default_meta_dir() -> String {
    dirs_next::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rmcl")
        .join("meta")
        .to_string_lossy()
        .into_owned()
}

fn is_default_meta_dir(path: &str) -> bool {
    path == default_meta_dir()
}

impl Default for Paths {
    fn default() -> Self {
        Self {
            instances_dir: default_instances_dir(),
            meta_dir: default_meta_dir(),
            java_path: None,
        }
    }
}

// expand ~ in paths since toml doesn't do that for us
pub fn resolve_path(raw: &str) -> PathBuf {
    if let Some(stripped) = raw.strip_prefix("~/") {
        if let Some(home) = dirs_next::home_dir() {
            return home.join(stripped);
        }
    } else if raw == "~"
        && let Some(home) = dirs_next::home_dir()
    {
        return home;
    }
    PathBuf::from(raw)
}

impl Paths {
    pub fn effective_java_path(&self) -> Option<&str> {
        self.java_path.as_deref().filter(|s| !s.is_empty())
    }

    pub fn resolve_instances_dir(&self) -> PathBuf {
        resolve_path(&self.instances_dir)
    }

    pub fn resolve_meta_dir(&self) -> PathBuf {
        resolve_path(&self.meta_dir)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Defaults {
    #[serde(default = "default_memory_min")]
    pub memory_min: String,
    #[serde(default = "default_memory_max")]
    pub memory_max: String,
    #[serde(default)]
    pub jvm_args: Vec<String>,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
    #[serde(default)]
    pub window_mode: WindowMode,
    #[serde(default = "default_resolution")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<(u32, u32)>,
}

fn default_memory_min() -> String {
    "512M".to_owned()
}
fn default_memory_max() -> String {
    "2G".to_owned()
}

fn default_resolution() -> Option<(u32, u32)> {
    Some(DEFAULT_RESOLUTION)
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            memory_min: default_memory_min(),
            memory_max: default_memory_max(),
            jvm_args: Vec::new(),
            environment: BTreeMap::new(),
            window_mode: WindowMode::default(),
            resolution: default_resolution(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
// timing knobs for the error toast animation: show for 5s, start sliding at 3.5s,
// fly off screen over 300ms. tweak these if the toasts feel too fast or slow.
pub struct Ui {
    #[serde(default)]
    pub image_protocol: ImageProtocol,
    #[serde(default)]
    pub hidden_shortcut_hints: BTreeSet<ShortcutHintScope>,
    #[serde(default = "default_error_auto_dismiss_ms")]
    pub error_auto_dismiss_ms: u64,
    #[serde(default = "default_error_slide_start_ms")]
    pub error_slide_start_ms: u64,
    #[serde(default = "default_error_fly_out_ms")]
    pub error_fly_out_ms: u64,
    #[serde(default = "default_max_error_events")]
    pub max_error_events: usize,
}

fn default_error_auto_dismiss_ms() -> u64 {
    5000
}
fn default_error_slide_start_ms() -> u64 {
    3500
}
fn default_error_fly_out_ms() -> u64 {
    300
}
fn default_max_error_events() -> usize {
    50
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            image_protocol: ImageProtocol::default(),
            hidden_shortcut_hints: BTreeSet::new(),
            error_auto_dismiss_ms: default_error_auto_dismiss_ms(),
            error_slide_start_ms: default_error_slide_start_ms(),
            error_fly_out_ms: default_error_fly_out_ms(),
            max_error_events: default_max_error_events(),
        }
    }
}

impl Ui {
    pub fn show_shortcut_hints(&self, scope: ShortcutHintScope) -> bool {
        !self.hidden_shortcut_hints.contains(&ShortcutHintScope::All)
            && !(scope != ShortcutHintScope::Popups
                && self
                    .hidden_shortcut_hints
                    .contains(&ShortcutHintScope::Main))
            && !self.hidden_shortcut_hints.contains(&scope)
    }

    pub fn set_shortcut_hints(&mut self, scope: ShortcutHintScope, visible: bool) {
        let mut hidden = ShortcutHintScope::AREAS
            .into_iter()
            .filter(|area| !self.show_shortcut_hints(*area))
            .collect::<BTreeSet<_>>();
        if visible {
            hidden.remove(&scope);
        } else {
            hidden.insert(scope);
        }
        self.hidden_shortcut_hints = if hidden.len() == ShortcutHintScope::AREAS.len() {
            [ShortcutHintScope::All].into_iter().collect()
        } else if ShortcutHintScope::AREAS[..4]
            .iter()
            .all(|area| hidden.contains(area))
        {
            [ShortcutHintScope::Main].into_iter().collect()
        } else {
            hidden
        };
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub general: General,
    #[serde(default)]
    pub paths: Paths,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub ui: Ui,
    #[serde(default)]
    pub content: Content,
}

impl Config {
    pub fn normalize(mut self) -> Self {
        if self.paths.instances_dir.trim().is_empty() {
            self.paths.instances_dir = default_instances_dir();
        }
        if self.paths.meta_dir.trim().is_empty() {
            self.paths.meta_dir = default_meta_dir();
        }
        self.paths.java_path = self
            .paths
            .java_path
            .take()
            .and_then(|path| (!path.trim().is_empty()).then(|| path.trim().to_owned()));
        self.defaults.memory_min =
            normalize_memory_value(&self.defaults.memory_min).unwrap_or_else(default_memory_min);
        self.defaults.memory_max =
            normalize_memory_value(&self.defaults.memory_max).unwrap_or_else(default_memory_max);
        if memory_kib(&self.defaults.memory_min) > memory_kib(&self.defaults.memory_max) {
            self.defaults
                .memory_max
                .clone_from(&self.defaults.memory_min);
        }
        if self
            .defaults
            .resolution
            .is_none_or(|(width, height)| width == 0 || height == 0)
        {
            self.defaults.resolution = default_resolution();
        }
        self.ui.error_auto_dismiss_ms = self.ui.error_auto_dismiss_ms.max(1);
        self.ui.error_slide_start_ms = self
            .ui
            .error_slide_start_ms
            .min(self.ui.error_auto_dismiss_ms);
        self.ui.error_fly_out_ms = self.ui.error_fly_out_ms.min(self.ui.error_auto_dismiss_ms);
        self.ui.max_error_events = self.ui.max_error_events.max(1);
        self
    }
}

#[cfg(test)]
#[path = "tests/settings.rs"]
mod tests;
