// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// modrinth .mrpack import: parse the manifest, download all the mods,
// and extract config/resource overrides from the zip

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::feedback::progress;
use crate::instance::content::manifest::{
    ContentFileRecord, ContentKind, ContentManifest, ProviderProject, Resolution, fingerprint,
};
use crate::instance::manager::InstanceManager;
use crate::instance::models::ModLoader;
use crate::storage::InstancePaths;

use super::{ImportSummary, PackFormat};

#[derive(Debug, Clone, Deserialize)]
pub struct MrpackIndex {
    #[serde(rename = "formatVersion")]
    pub format_version: u32,
    pub game: String,
    #[serde(rename = "versionId")]
    pub version_id: String,
    pub name: String,
    #[serde(default)]
    pub dependencies: HashMap<String, String>,
    #[serde(default)]
    pub files: Vec<MrpackFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MrpackFile {
    pub path: String,
    #[serde(default)]
    pub hashes: HashMap<String, String>,
    pub downloads: Vec<String>,
    #[serde(rename = "fileSize")]
    pub file_size: u64,
}

// .mrpack is just a zip with modrinth.index.json at the root
pub fn parse_mrpack(path: &Path) -> Result<MrpackIndex, String> {
    tracing::debug!("Parsing .mrpack manifest from {}", path.display());
    let file = std::fs::File::open(path).map_err(|e| format!("Cannot open .mrpack: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("Invalid ZIP: {e}"))?;
    let entry = archive
        .by_name("modrinth.index.json")
        .map_err(|_| "Missing modrinth.index.json in .mrpack".to_string())?;
    let index: MrpackIndex =
        serde_json::from_reader(entry).map_err(|e| format!("Invalid manifest JSON: {e}"))?;
    tracing::debug!(
        "Parsed .mrpack '{}' version_id={} files={} deps={}",
        index.name,
        index.version_id,
        index.files.len(),
        index.dependencies.len()
    );
    Ok(index)
}

// mrpack dependencies use keys like "fabric-loader", "forge", etc.
// checks in priority order and returns the first match.
pub fn loader_from_dependencies(
    deps: &HashMap<String, String>,
) -> (Option<ModLoader>, Option<String>) {
    let loaders = [
        ("fabric-loader", ModLoader::Fabric),
        ("forge", ModLoader::Forge),
        ("neoforge", ModLoader::NeoForge),
        ("quilt-loader", ModLoader::Quilt),
    ];
    for (key, loader) in &loaders {
        if let Some(version) = deps.get(*key) {
            tracing::trace!(
                "Resolved Modrinth loader dependency {}={} as {}",
                key,
                version,
                loader
            );
            return (Some(*loader), Some(version.clone()));
        }
    }
    tracing::trace!("No Modrinth loader dependency found; treating pack as vanilla");
    (None, None)
}

pub fn game_version_from_dependencies(deps: &HashMap<String, String>) -> Option<String> {
    deps.get("minecraft").cloned()
}

pub fn build_summary(path: &Path) -> Result<ImportSummary, String> {
    let index = parse_mrpack(path)?;
    tracing::debug!(
        "Parsed .mrpack '{}' version_id={} files={} deps={}",
        index.name,
        index.version_id,
        index.files.len(),
        index.dependencies.len()
    );

    let game_version = game_version_from_dependencies(&index.dependencies)
        .ok_or_else(|| "Manifest missing minecraft dependency".to_string())?;

    let (loader_opt, loader_version) = loader_from_dependencies(&index.dependencies);
    let loader = loader_opt.unwrap_or(ModLoader::Vanilla);

    let override_count = count_overrides(path).unwrap_or(0);
    tracing::trace!(
        ".mrpack summary: game_version={} loader={:?} loader_version={:?} overrides={}",
        game_version,
        loader,
        loader_version,
        override_count
    );

    Ok(ImportSummary {
        name: index.name.clone(),
        pack_version: index.version_id.clone(),
        game_version,
        loader,
        loader_version,
        mod_count: index.files.len(),
        override_count,
        format: PackFormat::Mrpack,
        archive_path: path.to_path_buf(),
        source: None,
    })
}

// peek into the zip to count files under overrides/ and client-overrides/
fn count_overrides(mrpack_path: &Path) -> Result<usize, String> {
    let file = std::fs::File::open(mrpack_path).map_err(|e| e.to_string())?;
    let archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let count = archive
        .file_names()
        .filter(|name| {
            (name.starts_with("overrides/") || name.starts_with("client-overrides/"))
                && !name.ends_with('/')
        })
        .count();
    Ok(count)
}

pub async fn execute_import(
    summary: &ImportSummary,
    manager: &InstanceManager,
    config: &crate::instance::InstanceConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing::info!(
        "Importing Modrinth pack '{}' as instance '{}'",
        summary.name,
        config.name
    );

    let minecraft_dir = manager
        .instances_dir
        .join(&config.name)
        .join(crate::storage::MINECRAFT_DIR_NAME);

    let index = parse_mrpack(&summary.archive_path)
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;
    download_mod_files(&index, &minecraft_dir).await?;
    extract_overrides(&summary.archive_path, &minecraft_dir)?;
    seed_content_manifest(
        &index,
        &InstancePaths::new(manager.instances_dir.join(&config.name)),
    )?;

    tracing::info!(
        "Imported Modrinth pack '{}' as '{}'",
        summary.name,
        config.name
    );
    Ok(())
}

// downloads all mod files listed in the mrpack index, capped at 10 concurrent
// downloads to avoid getting rate-limited into oblivion
async fn download_mod_files(
    index: &MrpackIndex,
    minecraft_dir: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let client = crate::net::HttpClient::new();
    let total = index.files.len();
    let completed = Arc::new(AtomicUsize::new(0));
    tracing::debug!(
        "Downloading {} file(s) from .mrpack '{}' into {}",
        total,
        index.name,
        minecraft_dir.display()
    );

    progress::set_action(format!("Downloading mods... 0/{total}"));

    // bounded concurrency via manual JoinSet draining: seed with max_concurrent
    // tasks, then spawn a new one each time one finishes
    let mut tasks = tokio::task::JoinSet::new();
    let max_concurrent = 10;
    let mut file_iter = index.files.iter();

    for _ in 0..max_concurrent {
        if let Some(file) = file_iter.next() {
            let client = client.clone();
            let relative = safe_mrpack_path(&file.path)?;
            let dest = minecraft_dir.join(relative);
            let url = file.downloads.first().cloned().ok_or_else(|| {
                crate::net::NetError::Parse(format!(
                    ".mrpack file '{}' has no download URL",
                    file.path
                ))
            })?;
            let filename = file
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&file.path)
                .to_string();
            let hashes = file.hashes.clone();
            let file_size = file.file_size;
            let completed = completed.clone();
            tasks.spawn(async move {
                if let Some(parent) = dest.parent()
                    && let Err(error) = tokio::fs::create_dir_all(parent).await
                {
                    return Err(crate::net::NetError::from(error));
                }
                progress::set_sub_action(filename);
                tracing::trace!("Downloading .mrpack file to {}", dest.display());
                crate::net::download_file(&client, &url, &dest, |_, _| {}).await?;
                if !verify_mrpack_file(&dest, file_size, &hashes)? {
                    let _ = tokio::fs::remove_file(&dest).await;
                    return Err(crate::net::NetError::Parse(format!(
                        "Downloaded .mrpack file '{}' failed its size or hash verification",
                        dest.display()
                    )));
                }
                let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                progress::set_action(format!("Downloading mods... {done}/{total}"));
                Ok::<(), crate::net::NetError>(())
            });
        }
    }

    for file in file_iter {
        if let Some(result) = tasks.join_next().await {
            result
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;
        }
        let client = client.clone();
        let relative = safe_mrpack_path(&file.path)?;
        let dest = minecraft_dir.join(relative);
        let url = file.downloads.first().cloned().ok_or_else(|| {
            crate::net::NetError::Parse(format!(".mrpack file '{}' has no download URL", file.path))
        })?;
        let filename = file
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&file.path)
            .to_string();
        let hashes = file.hashes.clone();
        let file_size = file.file_size;
        let completed = completed.clone();
        tasks.spawn(async move {
            if let Some(parent) = dest.parent()
                && let Err(error) = tokio::fs::create_dir_all(parent).await
            {
                return Err(crate::net::NetError::from(error));
            }
            progress::set_sub_action(filename);
            tracing::trace!("Downloading .mrpack file to {}", dest.display());
            crate::net::download_file(&client, &url, &dest, |_, _| {}).await?;
            if !verify_mrpack_file(&dest, file_size, &hashes)? {
                let _ = tokio::fs::remove_file(&dest).await;
                return Err(crate::net::NetError::Parse(format!(
                    "Downloaded .mrpack file '{}' failed its size or hash verification",
                    dest.display()
                )));
            }
            let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
            progress::set_action(format!("Downloading mods... {done}/{total}"));
            Ok::<(), crate::net::NetError>(())
        });
    }

    while let Some(result) = tasks.join_next().await {
        result
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { Box::new(e) })?;
    }

