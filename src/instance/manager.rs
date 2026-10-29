// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use std::path::{Path, PathBuf};

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
    #[error("Stop instance '{0}' before renaming or deleting it")]
    InstanceRunning(String),
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
        tracing::info!(
            "Creating instance '{}' (Minecraft {}, loader {}, loader_version={})",
            name,
            game_version,
            loader,
            loader_version.unwrap_or("<none>")
        );
        tracing::debug!("Instance directory: {}", instance_dir.display());

        if instance_dir.exists() {
            tracing::warn!(
                "Cannot create instance '{}': {} already exists",
                name,
                instance_dir.display()
            );
            return Err(InstanceError::AlreadyExists(name.to_string()));
        }

        std::fs::create_dir_all(&self.instances_dir)?;
        match std::fs::create_dir(&instance_dir) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(InstanceError::AlreadyExists(name.to_owned()));
            }
            Err(error) => return Err(error.into()),
        }

        let result = self
            .create_inner(name, game_version, loader, loader_version, &instance_dir)
            .await;

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
        let settings = crate::config::SETTINGS.read().clone();
        let instance_dir = std::path::absolute(instance_dir)?;
        let minecraft_dir = instance_dir.join(crate::storage::MINECRAFT_DIR_NAME);
        let environment = crate::instance::java::merge_environment(
            &settings.defaults.environment,
            &Default::default(),
        );
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
        let java_path = crate::instance::java::resolve_java_path_in(
            settings.paths.effective_java_path(),
            &minecraft_dir,
            &environment,
        );

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

        let runtime = crate::instance::java::probe_java_in(
            Path::new(&java_path),
            &minecraft_dir,
            &environment,
        )
        .await?;
        crate::net::mojang::download_libraries_for_platform(
            &self.client,
            &version_meta,
            &self.meta_dir,
            &runtime.platform,
        )
        .await?;

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
            .install_with_java(
                &self.client,
                game_version,
                effective_loader_version,
                &instance_dir,
                &self.meta_dir,
                Some(&java_path),
                &environment,
            )
            .await
            .map_err(|e| match e {
                InstallError::Download(net_error) => InstanceError::Download(net_error),
                InstallError::Installer(installer_error) => {
                    InstanceError::InstallerError(installer_error)
                }
            })?;

        let defaults = &settings.defaults;
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
        validate_name(&config.name)?;
        let settings = crate::config::SETTINGS.read().clone();
        let instance_dir = std::path::absolute(self.instances_dir.join(&config.name))?;
        let minecraft_dir = instance_dir.join(crate::storage::MINECRAFT_DIR_NAME);
        std::fs::create_dir_all(&minecraft_dir)?;
        let environment = crate::instance::java::merge_environment(
            &settings.defaults.environment,
            &config.environment,
        );
        let java_path = crate::instance::java::resolve_java_path_in(
            config
                .java_path
                .as_deref()
                .or(settings.paths.effective_java_path()),
            &minecraft_dir,
            &environment,
        );
        let task = crate::feedback::progress::ProgressTask::start("Rebuilding runtime");
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
        let runtime = crate::instance::java::probe_java_in(
            Path::new(&java_path),
            &minecraft_dir,
            &environment,
        )
        .await?;
        crate::net::mojang::download_libraries_for_platform(
            &self.client,
            &version_meta,
            &self.meta_dir,
            &runtime.platform,
        )
        .await?;
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
                    &instance_dir,
                    &self.meta_dir,
                    Some(&java_path),
                    &environment,
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
        if crate::instance::runtime::is_active(name) {
            return Err(InstanceError::InstanceRunning(name.to_owned()));
        }
        let _lock = crate::instance::runtime::lock_instance(&self.instances_dir, name)?;
        let _config_lock = self.config_lock(name)?;
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
        if crate::instance::runtime::is_active(old_name) {
            return Err(InstanceError::InstanceRunning(old_name.to_owned()));
        }
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
        validate_name(new_name)?;
        let _old_lock = crate::instance::runtime::lock_instance(&self.instances_dir, old_name)?;
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
        let aliases_source = paths_alias(&old_dir, &new_dir)?;
        let _new_lock = if aliases_source {
            None
        } else {
            Some(crate::instance::runtime::lock_instance(
                &self.instances_dir,
                new_name,
            )?)
        };
        if std::fs::symlink_metadata(&new_dir).is_ok() && !aliases_source {
            tracing::warn!(
                "Cannot rename instance '{}' to '{}': destination exists at {}",
                old_name,
                new_name,
                new_dir.display()
            );
            return Err(InstanceError::AlreadyExists(new_name.to_string()));
        }
        let _old_config_lock = self.config_lock(old_name)?;
        let _new_config_lock = if aliases_source {
            None
        } else {
            Some(self.config_lock(new_name)?)
        };
        let mut config = self.load_one(old_name)?;
        config.name = new_name.to_owned();
        let json = serde_json::to_vec_pretty(&config)?;
        tracing::info!("Renaming instance '{}' to '{}'", old_name, new_name);
        if let Err(e) = rename_path(&old_dir, &new_dir, aliases_source) {
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
            if let Err(rollback_error) = rename_path(&new_dir, &old_dir, aliases_source) {
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
            let Some(directory_name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if validate_name(&directory_name).is_err() {
                continue;
            }
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
                Ok(mut config) => {
                    config.name = directory_name;
                    instances.push(config);
                }
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
            tracing::debug!(
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
            Ok(mut config) => {
                config.name = name.to_owned();
                Ok(config)
            }
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
        let _lock = self.config_lock(&instance.name)?;
        let mut instance = instance.clone();
        match self.load_one(&instance.name) {
            Ok(current) => {
                instance.last_played = instance.last_played.max(current.last_played);
                // Selection changes must commit with their corresponding filesystem payload.
                instance.config_sync_profile = current.config_sync_profile;
            }
            Err(InstanceError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }
        self.save_unlocked(&instance)
    }

    pub(crate) fn save_config_sync_profile(
        &self,
        name: &str,
        profile: Option<String>,
    ) -> Result<InstanceConfig, InstanceError> {
        let _lock = self.config_lock(name)?;
        let mut config = self.load_one(name)?;
        config.config_sync_profile = profile;
        self.save_unlocked(&config)?;
        Ok(config)
    }

    pub(crate) fn config_lock(&self, name: &str) -> Result<std::fs::File, InstanceError> {
        validate_name(name)?;
        // the lock must survive replacing or renaming the instance directory.
        std::fs::create_dir_all(&self.instances_dir)?;
        let path = self.instances_dir.join(format!(".rmcl-config-{name}.lock"));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)?;
        file.lock()?;
        Ok(file)
    }

    fn save_unlocked(&self, instance: &InstanceConfig) -> Result<(), InstanceError> {
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
        let _lock = self.config_lock(name)?;
        let mut config = self.load_one(name)?;
        config.last_played = Some(chrono::Utc::now());
        tracing::debug!("Updating last_played for '{}'", name);
        self.save_unlocked(&config)
    }
}

pub(crate) fn validate_name(name: &str) -> Result<(), InstanceError> {
    if name.is_empty() || name.len() > 64 {
        return Err(InstanceError::InvalidName(format!(
            "Name must be 1-64 chars, got: {:?}",
            name
        )));
    }
    if name.starts_with('.') || !portable_component(name) {
        return Err(InstanceError::InvalidName(format!(
            "Name contains invalid characters or is reserved: {:?}",
            name
        )));
    }
    Ok(())
}

pub(crate) fn portable_component(name: &str) -> bool {
    if name.is_empty()
        || name.ends_with([' ', '.'])
        || name.chars().any(|c| {
            c.is_control() || matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*')
        })
    {
        return false;
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    !matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$" | "CONIN$" | "CONOUT$"
    ) && !["COM", "LPT"].into_iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
    })
}

