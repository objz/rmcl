// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// CRUD for instances: create, delete, rename, load, save.
// creation is the heavy one since it downloads the game, assets, and libraries.

use std::path::PathBuf;

use chrono::Utc;
use thiserror::Error;

use crate::instance::loader::InstallError;
use crate::instance::loader::InstallerError;
use crate::instance::models::{InstanceConfig, ModLoader};

#[derive(Debug, Error)]
pub enum InstanceError {
    #[error("Instance '{0}' already exists")]
    AlreadyExists(String),
    #[error("Instance '{0}' not found")]
    NotFound(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Download error: {0}")]
    Download(#[from] crate::net::NetError),
    #[error("Installer error: {0}")]
    InstallerError(#[from] InstallerError),
    #[error("Invalid instance name: {0}")]
    InvalidName(String),
}

pub struct InstanceManager {
    pub instances_dir: PathBuf,
    /// shared across all instances: versions, libraries, assets
    pub meta_dir: PathBuf,
    client: crate::net::HttpClient,
}

impl InstanceManager {
    pub fn new(instances_dir: impl Into<PathBuf>, meta_dir: impl Into<PathBuf>) -> Self {
        let manager = InstanceManager {
            instances_dir: instances_dir.into(),
            meta_dir: meta_dir.into(),
            client: crate::net::HttpClient::new(),
        };
        tracing::trace!(
            "Created InstanceManager with instances_dir={} meta_dir={}",
            manager.instances_dir.display(),
            manager.meta_dir.display()
        );
        manager
    }

    pub async fn create(
        &self,
        name: &str,
        game_version: &str,
        loader: ModLoader,
        loader_version: Option<&str>,
    ) -> Result<InstanceConfig, InstanceError> {
        validate_name(name)?;

        let instance_dir = self.instances_dir.join(name);
        let instance_json = instance_dir.join("instance.json");
        tracing::info!(
            "Creating instance '{}' (Minecraft {}, loader {}, loader_version={})",
            name,
            game_version,
            loader,
            loader_version.unwrap_or("<none>")
        );
        tracing::debug!("Instance directory: {}", instance_dir.display());

        if instance_json.exists() {
            tracing::warn!(
                "Cannot create instance '{}': {} already exists",
                name,
                instance_json.display()
            );
            return Err(InstanceError::AlreadyExists(name.to_string()));
        }

        // leftover directory without config = botched previous creation, nuke it
        if instance_dir.exists() && !instance_json.exists() {
            tracing::warn!(
                "Removing incomplete instance directory before recreating '{}': {}",
                name,
                instance_dir.display()
            );
            std::fs::remove_dir_all(&instance_dir)?;
        }

        std::fs::create_dir_all(&instance_dir)?;

        let result = self
            .create_inner(name, game_version, loader, loader_version, &instance_dir)
            .await;

        // clean up on failure so there's no half-baked instance left around
        if result.is_err() {
            tracing::debug!(
                "Cleaning up incomplete instance '{}' after creation failed",
                name
            );
            if let Err(cleanup_error) = std::fs::remove_dir_all(&instance_dir) {
                tracing::warn!(
                    "Failed to clean up incomplete instance directory {}: {}",
                    instance_dir.display(),
                    cleanup_error
                );
            } else {
                tracing::debug!(
                    "Cleaned up incomplete instance directory {}",
                    instance_dir.display()
                );
            }
        }

        result
    }

    async fn create_inner(
        &self,
        name: &str,
        game_version: &str,
        loader: ModLoader,
        loader_version: Option<&str>,
        instance_dir: &std::path::Path,
    ) -> Result<InstanceConfig, InstanceError> {
        let minecraft_dir = instance_dir.join(crate::storage::MINECRAFT_DIR_NAME);
        tracing::debug!(
            "Preparing Minecraft directory for '{}': {}",
            name,
            minecraft_dir.display()
        );
        for subdir in &["mods", "config", "resourcepacks", "shaderpacks", "saves"] {
            let path = minecraft_dir.join(subdir);
            std::fs::create_dir_all(&path)?;
            tracing::trace!("Ensured instance subdirectory {}", path.display());
        }

        // forge insists on this file existing, even if it's empty json. thanks forge.
        let launcher_profiles_path = minecraft_dir.join("launcher_profiles.json");
        if !launcher_profiles_path.exists() {
            std::fs::write(&launcher_profiles_path, "{}")?;
            tracing::debug!(
                "Created launcher_profiles.json for '{}': {}",
                name,
                launcher_profiles_path.display()
            );
        }

        let metadata_paths = crate::storage::MetadataPaths::new(&self.meta_dir);
        for meta_subdir in &[
            metadata_paths.versions(),
            metadata_paths.libraries(),
            metadata_paths.assets().join("objects"),
            metadata_paths.assets().join("indexes"),
        ] {
            std::fs::create_dir_all(meta_subdir)?;
            tracing::trace!("Ensured metadata directory {}", meta_subdir.display());
        }

        tracing::debug!("Fetching Mojang version manifest for '{}'", name);
        let manifest = crate::net::mojang::fetch_version_manifest(&self.client).await?;
        tracing::debug!(
            "Loaded Mojang manifest with {} version(s); latest release={} snapshot={}",
            manifest.versions.len(),
            manifest.latest.release,
            manifest.latest.snapshot
        );

        let version_entry = match manifest.versions.iter().find(|v| v.id == game_version) {
            Some(v) => {
                tracing::debug!(
                    "Resolved Minecraft version '{}' as {} entry",
                    game_version,
                    v.version_type
                );
                v
            }
            None => {
                return Err(InstanceError::InvalidName(format!(
                    "Minecraft version '{}' not found in manifest with {} entries",
                    game_version,
                    manifest.versions.len()
                )));
            }
        };

        let (version_meta, raw_meta_bytes) =
            crate::net::mojang::fetch_version_meta_with_raw(&self.client, version_entry).await?;
        tracing::debug!(
            "Fetched version meta '{}' with {} libraries and asset index {}",
            version_meta.id,
            version_meta.libraries.len(),
            version_meta.asset_index.id
        );

        crate::net::mojang::download_client_jar(&self.client, &version_meta, &self.meta_dir)
            .await?;

        let meta_json_path = metadata_paths
            .versions()
            .join(game_version)
            .join("meta.json");
        crate::storage::write_atomic(&meta_json_path, &raw_meta_bytes)?;
        tracing::debug!("Saved version meta to {}", meta_json_path.display());

        crate::net::mojang::download_libraries(&self.client, &version_meta, &self.meta_dir).await?;

        crate::net::mojang::download_assets(&self.client, &version_meta, &self.meta_dir).await?;

        let installer = crate::instance::loader::get_installer(loader);
        let effective_loader_version = match loader_version {
            Some(v) => v,
            None if loader == ModLoader::Vanilla => "vanilla",
            None => {
                return Err(InstanceError::InvalidName(format!(
                    "A loader version is required for {}",
                    loader
                )));
            }
        };
        tracing::debug!(
            "Installing loader {} {} for instance '{}'",
            loader,
            effective_loader_version,
            name
        );
        installer
            .install(
                &self.client,
                game_version,
                effective_loader_version,
                instance_dir,
                &self.meta_dir,
            )
            .await
            .map_err(|e| match e {
                InstallError::Download(net_error) => InstanceError::Download(net_error),
                InstallError::Installer(installer_error) => {
                    InstanceError::InstallerError(installer_error)
                }
            })?;

        let defaults = crate::config::SETTINGS.read().defaults.clone();
        let config = InstanceConfig {
            name: name.to_string(),
            game_version: game_version.to_string(),
            loader,
            loader_version: loader_version.map(String::from),
            created: Utc::now(),
            last_played: None,
            java_path: None,
            memory_max: None,
            memory_min: None,
            jvm_args: vec![],
            environment: Default::default(),
            window_mode: defaults.window_mode,
            inherit_window_mode: true,
            resolution: defaults.resolution,
            inherit_resolution: true,
            preferred_account: None,
            pre_launch_command: Default::default(),
            post_exit_command: Default::default(),
            glfw_path: None,
            config_sync_profile: None,
            modpack_source: None,
        };

        self.save(&config)?;
        tracing::info!("Created instance '{}'", name);

        crate::feedback::progress::clear();
        Ok(config)
    }

    pub async fn repair_runtime_cache(&self, config: &InstanceConfig) -> Result<(), InstanceError> {
        let task = crate::feedback::progress::ProgressTask::start(format!(
            "Rebuilding runtime for '{}'",
            config.name
        ));
        task.set_sub_action(format!("Minecraft {}", config.game_version));
        let metadata_paths = crate::storage::MetadataPaths::new(&self.meta_dir);
        for directory in [
            metadata_paths.versions(),
            metadata_paths.libraries(),
            metadata_paths.assets().join("objects"),
            metadata_paths.assets().join("indexes"),
            metadata_paths.loader_profiles(),
        ] {
            std::fs::create_dir_all(directory)?;
        }

        let meta_path = metadata_paths
            .versions()
            .join(&config.game_version)
            .join("meta.json");
        let cached_meta = match std::fs::read(&meta_path) {
            Ok(raw) => match serde_json::from_slice(&raw) {
                Ok(parsed)
                    if serde_json::from_slice::<serde_json::Value>(&raw)
                        .ok()
                        .is_some_and(|value| {
                            value.as_object().is_some_and(|object| {
                                object.contains_key("arguments")
                                    || object.contains_key("minecraftArguments")
                            })
                        }) =>
                {
                    Some((parsed, raw))
                }
                Ok(_) => {
                    tracing::warn!(
                        "Cached metadata for Minecraft {} is incomplete; downloading it again",
                        config.game_version
                    );
                    None
                }
                Err(error) => {
                    tracing::warn!(
                        "Cached metadata for Minecraft {} is invalid; downloading it again: {error}",
                        config.game_version
                    );
                    None
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let (version_meta, raw_meta, fetched_meta) = if let Some((parsed, raw)) = cached_meta {
            (parsed, raw, false)
        } else {
            let manifest = crate::net::mojang::fetch_version_manifest(&self.client).await?;
            let version_entry = manifest
                .versions
                .iter()
                .find(|version| version.id == config.game_version)
                .ok_or_else(|| {
                    InstanceError::InvalidName(format!(
                        "Minecraft version '{}' is no longer present in Mojang's manifest",
                        config.game_version
                    ))
                })?;
            let (parsed, raw) =
                crate::net::mojang::fetch_version_meta_with_raw(&self.client, version_entry)
                    .await?;
            (parsed, raw, true)
        };
        crate::net::mojang::download_client_jar(&self.client, &version_meta, &self.meta_dir)
            .await?;
        if let Some(parent) = meta_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if fetched_meta {
            crate::storage::write_atomic(&meta_path, &raw_meta)?;
        }
        crate::net::mojang::download_libraries(&self.client, &version_meta, &self.meta_dir).await?;
        crate::net::mojang::download_assets(&self.client, &version_meta, &self.meta_dir).await?;

        if config.loader != ModLoader::Vanilla {
            let loader_version = config.loader_version.as_deref().ok_or_else(|| {
                InstanceError::InvalidName(format!(
                    "Instance '{}' has no {} loader version",
                    config.name, config.loader
                ))
            })?;
            task.set_sub_action(format!("{} {}", config.loader, loader_version));
            crate::instance::loader::get_installer(config.loader)
                .install_with_java(
                    &self.client,
                    &config.game_version,
                    loader_version,
                    &self.instances_dir.join(&config.name),
                    &self.meta_dir,
                    config.java_path.as_deref(),
                )
                .await
                .map_err(|error| match error {
                    InstallError::Download(error) => InstanceError::Download(error),
                    InstallError::Installer(error) => InstanceError::InstallerError(error),
                })?;
            let profile_filename = crate::instance::loader::profile_filename(
                config.loader,
                &config.game_version,
                loader_version,
            )
            .expect("non-vanilla loaders have profile filenames");
            let profile_path = metadata_paths.loader_profiles().join(profile_filename);
            let profile: crate::launch_profile::model::LaunchProfile =
                serde_json::from_slice(&std::fs::read(&profile_path)?)?;
            if profile.id.trim().is_empty()
                || profile
                    .main_class
                    .as_deref()
                    .is_none_or(|main_class| main_class.trim().is_empty())
            {
                return Err(InstanceError::InstallerError(InstallerError::Profile(
                    format!(
                        "Installed {} profile {} is missing id or mainClass",
                        config.loader,
                        profile_path.display()
                    ),
                )));
            }
        }
        task.finish();
        Ok(())
    }

    pub fn delete(&self, name: &str) -> Result<(), InstanceError> {
        validate_name(name)?;
        let instance_dir = self.instances_dir.join(name);
        if !instance_dir.exists() {
            tracing::warn!(
                "Cannot delete missing instance '{}': {}",
                name,
                instance_dir.display()
            );
            return Err(InstanceError::NotFound(name.to_string()));
        }
        tracing::info!("Deleting instance '{}' at {}", name, instance_dir.display());
        if let Err(e) = std::fs::remove_dir_all(&instance_dir) {
            tracing::error!(
                "Failed to delete instance directory {}: {}",
                instance_dir.display(),
                e
            );
            return Err(e.into());
        }
        if let Err(e) = crate::instance::desktop::remove(name) {
            tracing::warn!("Failed to remove desktop shortcut for '{}': {}", name, e);
        }
        Ok(())
    }

    pub fn rename(&self, old_name: &str, new_name: &str) -> Result<(), InstanceError> {
        validate_name(old_name)?;
        let new_name = new_name.trim();
        if new_name.is_empty() {
            tracing::warn!("Cannot rename instance '{}': new name is empty", old_name);
            return Err(InstanceError::InvalidName(
                "Name cannot be empty".to_string(),
            ));
        }
        if old_name == new_name {
            tracing::debug!("Ignoring no-op instance rename '{}'", old_name);
            return Ok(());
        }
        // same path-traversal guards as create: without this, "../x" or ".x"
        // would move the instance directory out of (or hide it inside) the
        // instances root.
        validate_name(new_name)?;
        let old_dir = self.instances_dir.join(old_name);
        let new_dir = self.instances_dir.join(new_name);
        if !old_dir.exists() {
            tracing::warn!(
                "Cannot rename missing instance '{}': {}",
                old_name,
                old_dir.display()
            );
            return Err(InstanceError::NotFound(old_name.to_string()));
        }
        if new_dir.exists() {
            tracing::warn!(
                "Cannot rename instance '{}' to '{}': destination exists at {}",
                old_name,
                new_name,
                new_dir.display()
            );
            return Err(InstanceError::AlreadyExists(new_name.to_string()));
        }
        let mut config = self.load_one(old_name)?;
        config.name = new_name.to_owned();
        let json = serde_json::to_vec_pretty(&config)?;
        tracing::info!("Renaming instance '{}' to '{}'", old_name, new_name);
        if let Err(e) = std::fs::rename(&old_dir, &new_dir) {
            tracing::error!(
                "Failed to rename instance directory {} to {}: {}",
                old_dir.display(),
                new_dir.display(),
                e
            );
            return Err(e.into());
        }

        let config_path = new_dir.join("instance.json");
        if let Err(error) = crate::storage::write_atomic(&config_path, &json) {
            if let Err(rollback_error) = std::fs::rename(&new_dir, &old_dir) {
                tracing::error!(
                    "Failed to roll back instance rename from {} to {}: {}",
                    new_dir.display(),
                    old_dir.display(),
                    rollback_error
                );
            }
            return Err(error.into());
        }
        if let Err(e) = crate::instance::desktop::rename(old_name, &config) {
            tracing::warn!("Failed to rename desktop shortcut: {}", e);
        }

        Ok(())
    }

    pub fn load_all(&self) -> Vec<InstanceConfig> {
        let mut instances = vec![];
        tracing::debug!("Loading instances from {}", self.instances_dir.display());
        let read_dir = match std::fs::read_dir(&self.instances_dir) {
            Ok(rd) => rd,
            Err(e) => {
                tracing::error!(
                    "Failed to read instances directory {}: {}",
                    self.instances_dir.display(),
                    e
                );
                return instances;
            }
        };
        for entry in read_dir {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    tracing::error!("Failed to read directory entry: {}", e);
                    continue;
                }
            };
            let config_path = entry.path().join("instance.json");
            if !config_path.exists() {
                tracing::trace!("Skipping non-instance directory {}", entry.path().display());
                continue;
            }
            let contents = match std::fs::read_to_string(&config_path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("Failed to read {}: {}", config_path.display(), e);
                    continue;
                }
            };
            match serde_json::from_str::<InstanceConfig>(&contents) {
                Ok(config) => instances.push(config),
                Err(e) => {
                    tracing::error!("Failed to parse {}: {}", config_path.display(), e);
                }
            }
        }
        tracing::debug!("Loaded {} instance(s)", instances.len());
        instances
    }

    pub fn load_one(&self, name: &str) -> Result<InstanceConfig, InstanceError> {
        validate_name(name)?;

        let config_path = self.instances_dir.join(name).join("instance.json");
        if !config_path.exists() {
            tracing::warn!(
                "Instance '{}' config is missing at {}",
                name,
                config_path.display()
            );
            return Err(InstanceError::NotFound(name.to_string()));
        }

        tracing::debug!("Loading instance '{}' from {}", name, config_path.display());
        let contents = match std::fs::read_to_string(&config_path) {
            Ok(contents) => contents,
            Err(e) => {
                tracing::error!(
                    "Failed to read instance '{}' config {}: {}",
                    name,
                    config_path.display(),
                    e
                );
                return Err(e.into());
            }
        };
        match serde_json::from_str::<InstanceConfig>(&contents) {
            Ok(config) => Ok(config),
            Err(e) => {
                tracing::error!(
                    "Failed to parse instance '{}' config {}: {}",
                    name,
                    config_path.display(),
                    e
                );
                Err(e.into())
            }
        }
    }

    pub fn save(&self, instance: &InstanceConfig) -> Result<(), InstanceError> {
        validate_name(&instance.name)?;
        let instance_dir = self.instances_dir.join(&instance.name);
        if !instance_dir.is_dir() {
            return Err(InstanceError::NotFound(instance.name.clone()));
        }
        let config_path = instance_dir.join("instance.json");
        let json = serde_json::to_string_pretty(instance)?;
        crate::storage::write_atomic(&config_path, json.as_bytes())?;
        tracing::debug!(
            "Saved instance '{}' config to {}",
            instance.name,
            config_path.display()
        );
        Ok(())
    }

    pub fn touch_last_played(&self, name: &str) -> Result<(), InstanceError> {
        let mut config = self.load_one(name)?;
        config.last_played = Some(chrono::Utc::now());
        tracing::debug!("Updating last_played for '{}'", name);
        self.save(&config)
    }
}

// guard against path traversal and other filesystem shenanigans
fn validate_name(name: &str) -> Result<(), InstanceError> {
    if name.is_empty() || name.len() > 64 {
        return Err(InstanceError::InvalidName(format!(
            "Name must be 1-64 chars, got: {:?}",
            name
        )));
    }
    if name.contains('/')
        || name.contains('\\')
        || name.starts_with('.')
        || name.chars().any(char::is_control)
    {
        return Err(InstanceError::InvalidName(format!(
            "Name contains invalid characters: {:?}",
            name
        )));
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/manager.rs"]
mod tests;
