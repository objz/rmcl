// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// curseforge modpack archives: manifest.json references provider file ids,
// while the overrides directory contains configs and other bundled files.

use std::path::Path;

use serde::Deserialize;

use crate::feedback::progress;
use crate::instance::{InstanceConfig, InstanceManager, ModLoader};

use super::{ImportSummary, PackFormat};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    name: String,
    version: String,
    minecraft: Minecraft,
    #[serde(default)]
    files: Vec<ManifestFile>,
    #[serde(default = "default_overrides")]
    overrides: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Minecraft {
    version: String,
    #[serde(default)]
    mod_loaders: Vec<ManifestLoader>,
}

#[derive(Debug, Deserialize)]
struct ManifestLoader {
    id: String,
    #[serde(default)]
    primary: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestFile {
    #[serde(rename = "projectID")]
    _project_id: u64,
    #[serde(rename = "fileID")]
    file_id: u64,
    #[serde(default = "required_by_default")]
    required: bool,
}

fn default_overrides() -> String {
    "overrides".to_owned()
}

fn required_by_default() -> bool {
    true
}

fn parse(path: &Path) -> Result<Manifest, String> {
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
    let entry = archive
        .by_name("manifest.json")
        .map_err(|_| "Missing manifest.json in CurseForge pack".to_owned())?;
    serde_json::from_reader(entry).map_err(|error| format!("Invalid CurseForge manifest: {error}"))
}

fn loader(manifest: &Manifest) -> (ModLoader, Option<String>) {
    let id = manifest
        .minecraft
        .mod_loaders
        .iter()
        .find(|loader| loader.primary)
        .or_else(|| manifest.minecraft.mod_loaders.first())
        .map(|loader| loader.id.as_str());
    let Some(id) = id else {
        return (ModLoader::Vanilla, None);
    };
    let (name, version) = id.split_once('-').unwrap_or((id, ""));
    let loader = match name.to_ascii_lowercase().as_str() {
        "fabric" => ModLoader::Fabric,
        "forge" => ModLoader::Forge,
        "neoforge" => ModLoader::NeoForge,
        "quilt" => ModLoader::Quilt,
        _ => ModLoader::Vanilla,
    };
    (
        loader,
        (!version.is_empty() && loader != ModLoader::Vanilla).then(|| version.to_owned()),
    )
}

pub fn build_summary(path: &Path) -> Result<ImportSummary, String> {
    let manifest = parse(path)?;
    let (loader, loader_version) = loader(&manifest);
    let override_count = count_overrides(path, &manifest.overrides)?;
    Ok(ImportSummary {
        name: manifest.name,
        pack_version: manifest.version,
        game_version: manifest.minecraft.version,
        loader,
        loader_version,
        mod_count: manifest.files.iter().filter(|file| file.required).count(),
        override_count,
        format: PackFormat::CurseForge,
        archive_path: path.to_owned(),
        source: None,
    })
}

pub async fn execute_import(
    summary: &ImportSummary,
    manager: &InstanceManager,
    config: &InstanceConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let minecraft_dir = manager
        .instances_dir
        .join(&config.name)
        .join(crate::storage::MINECRAFT_DIR_NAME);
    let manifest = parse(&summary.archive_path)?;
    download_files(&manifest, &minecraft_dir).await?;
    extract_overrides(&summary.archive_path, &minecraft_dir, &manifest.overrides)?;
    Ok(())
}

async fn download_files(
    manifest: &Manifest,
    minecraft_dir: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let api_key =
        crate::net::curseforge::api_key().ok_or("CurseForge API key is not configured")?;
    let client = crate::net::HttpClient::new();
    let ids = manifest
        .files
        .iter()
        .filter(|file| file.required)
        .map(|file| file.file_id)
        .collect::<Vec<_>>();
    let versions = crate::net::curseforge::fetch_file_versions(&client, api_key, &ids).await?;
    if versions.len() != ids.len() {
        return Err(format!(
            "CurseForge returned {} of {} required pack files",
            versions.len(),
            ids.len()
        )
        .into());
    }
    let mods_dir = minecraft_dir.join("mods");
    tokio::fs::create_dir_all(&mods_dir).await?;
    let total = versions.len();
    progress::set_action(format!("Downloading mods... 0/{total}"));
    let completed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let slots = std::sync::Arc::new(tokio::sync::Semaphore::new(10));
    let mut tasks = tokio::task::JoinSet::new();
    for mut version in versions {
        let client = client.clone();
        let api_key = api_key.to_owned();
        let mods_dir = mods_dir.clone();
        let completed = completed.clone();
        let slots = slots.clone();
        tasks.spawn(async move {
            let _permit = slots.acquire_owned().await?;
            crate::net::curseforge::ensure_download_url(&client, &api_key, &mut version).await?;
            crate::net::modrinth::download_version_file(&client, &version, &mods_dir).await?;
            let finished = completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            progress::set_action(format!("Downloading mods... {finished}/{total}"));
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        });
    }
    while let Some(result) = tasks.join_next().await {
        result??;
    }
    Ok(())
}

pub(super) async fn owned_files(path: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let manifest = parse(path)?;
    let api_key = crate::net::curseforge::api_key()
        .ok_or_else(|| "CurseForge API key is not configured".to_owned())?;
    let ids = manifest
        .files
        .iter()
        .filter(|file| file.required)
        .map(|file| file.file_id)
        .collect::<Vec<_>>();
    let versions =
        crate::net::curseforge::fetch_file_versions(&crate::net::HttpClient::new(), api_key, &ids)
            .await
            .map_err(|error| error.to_string())?;
    if versions.len() != ids.len() {
        return Err(format!(
            "CurseForge returned {} of {} required pack files",
            versions.len(),
            ids.len()
        ));
    }
    versions
        .into_iter()
        .map(|version| {
            version
                .files
                .iter()
                .find(|file| file.primary)
                .or_else(|| version.files.first())
                .map(|file| std::path::PathBuf::from("mods").join(&file.filename))
                .ok_or_else(|| format!("CurseForge file '{}' has no artifact", version.id))
        })
        .collect()
}

fn count_overrides(path: &Path, root: &str) -> Result<usize, String> {
    let root = root.trim_matches('/');
    if root.is_empty() {
        return Ok(0);
    }
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let archive = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
    let prefix = format!("{root}/");
    Ok(archive
        .file_names()
        .filter(|name| name.starts_with(&prefix) && !name.ends_with('/'))
        .count())
}

fn extract_overrides(
    path: &Path,
    minecraft_dir: &Path,
    root: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    progress::set_action("Extracting overrides...");
    let file = std::fs::File::open(path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let root = root.trim_matches('/');
    if root.is_empty() {
        return Ok(());
    }
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let enclosed = entry.enclosed_name().ok_or("Unsafe override path")?;
        let Ok(relative) = enclosed.strip_prefix(root) else {
            continue;
        };
        if relative.as_os_str().is_empty() {
            continue;
        }
        let destination = minecraft_dir.join(relative);
        if entry.is_dir() {
            std::fs::create_dir_all(destination)?;
            continue;
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut destination = std::fs::File::create(destination)?;
        std::io::copy(&mut entry, &mut destination)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/import/curseforge.rs"]
mod tests;
