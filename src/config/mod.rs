// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// config loading: reads config.toml from the platform config dir, creates defaults if missing.
// everything lands in the SETTINGS static so the rest of the app can just grab it.

use config::{Config as ConfigLoader, ConfigError, File};
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::{LazyLock, RwLock, RwLockReadGuard};

pub mod settings;
pub mod theme;

pub use settings::Config;

#[must_use]
pub fn get_config_path() -> PathBuf {
    dirs_next::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rmcl")
}

// seeds the config file from the bundled default on first run
fn ensure_config_exists() -> PathBuf {
    let config_path = get_config_path().join("config.toml");
    if !config_path.exists() {
        if let Some(parent) = config_path.parent()
            && let Err(e) = fs::create_dir_all(parent)
        {
            tracing::warn!(
                "Failed to create config directory {}: {}",
                parent.display(),
                e
            );
        }
        match fs::write(&config_path, include_str!("../../assets/config.toml")) {
            Ok(()) => tracing::debug!("Wrote default config to {}", config_path.display()),
            Err(e) => tracing::warn!(
                "Failed to write default config to {}: {}",
                config_path.display(),
                e
            ),
        }
    } else {
        tracing::trace!("Using existing config at {}", config_path.display());
    }
    config_path
}

pub fn load_config(config_path: &std::path::Path) -> Result<Config, ConfigError> {
    tracing::debug!("Loading config from {}", config_path.display());
    ConfigLoader::builder()
        .add_source(File::from(config_path).required(false))
        .build()?
        .try_deserialize::<Config>()
        .map(Config::normalize)
}

pub(crate) fn upgrade_config_file(path: &std::path::Path) -> io::Result<bool> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            crate::storage::write_atomic(
                path,
                include_str!("../../assets/config.toml").as_bytes(),
            )?;
            return Ok(true);
        }
        Err(error) => return Err(error),
    };
    let mut document = parse_toml_document(path, &source)?;
    let template = include_str!("../../assets/config.toml")
        .parse::<toml_edit::DocumentMut>()
        .map_err(io::Error::other)?;
    merge_missing_table(document.as_table_mut(), template.as_table());
    let upgraded = document.to_string();
    toml::from_str::<Config>(&upgraded).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("cannot upgrade {}: {error}", path.display()),
        )
    })?;
    if upgraded == source {
        return Ok(false);
    }
    crate::storage::write_atomic(path, upgraded.as_bytes())?;
    Ok(true)
}

pub(crate) fn migrate_legacy_data_paths(path: &std::path::Path) -> io::Result<bool> {
    let Some(data_dir) = dirs_next::data_dir() else {
        return Ok(false);
    };
    migrate_legacy_data_paths_from(path, &data_dir)
}

