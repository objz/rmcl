// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// mod loader installation. each loader (fabric, forge, neoforge, quilt, vanilla)
// implements the same trait so the UI can treat them uniformly: pick game version,
// pick loader version, install. the actual installation strategies differ wildly
// though (fabric/quilt just download jars, forge/neoforge run a whole java installer).

mod fabric;
pub mod forge;
pub mod maven;
pub mod neoforge;
mod quilt;
mod vanilla;

use std::path::Path;

use async_trait::async_trait;
use thiserror::Error;

use crate::instance::models::ModLoader;
use crate::net::{HttpClient, NetError};

#[derive(Debug, Error)]
pub enum InstallerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Process failed: {0}")]
    ProcessFailed(String),
    #[error("Profile error: {0}")]
    Profile(String),
}

#[derive(Debug, Error)]
pub enum InstallError {
    #[error("Download error: {0}")]
    Download(#[from] NetError),
    #[error("Installer error: {0}")]
    Installer(#[from] InstallerError),
}

pub use vanilla::VanillaInstaller;

#[derive(Debug, Clone)]
pub struct GameVersion {
    pub id: String,
    pub stable: bool,
}

#[async_trait]
pub trait ModLoaderInstaller: Send + Sync {
    fn loader_type(&self) -> ModLoader;

    async fn get_game_versions(&self, client: &HttpClient) -> Result<Vec<GameVersion>, NetError>;

    async fn get_versions(
        &self,
        client: &HttpClient,
        game_version: &str,
    ) -> Result<Vec<String>, NetError>;

    async fn install(
        &self,
        client: &HttpClient,
        game_version: &str,
        loader_version: &str,
        instance_dir: &Path,
        meta_dir: &Path,
    ) -> Result<(), InstallError>;

    async fn install_with_java(
        &self,
        client: &HttpClient,
        game_version: &str,
        loader_version: &str,
        instance_dir: &Path,
        meta_dir: &Path,
        java_path: Option<&str>,
    ) -> Result<(), InstallError> {
        let _ = java_path;
        self.install(client, game_version, loader_version, instance_dir, meta_dir)
            .await
    }
}

// writes raw profile JSON bytes to meta_dir/loader-profiles/<filename>.
// callers that already have the upstream bytes (fabric/quilt http fetch,
// legacy forge versionInfo extract) use this directly to keep the on-disk
// file byte-for-byte identical to the source.
pub(crate) fn save_profile_bytes(
    meta_dir: &Path,
    filename: &str,
    bytes: &[u8],
) -> std::io::Result<()> {
    let profiles_dir = crate::storage::MetadataPaths::new(meta_dir).loader_profiles();
    std::fs::create_dir_all(&profiles_dir)?;
    crate::storage::write_atomic(&profiles_dir.join(filename), bytes)
}

pub(crate) fn profile_filename(
    loader: ModLoader,
    game_version: &str,
    loader_version: &str,
) -> Option<String> {
    match loader {
        ModLoader::Vanilla => None,
        ModLoader::Fabric => Some(format!("fabric-{game_version}-{loader_version}.json")),
        ModLoader::Quilt => Some(format!("quilt-{game_version}-{loader_version}.json")),
        ModLoader::Forge => Some(format!("forge-{game_version}-{loader_version}.json")),
        ModLoader::NeoForge => Some(format!("neoforge-{loader_version}.json")),
    }
}

// used by forge/neoforge. their java installer drops a version json into
// .minecraft/versions/. we copy that file byte-for-byte to our loader
// profile cache so launch-time code sees the full upstream JSON -
// inheritsFrom, arguments.jvm, library rules, all of it - instead of a
// stripped-down version that would silently drop modern features (e.g.
// the --add-opens flags forge 1.17+ ships for java 17+ support).
pub(crate) fn save_installer_profile(
    instance_dir: &Path,
    meta_dir: &Path,
    version_dir_name: &str,
    profile_filename: &str,
) -> Result<(), InstallerError> {
    let ver_json_path = instance_dir
        .join(crate::storage::MINECRAFT_DIR_NAME)
        .join("versions")
        .join(version_dir_name)
        .join(format!("{version_dir_name}.json"));

    if !ver_json_path.exists() {
        tracing::debug!(
            "Installer profile JSON missing: {}",
            ver_json_path.display()
        );
        return Err(InstallerError::Profile(format!(
            "Version JSON not found at {}",
            ver_json_path.display()
        )));
    }

    tracing::debug!(
        "Saving installer profile {} from {}",
        profile_filename,
        ver_json_path.display()
    );
    let raw = std::fs::read(&ver_json_path)?;
    let profile: crate::launch_profile::model::LaunchProfile = serde_json::from_slice(&raw)
        .map_err(|error| {
            InstallerError::Profile(format!(
                "Invalid installer profile {}: {error}",
                ver_json_path.display()
            ))
        })?;
    if profile.id.trim().is_empty()
        || profile
            .main_class
            .as_deref()
            .is_none_or(|main_class| main_class.trim().is_empty())
    {
        return Err(InstallerError::Profile(format!(
            "Installer profile {} is missing id or mainClass",
            ver_json_path.display()
        )));
    }

    let profiles_dir = crate::storage::MetadataPaths::new(meta_dir).loader_profiles();
    std::fs::create_dir_all(&profiles_dir)?;
    let profile_path = profiles_dir.join(profile_filename);
    crate::storage::write_atomic(&profile_path, &raw)?;
    tracing::debug!(
        "Saved installer profile to {} ({} bytes)",
        profile_path.display(),
        raw.len()
    );
    Ok(())
}

pub fn get_installer(loader: ModLoader) -> Box<dyn ModLoaderInstaller + Send + Sync> {
    match loader {
        ModLoader::Vanilla => Box::new(vanilla::VanillaInstaller),
        ModLoader::Fabric => Box::new(fabric::FabricInstaller),
        ModLoader::Forge => Box::new(forge::ForgeInstaller),
        ModLoader::NeoForge => Box::new(neoforge::NeoForgeInstaller),
        ModLoader::Quilt => Box::new(quilt::QuiltInstaller),
    }
}

#[cfg(test)]
#[path = "../tests/loader/installers.rs"]
mod tests;
