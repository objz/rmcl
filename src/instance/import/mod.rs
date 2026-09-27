// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// modpack importing: parses user input, detects pack format from zip contents,
// builds a summary, and delegates the actual import to format-specific modules.

pub mod curseforge;
pub mod mmc;
pub mod mrpack;
pub mod refresh;

use std::path::{Path, PathBuf};

use crate::instance::manager::InstanceManager;
use crate::instance::models::ModLoader;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackFormat {
    CurseForge,
    Mrpack,
    Mmc,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImportInput {
    ProjectSlug(String),
    VersionId { slug: String, version_id: String },
    LocalFile(String),
}

// figures out what the user gave us: a modrinth URL, a local pack file,
// or just a project slug. accepts a pretty wide range of inputs so users
// don't have to think about it.
pub fn parse_import_input(input: &str) -> ImportInput {
    let input = input.trim();

    if input.ends_with(".mrpack")
        || input.ends_with(".zip")
        || input.starts_with('/')
        || input.starts_with("~/")
    {
        tracing::debug!("Import input resolved as local file: {}", input);
        return ImportInput::LocalFile(input.to_string());
    }

    if let Some(rest) = input
        .strip_prefix("https://modrinth.com/modpack/")
        .or_else(|| input.strip_prefix("http://modrinth.com/modpack/"))
    {
        let parts: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
        let parsed = match parts.as_slice() {
            [slug, "version", version_id, ..] => ImportInput::VersionId {
                slug: slug.to_string(),
                version_id: version_id.to_string(),
            },
            [slug, ..] => ImportInput::ProjectSlug(slug.to_string()),
            [] => ImportInput::ProjectSlug(String::new()),
        };
        tracing::debug!("Import input resolved as Modrinth URL: {:?}", parsed);
        return parsed;
    }

    tracing::debug!("Import input resolved as Modrinth project slug: {}", input);
    ImportInput::ProjectSlug(input.to_string())
}

#[derive(Debug, Clone)]
pub struct ImportSummary {
    pub name: String,
    pub pack_version: String,
    pub game_version: String,
    pub loader: ModLoader,
    pub loader_version: Option<String>,
    pub mod_count: usize,
    pub override_count: usize,
    pub format: PackFormat,
    pub archive_path: PathBuf,
    pub source: Option<crate::instance::ProviderProject>,
}

// peeks inside a zip to figure out what format it is.
// checks provider manifests first, then mmc-pack.json.
pub fn detect_format(path: &Path) -> Result<PackFormat, String> {
    tracing::debug!("Detecting modpack format for {}", path.display());
    let file =
        std::fs::File::open(path).map_err(|e| format!("Cannot open '{}': {e}", path.display()))?;
    let archive =
        zip::ZipArchive::new(file).map_err(|e| format!("Invalid ZIP '{}': {e}", path.display()))?;

    if archive.file_names().any(|n| n == "modrinth.index.json") {
        tracing::debug!("Detected Modrinth .mrpack archive: {}", path.display());
        return Ok(PackFormat::Mrpack);
    }

    if archive.file_names().any(|name| name == "manifest.json") {
        tracing::debug!("Detected CurseForge archive: {}", path.display());
        return Ok(PackFormat::CurseForge);
    }

    // mmc-pack.json can be at root or one directory deep
    if archive
        .file_names()
        .any(|n| n == "mmc-pack.json" || n.ends_with("/mmc-pack.json"))
    {
        tracing::debug!("Detected MultiMC/Prism archive: {}", path.display());
        return Ok(PackFormat::Mmc);
    }

    tracing::warn!("Unknown modpack archive format: {}", path.display());
    Err(
        "Unknown pack format: no modrinth.index.json, manifest.json, or mmc-pack.json found"
            .to_string(),
    )
}

pub fn build_summary(path: &Path) -> Result<ImportSummary, String> {
    if !path.exists() {
        tracing::warn!("Cannot import missing file {}", path.display());
        return Err(format!("File not found: {}", path.display()));
    }
    let format = detect_format(path)?;
    let summary = match format {
        PackFormat::CurseForge => curseforge::build_summary(path),
        PackFormat::Mrpack => mrpack::build_summary(path),
        PackFormat::Mmc => mmc::build_summary(path),
    }?;
    tracing::debug!(
        "Built import summary for {}: name='{}' format={:?} mc={} loader={} loader_version={:?} mods={} overrides={}",
        path.display(),
        summary.name,
        summary.format,
        summary.game_version,
        summary.loader,
        summary.loader_version,
        summary.mod_count,
        summary.override_count
    );
    Ok(summary)
}

pub fn unique_instance_name(base: &str, instances_dir: &Path) -> String {
    let candidate = base.to_string();
    if !instances_dir.join(&candidate).exists() {
        tracing::trace!("Import instance name '{}' is available", candidate);
        return candidate;
    }
    for n in 2..100 {
        let candidate = format!("{base} ({n})");
        if !instances_dir.join(&candidate).exists() {
            tracing::debug!(
                "Import instance name '{}' collided; using '{}'",
                base,
                candidate
            );
            return candidate;
        }
    }
    tracing::warn!(
        "Import instance name '{}' had many collisions; using fallback suffix",
        base
    );
    format!("{base} (import)")
}

pub async fn execute_import(
    summary: &ImportSummary,
    manager: &InstanceManager,
) -> Result<crate::instance::InstanceConfig, Box<dyn std::error::Error + Send + Sync>> {
    tracing::info!(
        "Executing {:?} import '{}' from {}",
        summary.format,
        summary.name,
        summary.archive_path.display()
    );
    let name = unique_instance_name(&summary.name, &manager.instances_dir);
    crate::feedback::progress::set_action(format!("Importing '{name}'..."));
    crate::feedback::progress::set_sub_action(format!(
        "{} {}",
        summary.game_version, summary.loader
    ));
    let config = manager
        .create(
            &name,
            &summary.game_version,
            summary.loader,
            summary.loader_version.as_deref(),
        )
        .await;
    let mut config = match config {
        Ok(config) => config,
        Err(error) => {
            crate::feedback::progress::clear();
            return Err(Box::new(error));
        }
    };
    let result = async {
        match summary.format {
            PackFormat::CurseForge => curseforge::execute_import(summary, manager, &config).await?,
            PackFormat::Mrpack => mrpack::execute_import(summary, manager, &config).await?,
            PackFormat::Mmc => mmc::execute_import(summary, manager, &config).await?,
        }
        if let Some(source) = &summary.source {
            config.modpack_source = Some(source.clone());
            manager.save(&config)?;
            let state = refresh::PackState {
                source: source.clone(),
                files: owned_files(summary).await?,
            };
            state.save(&crate::storage::InstancePaths::new(
                manager.instances_dir.join(&config.name),
            ))?;
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(config)
    }
    .await;
    crate::feedback::progress::clear();
    if result.is_err() {
        cleanup_failed_import(manager, &name);
    }
    result
}

pub async fn provider_versions(
    source: &crate::instance::ProviderProject,
) -> Result<Vec<crate::net::modrinth::VersionInfo>, crate::net::NetError> {
    let client = crate::net::HttpClient::new();
    match source.provider.as_str() {
        "curseforge" => {
            let api_key = crate::net::curseforge::api_key().ok_or_else(|| {
                crate::net::NetError::Parse("CurseForge API key is not configured".to_owned())
            })?;
            crate::net::curseforge::fetch_versions(&client, api_key, &source.project_id, "", None)
                .await
        }
        "modrinth" => crate::net::modrinth::fetch_versions(&client, &source.project_id).await,
        provider => Err(crate::net::NetError::Parse(format!(
            "Unsupported modpack provider '{provider}'"
        ))),
    }
}

pub async fn download_provider_summary(
    source: &crate::instance::ProviderProject,
    version: &crate::net::modrinth::VersionInfo,
    temporary_dir: &Path,
) -> Result<ImportSummary, crate::net::NetError> {
    tokio::fs::create_dir_all(temporary_dir).await?;
    let registry = crate::instance::content::provider::ProviderRegistry::configured(
        crate::net::HttpClient::new(),
    );
    let provider = registry.get(&source.provider).ok_or_else(|| {
        crate::net::NetError::Parse(format!(
            "{} content provider is unavailable",
            source.provider
        ))
    })?;
    let archive = match provider
        .download_version(version, temporary_dir, None)
        .await?
    {
        crate::net::modrinth::DownloadOutcome::Downloaded(path)
        | crate::net::modrinth::DownloadOutcome::SkippedExisting(path) => path,
    };
    let mut summary = build_summary(&archive).map_err(crate::net::NetError::Parse)?;
    summary.source = Some(crate::instance::ProviderProject {
        provider: source.provider.clone(),
        project_id: source.project_id.clone(),
        version_id: version.id.clone(),
    });
    Ok(summary)
}

async fn owned_files(summary: &ImportSummary) -> Result<Vec<PathBuf>, String> {
    match summary.format {
        PackFormat::Mrpack => mrpack::owned_files(&summary.archive_path),
        PackFormat::CurseForge => curseforge::owned_files(&summary.archive_path).await,
        PackFormat::Mmc => Err("Managed updates are unavailable for MultiMC packs".to_owned()),
    }
}

fn cleanup_failed_import(manager: &InstanceManager, name: &str) {
    let instance_dir = manager.instances_dir.join(name);
    if let Err(error) = std::fs::remove_dir_all(&instance_dir)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(
            "Failed to clean up incomplete imported instance {}: {}",
            instance_dir.display(),
            error
        );
    }
}

#[cfg(test)]
#[path = "../tests/import/format_detection.rs"]
mod tests;
