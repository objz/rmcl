// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// theme resolution: loads theme.toml, picks a base theme (builtin or custom file),
// then layers user color overrides on top. supports loading .toml themes from the
// config/theme/ directory or by absolute path.

use std::path::Path;
use std::sync::{Arc, LazyLock, RwLock};

use ratatui::style::Color;
use ratatui::widgets::BorderType;
use ratatui_themekit::{CustomTheme, Theme, resolve_theme};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum BorderStyle {
    #[default]
    Rounded,
    Plain,
    Double,
    Thick,
}

impl BorderStyle {
    pub fn to_border_type(&self) -> BorderType {
        match self {
            Self::Rounded => BorderType::Rounded,
            Self::Plain => BorderType::Plain,
            Self::Double => BorderType::Double,
            Self::Thick => BorderType::Thick,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ThemeOverrides {
    pub accent: Option<Color>,
    pub accent_dim: Option<Color>,
    pub text: Option<Color>,
    pub text_dim: Option<Color>,
    pub text_bright: Option<Color>,
    pub success: Option<Color>,
    pub error: Option<Color>,
    pub warning: Option<Color>,
    pub info: Option<Color>,
    pub diff_added: Option<Color>,
    pub diff_removed: Option<Color>,
    pub diff_context: Option<Color>,
    pub border: Option<Color>,
    pub surface: Option<Color>,
    pub background: Option<Color>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeConfig {
    #[serde(default)]
    pub border_style: BorderStyle,
    #[serde(default = "default_theme_name")]
    pub theme: String,
    #[serde(default)]
    pub custom: Option<ThemeOverrides>,
}

fn default_theme_name() -> String {
    "catppuccin".to_owned()
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            border_style: BorderStyle::default(),
            theme: default_theme_name(),
            custom: None,
        }
    }
}

fn load_theme_config() -> ThemeConfig {
    let path = super::get_config_path().join("theme.toml");
    ensure_theme_exists(&path);
    match std::fs::read_to_string(&path) {
        Ok(content) => toml::from_str(&content).unwrap_or_else(|e| {
            tracing::warn!("Failed to parse theme.toml: {}. Using defaults.", e);
            ThemeConfig::default()
        }),
        Err(_) => ThemeConfig::default(),
    }
}

fn ensure_theme_exists(path: &Path) {
    if path.exists() {
        return;
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, include_str!("../../assets/theme.toml"));
}

// start from a base theme, then override individual colors if the user specified any
fn resolve_app_theme(config: &ThemeConfig) -> Box<dyn Theme> {
    let base = load_base_theme(&config.theme);

    let Some(overrides) = &config.custom else {
        return base;
    };

    Box::new(CustomTheme {
        name: format!("{} (customized)", base.name()),
        id: base.id().to_owned(),
        accent: overrides.accent.unwrap_or_else(|| base.accent()),
        accent_dim: overrides.accent_dim.unwrap_or_else(|| base.accent_dim()),
        text: overrides.text.unwrap_or_else(|| base.text()),
        text_dim: overrides.text_dim.unwrap_or_else(|| base.text_dim()),
        text_bright: overrides.text_bright.unwrap_or_else(|| base.text_bright()),
        success: overrides.success.unwrap_or_else(|| base.success()),
        error: overrides.error.unwrap_or_else(|| base.error()),
        warning: overrides.warning.unwrap_or_else(|| base.warning()),
        info: overrides.info.unwrap_or_else(|| base.info()),
        diff_added: overrides.diff_added.unwrap_or_else(|| base.diff_added()),
        diff_removed: overrides
            .diff_removed
            .unwrap_or_else(|| base.diff_removed()),
        diff_context: overrides
            .diff_context
            .unwrap_or_else(|| base.diff_context()),
        border: overrides.border.unwrap_or_else(|| base.border()),
        surface: overrides.surface.unwrap_or_else(|| base.surface()),
        background: overrides.background.unwrap_or_else(|| base.background()),
    })
}

// tries to find the theme: absolute path > config/theme/<name> > config/theme/<name>.toml > builtin
fn load_base_theme(name: &str) -> Box<dyn Theme> {
    let path = if Path::new(name).is_absolute() {
        Some(std::path::PathBuf::from(name))
    } else {
        let theme_dir = super::get_config_path().join("theme");
        let candidate = theme_dir.join(name);
        if candidate.exists() {
            Some(candidate)
        } else {
            let with_ext = theme_dir.join(format!("{name}.toml"));
            if with_ext.exists() {
                Some(with_ext)
            } else {
                None
            }
        }
    };

    if let Some(path) = path
        && let Ok(content) = std::fs::read_to_string(&path)
    {
        match toml::from_str::<CustomTheme>(&content) {
            Ok(custom) => return Box::new(custom),
            Err(e) => tracing::warn!(
                "Failed to parse theme file {}: {e}. Theme files need top-level \
                 name, id and accent keys (not nested under [theme]).",
                path.display()
            ),
        }
    }

    resolve_theme(name)
}

pub struct ThemeStore(RwLock<Arc<dyn Theme>>);

impl ThemeStore {
    pub fn as_ref(&self) -> Arc<dyn Theme> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set(&self, theme: Box<dyn Theme>) {
        *self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::from(theme);
    }
}

pub struct BorderStyleStore(RwLock<BorderStyle>);

impl BorderStyleStore {
    pub fn to_border_type(&self) -> BorderType {
        self.current().to_border_type()
    }

    pub fn current(&self) -> BorderStyle {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set(&self, style: BorderStyle) {
        *self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = style;
    }
}

static THEME_CONFIG: LazyLock<RwLock<ThemeConfig>> =
    LazyLock::new(|| RwLock::new(load_theme_config()));

pub static THEME: LazyLock<ThemeStore> = LazyLock::new(|| {
    let config = THEME_CONFIG
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    ThemeStore(RwLock::new(Arc::from(resolve_app_theme(&config))))
});

pub static BORDER_STYLE: LazyLock<BorderStyleStore> = LazyLock::new(|| {
    let config = THEME_CONFIG
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    BorderStyleStore(RwLock::new(config.border_style.clone()))
});

pub fn current_theme_config() -> ThemeConfig {
    THEME_CONFIG
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

pub fn apply_theme(theme: String, border_style: BorderStyle) -> std::io::Result<()> {
    validate_theme_name(&theme)?;
    let mut config = current_theme_config();
    config.theme = theme;
    config.border_style = border_style.clone();
    super::write_merged_toml_document(
        &super::get_config_path().join("theme.toml"),
        &config,
        |document| {
            if config.custom.is_none() {
                document.as_table_mut().remove("custom");
            }
        },
    )?;
    THEME.set(resolve_app_theme(&config));
    BORDER_STYLE.set(border_style);
    *THEME_CONFIG
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = config;
    crate::feedback::request_redraw();
    Ok(())
}

fn validate_theme_name(name: &str) -> std::io::Result<()> {
    if ratatui_themekit::available_theme_ids().contains(&name) {
        return Ok(());
    }
    let path = if Path::new(name).is_absolute() {
        std::path::PathBuf::from(name)
    } else {
        let directory = super::get_config_path().join("theme");
        let direct = directory.join(name);
        if direct.exists() {
            direct
        } else {
            directory.join(format!("{name}.toml"))
        }
    };
    let content = std::fs::read_to_string(&path).map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!("failed to load theme {}: {error}", path.display()),
        )
    })?;
    toml::from_str::<CustomTheme>(&content).map_err(|error| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid theme {}: {error}", path.display()),
        )
    })?;
    Ok(())
}

pub fn reload_theme() -> std::io::Result<()> {
    let path = super::get_config_path().join("theme.toml");
    let content = std::fs::read_to_string(path)?;
    let config: ThemeConfig = toml::from_str(&content)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    THEME.set(resolve_app_theme(&config));
    BORDER_STYLE.set(config.border_style.clone());
    *THEME_CONFIG
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = config;
    crate::feedback::request_redraw();
    Ok(())
}

#[cfg(test)]
#[path = "tests/theme.rs"]
mod tests;
