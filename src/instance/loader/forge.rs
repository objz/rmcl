// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// forge installation. modern forge runs a java installer, old forge (pre-1.13)
// can't run headless so we extract the profile and libraries from the jar
// directly. the installer jar gets cleaned up either way.

use std::path::Path;

use async_trait::async_trait;

use super::{GameVersion, InstallError, InstallerError, ModLoaderInstaller};
use crate::feedback::progress::{set_action, set_sub_action};
use crate::instance::models::ModLoader;
use crate::net::{HttpClient, NetError, download_file, forge as forge_api};

pub struct ForgeInstaller;

#[async_trait]
impl ModLoaderInstaller for ForgeInstaller {
    fn loader_type(&self) -> ModLoader {
        ModLoader::Forge
    }

    async fn get_game_versions(&self, client: &HttpClient) -> Result<Vec<GameVersion>, NetError> {
        forge_api::fetch_forge_game_versions(client).await
    }

    async fn get_versions(
        &self,
        client: &HttpClient,
        game_version: &str,
    ) -> Result<Vec<String>, NetError> {
        tracing::debug!("Fetching Forge versions for Minecraft {}", game_version);
        let versions = forge_api::fetch_forge_versions(client, game_version).await?;
        tracing::debug!(
            "Fetched {} Forge version(s) for Minecraft {}",
            versions.len(),
            game_version
        );
        Ok(versions)
    }

    async fn install(
        &self,
        client: &HttpClient,
        game_version: &str,
        loader_version: &str,
        instance_dir: &Path,
        meta_dir: &Path,
    ) -> Result<(), InstallError> {
        self.install_with_java(
            client,
            game_version,
            loader_version,
            instance_dir,
            meta_dir,
            None,
        )
        .await
    }

    async fn install_with_java(
        &self,
        client: &HttpClient,
        game_version: &str,
        loader_version: &str,
        instance_dir: &Path,
        meta_dir: &Path,
        java_path: Option<&str>,
    ) -> Result<(), InstallError> {
        let installer_jar = instance_dir
            .join(crate::storage::MINECRAFT_DIR_NAME)
            .join("forge-installer.jar");
        tracing::info!(
            "Installing Forge {} for Minecraft {}",
            loader_version,
            game_version
        );
        tracing::debug!("Forge installer path: {}", installer_jar.display());

        forge_api::download_forge_installer(client, game_version, loader_version, &installer_jar)
            .await?;

        let profile_filename = format!("forge-{game_version}-{loader_version}.json");

        if has_legacy_install_profile(&installer_jar) {
            // old forge: no --installClient support, extract directly from jar
            tracing::debug!("Forge installer uses legacy install_profile.json path");
            if let Err(e) =
                install_forge_from_profile(client, &installer_jar, meta_dir, &profile_filename)
                    .await
            {
                let _ = tokio::fs::remove_file(&installer_jar).await;
                return Err(e);
            }
        } else {
            // modern forge: run the java installer
            let java_path = java_path.map(str::to_owned).unwrap_or_else(|| {
                crate::config::SETTINGS
                    .read()
                    .paths
                    .effective_java_path()
                    .map(str::to_owned)
                    .unwrap_or_else(crate::instance::java::detect_java_path)
            });
            tracing::debug!("Running Forge installer with Java {}", java_path);
            if let Err(e) = run_forge_installer(&installer_jar, instance_dir, &java_path).await {
                let _ = tokio::fs::remove_file(&installer_jar).await;
                return Err(InstallError::Installer(e));
            }

            // extract the profile from what the installer just wrote to disk
            save_forge_profile(instance_dir, meta_dir, game_version, loader_version)
                .map_err(InstallError::Installer)?;
        }

        if let Err(e) = tokio::fs::remove_file(&installer_jar).await {
            tracing::warn!("Failed to remove Forge installer JAR: {}", e);
        }

        tracing::debug!(
            "Installed Forge {} for Minecraft {}",
            loader_version,
            game_version
        );
        Ok(())
    }
}