pub(crate) fn paths_alias(left: &Path, right: &Path) -> std::io::Result<bool> {
    if left.parent() != right.parent() {
        return Ok(false);
    }
    let right_metadata = match std::fs::symlink_metadata(right) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let left_metadata = std::fs::symlink_metadata(left)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if left_metadata.dev() != right_metadata.dev()
            || left_metadata.ino() != right_metadata.ino()
        {
            return Ok(false);
        }
        // Distinct hard links are not aliases of the same directory entry.
        if left.file_name() != right.file_name() {
            let mut left_entry = false;
            let mut right_entry = false;
            for entry in std::fs::read_dir(
                left.parent()
                    .ok_or_else(|| std::io::Error::other("Path has no parent"))?,
            )? {
                let name = entry?.file_name();
                left_entry |= Some(name.as_os_str()) == left.file_name();
                right_entry |= Some(name.as_os_str()) == right.file_name();
            }
            if left_entry && right_entry {
                return Ok(false);
            }
        }
        Ok(true)
    }
    #[cfg(not(unix))]
    {
        if left_metadata.file_type().is_symlink() || right_metadata.file_type().is_symlink() {
            return Ok(false);
        }
        Ok(std::fs::canonicalize(left)? == std::fs::canonicalize(right)?)
    }
}

pub(crate) fn rename_path(from: &Path, to: &Path, aliases_source: bool) -> std::io::Result<()> {
    if !aliases_source {
        return std::fs::rename(from, to);
    }
    let temporary = tempfile::Builder::new()
        .prefix(".rmcl-rename-")
        .tempdir_in(
            from.parent()
                .ok_or_else(|| std::io::Error::other("Path has no parent"))?,
        )?;
    let staged = temporary.path().join("original");
    std::fs::rename(from, &staged)?;
    if let Err(error) = std::fs::rename(&staged, to) {
        if let Err(rollback) = std::fs::rename(&staged, from) {
            let retained = temporary.keep();
            return Err(std::io::Error::other(format!(
                "Could not rename '{}' to '{}': {error}; rollback failed: {rollback}; original retained at '{}'",
                from.display(),
                to.display(),
                retained.join("original").display(),
            )));
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/manager.rs"]
mod tests;
