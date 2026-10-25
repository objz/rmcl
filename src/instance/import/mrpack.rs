// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::feedback::progress;
use crate::instance::content::manifest::{
    ContentFileRecord, ContentKind, ContentManifest, ProviderProject, Resolution, fingerprint,
};
use crate::instance::manager::InstanceManager;
use crate::instance::models::ModLoader;
use crate::storage::InstancePaths;

use super::{ImportSummary, PackFormat};

#[derive(Debug, Clone, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MrpackFile {
    pub path: String,
    #[serde(default)]
    pub hashes: HashMap<String, String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub downloads: Vec<String>,
    #[serde(rename = "fileSize")]
    pub file_size: u64,
}

impl MrpackIndex {
    fn client_files(&self) -> impl Iterator<Item = &MrpackFile> {
        self.files.iter().filter(|file| {
            file.env
                .get("client")
                .is_none_or(|support| support != "unsupported")
        })
    }
}

pub fn parse_mrpack(path: &Path) -> Result<MrpackIndex, String> {
    tracing::debug!("Parsing .mrpack manifest from {}", path.display());
    let file = std::fs::File::open(path).map_err(|e| format!("Cannot open .mrpack: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("Invalid ZIP: {e}"))?;
    let entry = archive
        .by_name("modrinth.index.json")
        .map_err(|_| "Missing modrinth.index.json in .mrpack".to_string())?;
    let raw = super::read_pack_manifest(entry)?;
    let index: MrpackIndex =
        serde_json::from_slice(&raw).map_err(|e| format!("Invalid manifest JSON: {e}"))?;
    if index.format_version != 1 || index.game != "minecraft" {
        return Err(format!(
            "Unsupported .mrpack format {} for game '{}'",
            index.format_version, index.game
        ));
    }
    tracing::debug!(
        "Parsed .mrpack '{}' version_id={} files={} deps={}",
        index.name,
        index.version_id,
        index.files.len(),
        index.dependencies.len()
    );
    Ok(index)
}

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

    let override_count = super::override_files(path, &["overrides", "client-overrides"])?.len();
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
        mod_count: index.client_files().count(),
        override_count,
        format: PackFormat::Mrpack,
        archive_path: path.to_path_buf(),
        source: None,
    })
}

pub async fn execute_import(
    summary: &ImportSummary,
    manager: &InstanceManager,
    config: &crate::instance::InstanceConfig,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    crate::instance::manager::validate_name(&config.name)?;
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

async fn download_mod_files(
    index: &MrpackIndex,
    minecraft_dir: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let client = crate::net::HttpClient::new();
    let total = index.client_files().count();
    let completed = Arc::new(AtomicUsize::new(0));
    tracing::debug!(
        "Downloading {} file(s) from .mrpack '{}' into {}",
        total,
        index.name,
        minecraft_dir.display()
    );

    progress::set_action(format!("Downloading mods... 0/{total}"));

    let mut tasks = tokio::task::JoinSet::new();

    for file in index.client_files() {
        // Keep at most ten downloads in flight to avoid provider rate limits.
        if tasks.len() == 10
            && let Some(result) = tasks.join_next().await
        {
            result??;
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
        result??;
    }

    Ok(())
}

fn safe_mrpack_path(path: &str) -> Result<PathBuf, crate::net::NetError> {
    super::portable_pack_path(path)
        .map(PathBuf::from)
        .map_err(|error| {
            crate::net::NetError::Parse(format!("Unsafe .mrpack file path '{path}': {error}"))
        })
}

pub(super) fn owned_files(path: &Path) -> Result<Vec<PathBuf>, String> {
    let index = parse_mrpack(path)?;
    let mut files: Vec<_> = index
        .client_files()
        .map(|file| safe_mrpack_path(&file.path).map_err(|error| error.to_string()))
        .collect::<Result<_, _>>()?;
    files.extend(super::override_files(
        path,
        &["overrides", "client-overrides"],
    )?);
    Ok(files)
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
    for file in index.client_files() {
        let relative_path = safe_mrpack_path(&file.path)?;
        let Some(kind) = mrpack_content_kind(&relative_path) else {
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
        let file_fingerprint = fingerprint(&paths.minecraft().join(&relative_path))?;
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
            relative_path,
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
    progress::set_action("Extracting overrides...".to_string());
    progress::set_sub_action(String::new());

    super::archive::extract_overrides(
        mrpack_path,
        minecraft_dir,
        &["overrides", "client-overrides"],
    )
}

#[cfg(test)]
#[path = "../tests/import/mrpack.rs"]
mod tests;