pub async fn run_forge_installer(
    installer_path: &Path,
    instance_dir: &Path,
    java_path: &str,
) -> Result<(), InstallerError> {
    use tokio::process::Command;

    set_action("Running Forge installer...");

    let output = match Command::new(java_path)
        .arg("-jar")
        .arg(installer_path)
        .arg("--installClient")
        .current_dir(instance_dir.join(crate::storage::MINECRAFT_DIR_NAME))
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => {
            tracing::debug!(
                "Failed to spawn Forge installer {} with Java {}: {}",
                installer_path.display(),
                java_path,
                e
            );
            return Err(InstallerError::Io(e));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = if stderr.trim().is_empty() {
            format!("exit code {:?}", output.status.code())
        } else {
            stderr.lines().last().unwrap_or("unknown error").to_string()
        };
        tracing::debug!(
            "Forge installer {} failed with status {:?}: {}",
            installer_path.display(),
            output.status.code(),
            detail
        );
        return Err(InstallerError::ProcessFailed(detail));
    }

    tracing::debug!("Forge installer completed successfully");
    Ok(())
}

// old forge installers have an install_profile.json with a "versionInfo" key
// containing everything needed. modern ones don't have this structure.
pub(crate) fn has_legacy_install_profile(installer_path: &Path) -> bool {
    let file = match std::fs::File::open(installer_path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(_) => return false,
    };
    let entry = match archive.by_name("install_profile.json") {
        Ok(e) => e,
        Err(_) => return false,
    };
    let value: serde_json::Value = match serde_json::from_reader(entry) {
        Ok(v) => v,
        Err(_) => return false,
    };
    value.get("versionInfo").is_some()
}