    Ok(())
}

fn safe_mrpack_path(path: &str) -> Result<PathBuf, crate::net::NetError> {
    let path = Path::new(path);
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(crate::net::NetError::Parse(format!(
            "Unsafe .mrpack file path '{}'",
            path.display()
        )));
    }
    Ok(path.to_owned())
}

pub(super) fn owned_files(path: &Path) -> Result<Vec<PathBuf>, String> {
    parse_mrpack(path)?
        .files
        .into_iter()
        .map(|file| safe_mrpack_path(&file.path).map_err(|error| error.to_string()))
        .collect()
}

fn verify_mrpack_file(
    path: &Path,
    expected_size: u64,
    expected_hashes: &HashMap<String, String>,
) -> Result<bool, crate::net::NetError> {
    let fingerprint = fingerprint(path)?;
    if fingerprint.size != expected_size {
        return Ok(false);
    }
    Ok(["sha512", "sha1"].into_iter().all(|algorithm| {
        expected_hashes.get(algorithm).is_none_or(|expected| {
            fingerprint
                .hash(algorithm)
                .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
        })
    }))
}

fn seed_content_manifest(
    index: &MrpackIndex,
    paths: &InstancePaths,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut manifest = ContentManifest::default();
    for file in &index.files {
        let relative_path = Path::new(&file.path);
        let Some(kind) = mrpack_content_kind(relative_path) else {
            continue;
        };
        let Some((project_id, version_id)) = file.downloads.iter().find_map(|url| {
            let path = url.strip_prefix("https://cdn.modrinth.com/data/")?;
            let (project_id, path) = path.split_once("/versions/")?;
            let version_id = path.split('/').next()?;
            (!project_id.is_empty() && !version_id.is_empty())
                .then_some((project_id.to_owned(), version_id.to_owned()))
        }) else {
            continue;
        };
        let Some(expected_sha512) = file.hashes.get("sha512") else {
            continue;
        };
        let file_fingerprint = fingerprint(&paths.minecraft().join(relative_path))?;
        if file_fingerprint
            .hash("sha512")
            .is_none_or(|actual| !actual.eq_ignore_ascii_case(expected_sha512))
        {
            tracing::warn!(
                "Skipping stale Modrinth identity for '{}' because its downloaded hash changed",
                file.path
            );
            continue;
        }
        manifest.upsert(ContentFileRecord {
            relative_path: relative_path.to_owned(),
            kind,
            enabled: true,
            fingerprint: file_fingerprint,
            resolution: Resolution::Resolved {
                project: ProviderProject {
                    provider: "modrinth".to_owned(),
                    project_id,
                    version_id,
                },
            },
            provider_aliases: Vec::new(),
            provider_checks: vec!["modrinth".to_owned()],
            required_dependencies: Vec::new(),
            automatic_dependency: false,
            cleanup_eligible: false,
        });
    }
    let seeded = manifest.files.len();
    manifest.save(&paths.content_manifest())?;
    tracing::debug!(
        "Seeded {} exact Modrinth content record(s) from .mrpack '{}'",
        seeded,
        index.name
    );
    Ok(())
}