fn migrate_legacy_data_paths_from(
    path: &std::path::Path,
    data_dir: &std::path::Path,
) -> io::Result<bool> {
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let mut document = parse_toml_document(path, &source)?;
    let Some(paths) = document
        .get_mut("paths")
        .and_then(toml_edit::Item::as_table_mut)
    else {
        return Ok(false);
    };
    let old_root = data_dir.join("mcl");
    let new_root = data_dir.join("rmcl");
    let mut changed = false;
    for (key, old_path, replacement) in [
        (
            "instances_dir",
            old_root.join("instances"),
            new_root.join("instances"),
        ),
        ("meta_dir", old_root.join("meta"), new_root.join("meta")),
    ] {
        let Some(value) = paths.get_mut(key).and_then(toml_edit::Item::as_value_mut) else {
            continue;
        };
        if value
            .as_str()
            .is_some_and(|raw| settings::resolve_path(raw) == old_path)
        {
            let decor = value.decor().clone();
            *value = toml_edit::Value::from(replacement.to_string_lossy().into_owned());
            *value.decor_mut() = decor;
            changed = true;
        }
    }
    if changed {
        crate::storage::write_atomic(path, document.to_string().as_bytes())?;
    }
    Ok(changed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LauncherSettingsSave {
    pub restart_required: bool,
    pub restart_changed: bool,
    pub provider_changed: bool,
    pub modpack_updates_enabled: bool,
    pub content_updates_enabled: bool,
}

pub struct ConfigStore(RwLock<Config>);

impl ConfigStore {
    pub fn read(&self) -> RwLockReadGuard<'_, Config> {
        self.0
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn save_launcher_settings(&self, edited: Config) -> io::Result<LauncherSettingsSave> {
        let path = get_config_path().join("config.toml");
        let current = self.read().clone();
        let edited = edited.normalize();
        let persisted = load_config(&path).unwrap_or_else(|_| current.clone());
        let restart_required = current.paths.instances_dir != edited.paths.instances_dir
            || current.paths.meta_dir != edited.paths.meta_dir
            || current.ui.image_protocol != edited.ui.image_protocol;
        let restart_changed = persisted.paths.instances_dir != edited.paths.instances_dir
            || persisted.paths.meta_dir != edited.paths.meta_dir
            || persisted.ui.image_protocol != edited.ui.image_protocol;
        let provider_changed = current.content.preferred_provider
            != edited.content.preferred_provider
            || current.content.preferred_provider_only != edited.content.preferred_provider_only;
        let modpack_updates_enabled =
            !current.general.check_modpack_updates && edited.general.check_modpack_updates;
        let content_updates_enabled =
            !current.general.check_content_updates && edited.general.check_content_updates;
        write_config_document(&path, &edited)?;

        let mut runtime = edited;
        runtime
            .paths
            .instances_dir
            .clone_from(&current.paths.instances_dir);
        runtime.paths.meta_dir.clone_from(&current.paths.meta_dir);
        runtime.ui.image_protocol = current.ui.image_protocol;
        *self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = runtime;
        crate::feedback::errors::set_max_error_events(self.read().ui.max_error_events);
        Ok(LauncherSettingsSave {
            restart_required,
            restart_changed,
            provider_changed,
            modpack_updates_enabled,
            content_updates_enabled,
        })
    }

    pub fn reload(&self) -> Result<LauncherSettingsSave, ConfigError> {
        let mut config = load_config(&get_config_path().join("config.toml"))?;
        let outcome = {
            let current = self.read();
            let restart_required = current.paths.instances_dir != config.paths.instances_dir
                || current.paths.meta_dir != config.paths.meta_dir
                || current.ui.image_protocol != config.ui.image_protocol;
            let provider_changed = current.content.preferred_provider
                != config.content.preferred_provider
                || current.content.preferred_provider_only
                    != config.content.preferred_provider_only;
            let modpack_updates_enabled =
                !current.general.check_modpack_updates && config.general.check_modpack_updates;
            let content_updates_enabled =
                !current.general.check_content_updates && config.general.check_content_updates;
            // App owns a manager and several watchers rooted at these paths.
            // Keep them stable for this process; persisted path edits apply on restart.
            config
                .paths
                .instances_dir
                .clone_from(&current.paths.instances_dir);
            config.paths.meta_dir.clone_from(&current.paths.meta_dir);
            config.ui.image_protocol = current.ui.image_protocol;
            LauncherSettingsSave {
                restart_required,
                restart_changed: restart_required,
                provider_changed,
                modpack_updates_enabled,
                content_updates_enabled,
            }
        };
        *self
            .0
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = config;
        crate::feedback::errors::set_max_error_events(self.read().ui.max_error_events);
        Ok(outcome)
    }
}

pub static SETTINGS: LazyLock<ConfigStore> = LazyLock::new(|| {
    let config = {
        let path = ensure_config_exists();
        load_config(&path).unwrap_or_else(|e| {
            tracing::error!("Config load failed, using defaults: {}", e);
            Config::default()
        })
    };
    crate::feedback::errors::set_max_error_events(config.ui.max_error_events);
    ConfigStore(RwLock::new(config))
});

fn write_config_document(path: &std::path::Path, config: &Config) -> io::Result<()> {
    write_merged_toml_document(path, config, |document| {
        if config.paths.java_path.is_none()
            && let Some(paths) = document
                .get_mut("paths")
                .and_then(|item| item.as_table_mut())
        {
            paths.remove("java_path");
        }
        let default_paths = settings::Paths::default();
        if let Some(paths) = document
            .get_mut("paths")
            .and_then(|item| item.as_table_mut())
        {
            if config.paths.instances_dir == default_paths.instances_dir {
                paths.remove("instances_dir");
            }
            if config.paths.meta_dir == default_paths.meta_dir {
                paths.remove("meta_dir");
            }
        }
        if config.defaults.resolution.is_none()
            && let Some(defaults) = document
                .get_mut("defaults")
                .and_then(|item| item.as_table_mut())
        {
            defaults.remove("resolution");
        }
    })
}

pub(crate) fn write_merged_toml_document<T, F>(
    path: &std::path::Path,
    value: &T,
    edit: F,
) -> io::Result<()>
where
    T: serde::Serialize,
    F: FnOnce(&mut toml_edit::DocumentMut),
{
    let source = match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error),
    };
    let mut document = parse_toml_document(path, &source)?;
    let generated = toml::to_string_pretty(value)
        .map_err(io::Error::other)?
        .parse::<toml_edit::DocumentMut>()
        .map_err(io::Error::other)?;
    merge_table(document.as_table_mut(), generated.as_table());
    edit(&mut document);
    crate::storage::write_atomic(path, document.to_string().as_bytes())
}

fn parse_toml_document(path: &std::path::Path, source: &str) -> io::Result<toml_edit::DocumentMut> {
    source.parse::<toml_edit::DocumentMut>().map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("cannot update {}: {error}", path.display()),
        )
    })
}

fn merge_table(target: &mut toml_edit::Table, source: &toml_edit::Table) {
    for (key, source_item) in source {
        if let Some(target_item) = target.get_mut(key) {
            if let (Some(target_table), Some(source_table)) =
                (target_item.as_table_mut(), source_item.as_table())
            {
                merge_table(target_table, source_table);
                continue;
            }
            if let (Some(target_value), Some(source_value)) =
                (target_item.as_value_mut(), source_item.as_value())
            {
                let decor = target_value.decor().clone();
                *target_value = source_value.clone();
                *target_value.decor_mut() = decor;
                continue;
            }
            *target_item = source_item.clone();
        } else {
            target.insert(key, source_item.clone());
        }
    }
}

fn merge_missing_table(target: &mut toml_edit::Table, defaults: &toml_edit::Table) {
    for (key, default_item) in defaults {
        if let Some(target_item) = target.get_mut(key) {
            if let (Some(target_table), Some(default_table)) =
                (target_item.as_table_mut(), default_item.as_table())
            {
                merge_missing_table(target_table, default_table);
            }
        } else {
            target.insert(key, default_item.clone());
        }
    }
}

#[cfg(test)]
#[path = "tests/loading.rs"]
mod tests;