// handles old-style forge installation by extracting the universal jar and
// library info directly from the installer, bypassing the GUI-only installer
pub(crate) async fn install_forge_from_profile(
    client: &HttpClient,
    installer_path: &Path,
    meta_dir: &Path,
    profile_filename: &str,
) -> Result<(), InstallError> {
    use std::io::Read;

    set_action("Installing legacy Forge from profile...");
    tracing::debug!(
        "Installing legacy Forge from {} into {}",
        installer_path.display(),
        meta_dir.display()
    );

    let file = std::fs::File::open(installer_path)
        .map_err(|e| InstallError::Installer(InstallerError::Io(e)))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| {
        InstallError::Installer(InstallerError::Profile(format!(
            "Failed to open installer as ZIP: {e}"
        )))
    })?;

    let profile_data: serde_json::Value = {
        let entry = archive.by_name("install_profile.json").map_err(|e| {
            InstallError::Installer(InstallerError::Profile(format!(
                "install_profile.json not found in installer: {e}"
            )))
        })?;
        serde_json::from_reader(entry).map_err(|e| {
            InstallError::Installer(InstallerError::Profile(format!(
                "Failed to parse install_profile.json: {e}"
            )))
        })?
    };

    let version_info = profile_data.get("versionInfo").ok_or_else(|| {
        InstallError::Installer(InstallerError::Profile(
            "install_profile.json missing versionInfo".into(),
        ))
    })?;
    let install_info = profile_data.get("install").ok_or_else(|| {
        InstallError::Installer(InstallerError::Profile(
            "install_profile.json missing install section".into(),
        ))
    })?;

    let libraries = version_info
        .get("libraries")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            InstallError::Installer(InstallerError::Profile(
                "missing versionInfo.libraries".into(),
            ))
        })?;

    let file_path = install_info
        .get("filePath")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            InstallError::Installer(InstallerError::Profile("missing install.filePath".into()))
        })?;

    let install_path_coord = install_info
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            InstallError::Installer(InstallerError::Profile("missing install.path".into()))
        })?;

    // extract the universal jar to the correct maven location
    let universal_maven_path = crate::instance::loader::maven::maven_coord_to_path(
        install_path_coord,
    )
    .ok_or_else(|| {
        InstallError::Installer(InstallerError::Profile(format!(
            "Invalid maven coord in install.path: {install_path_coord}"
        )))
    })?;

    set_sub_action("Extracting universal JAR...");
    let universal_dest = crate::storage::MetadataPaths::new(meta_dir)
        .libraries()
        .join(&universal_maven_path);
    if let Some(parent) = universal_dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| InstallError::Installer(InstallerError::Io(e)))?;
    }

    {
        let mut entry = archive.by_name(file_path).map_err(|e| {
            InstallError::Installer(InstallerError::Profile(format!(
                "Universal JAR '{file_path}' not found in installer: {e}"
            )))
        })?;
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| InstallError::Installer(InstallerError::Io(e)))?;
        std::fs::write(&universal_dest, &buf)
            .map_err(|e| InstallError::Installer(InstallerError::Io(e)))?;
        tracing::debug!(
            "Extracted legacy Forge universal JAR to {} ({} bytes)",
            universal_dest.display(),
            buf.len()
        );
    }

    // download libraries needed by this forge version. libs with a url field
    // are forge-hosted, libs without one are typically from mojang's library
    // server. old forge versions reference libs like launchwrapper that aren't
    // in mojang's modern version metadata, so we fetch those too.
    let libraries_dir = crate::storage::MetadataPaths::new(meta_dir).libraries();
    for lib in libraries {
        let name = lib.get("name").and_then(|v| v.as_str()).unwrap_or_default();

        let maven_path = match crate::instance::loader::maven::maven_coord_to_path(name) {
            Some(p) => p,
            None => {
                return Err(InstallError::Installer(InstallerError::Profile(format!(
                    "Invalid Maven coordinate: {name}"
                ))));
            }
        };

        let dest = libraries_dir.join(&maven_path);
        if dest.exists() {
            tracing::trace!("Legacy Forge library already cached: {}", name);
            continue;
        }

        let base_url = lib
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("https://libraries.minecraft.net/")
            .trim_end_matches('/');
        let download_url = format!("{base_url}/{maven_path}");

        set_sub_action(name);
        tracing::debug!("Downloading legacy Forge library {}", name);
        download_file(client, &download_url, &dest, |_, _| {}).await?;
    }

    set_action("Saving Forge profile...");
    // write the installer's versionInfo as compact JSON. it already has the
    // mainClass, the full library list (with name + url for forge-hosted
    // libs), and minecraftArguments (the legacy --tweakClass etc). the
    // launch flow parses this as a LaunchProfile and - if there's no
    // inheritsFrom field - implicitly inherits from the configured game
    // version so vanilla libraries layer in via resolve().
    //
    // we use serde_json::to_vec (not the pretty-print variant via
    // save_profile_json) so the written file is content-faithful: every
    // field present in the installer's versionInfo round-trips. key order
    // and whitespace may differ from the original installer JSON because
    // the source is a serde_json::Value (which doesn't preserve order),
    // but no field is silently dropped.
    let serialized = serde_json::to_vec(version_info).map_err(|e| {
        InstallError::Installer(InstallerError::Profile(format!(
            "Failed to serialize Forge profile: {e}"
        )))
    })?;
    crate::instance::loader::save_profile_bytes(meta_dir, profile_filename, &serialized)
        .map_err(|e| InstallError::Installer(InstallerError::Io(e)))?;
    Ok(())
}

fn save_forge_profile(
    instance_dir: &Path,
    meta_dir: &Path,
    game_version: &str,
    loader_version: &str,
) -> Result<(), super::InstallerError> {
    let version_dir_name = format!("{game_version}-forge-{loader_version}");
    let profile_filename = format!("forge-{game_version}-{loader_version}.json");
    super::save_installer_profile(instance_dir, meta_dir, &version_dir_name, &profile_filename)
}

#[cfg(test)]
#[path = "../tests/loader/forge.rs"]
mod tests;