fn mrpack_content_kind(path: &Path) -> Option<ContentKind> {
    use std::path::Component;

    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    let directory = path.components().next()?.as_os_str().to_str()?;
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    match (directory, name.as_str()) {
        ("mods", name) if name.ends_with(".jar") => Some(ContentKind::Mod),
        ("resourcepacks", name) if name.ends_with(".zip") => Some(ContentKind::ResourcePack),
        ("shaderpacks", name) if name.ends_with(".zip") => Some(ContentKind::Shader),
        _ => None,
    }
}

fn extract_overrides(
    mrpack_path: &Path,
    minecraft_dir: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use std::io::Read;

    progress::set_action("Extracting overrides...".to_string());
    progress::set_sub_action(String::new());

    let file = std::fs::File::open(mrpack_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let mut extracted = 0usize;
    let mut dirs = 0usize;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let entry_name = entry.name().to_string();

        let root = if entry_name.starts_with("overrides/") {
            "overrides"
        } else if entry_name.starts_with("client-overrides/") {
            "client-overrides"
        } else {
            continue;
        };
        let enclosed = entry.enclosed_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Unsafe override path: {entry_name}"),
            )
        })?;
        let relative = enclosed.strip_prefix(root).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Invalid override path: {entry_name}"),
            )
        })?;

        if relative.as_os_str().is_empty() || entry_name.ends_with('/') {
            let dir = minecraft_dir.join(relative);
            std::fs::create_dir_all(dir)?;
            dirs += 1;
            continue;
        }

        let dest = minecraft_dir.join(relative);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut buf = Vec::new();
        entry.read_to_end(&mut buf)?;
        tracing::trace!(
            "Extracting .mrpack override {} to {} ({} bytes)",
            entry_name,
            dest.display(),
            buf.len()
        );
        std::fs::write(&dest, &buf)?;
        extracted += 1;
    }

    tracing::debug!(
        "Extracted {} override files and {} directories from {}",
        extracted,
        dirs,
        mrpack_path.display()
    );
    Ok(())
}

#[cfg(test)]
#[path = "../tests/import/mrpack.rs"]
mod tests;
