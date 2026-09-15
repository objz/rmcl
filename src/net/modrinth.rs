// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// modrinth api client and provider file downloads.

use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct ProjectInfo {
    pub id: String,
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub additional_categories: Vec<String>,
    #[serde(default)]
    pub project_type: String,
    #[serde(default)]
    pub loaders: Vec<String>,
}

impl ProjectInfo {
    pub fn is_library_only(&self) -> bool {
        let mut categories = self
            .categories
            .iter()
            .chain(self.additional_categories.iter())
            .peekable();
        categories.peek().is_some()
            && categories.all(|category| {
                matches!(
                    category
                        .chars()
                        .filter(|character| character.is_alphanumeric())
                        .flat_map(char::to_lowercase)
                        .collect::<String>()
                        .as_str(),
                    "library" | "libraries" | "apiandlibrary" | "libraryapi"
                )
            })
    }
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct VersionInfo {
    pub id: String,
    #[serde(default)]
    pub project_id: String,
    pub name: String,
    pub version_number: String,
    pub game_versions: Vec<String>,
    pub loaders: Vec<String>,
    #[serde(default)]
    pub version_type: VersionType,
    #[serde(default)]
    pub dependencies: Vec<VersionDependency>,
    #[serde(default)]
    pub date_published: String,
    pub files: Vec<VersionFile>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VersionType {
    #[default]
    Release,
    Beta,
    Alpha,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, serde::Serialize)]
pub struct VersionDependency {
    #[serde(default)]
    pub version_id: Option<String>,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub file_name: Option<String>,
    pub dependency_type: DependencyType,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DependencyType {
    #[default]
    Required,
    Optional,
    Incompatible,
    Embedded,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct VersionFile {
    pub url: String,
    pub filename: String,
    pub size: u64,
    pub primary: bool,
    #[serde(default)]
    pub hashes: HashMap<String, String>,
}

use crate::instance::{ContentKind, ModLoader};

pub type DiscoveryKind = ContentKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryProject {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub downloads: u64,
    pub icon_url: Option<String>,
    pub icon_bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryResults {
    pub projects: Vec<DiscoveryProject>,
    pub received: usize,
    pub total_hits: usize,
}

#[derive(Debug, Deserialize)]
struct DiscoverySearchResponse {
    hits: Vec<DiscoverySearchHit>,
    #[serde(default)]
    total_hits: i64,
}

#[derive(Debug, Deserialize)]
struct DiscoverySearchHit {
    project_id: String,
    slug: String,
    title: String,
    description: String,
    #[serde(default)]
    downloads: i64,
    #[serde(default)]
    icon_url: Option<String>,
}

impl From<DiscoverySearchHit> for DiscoveryProject {
    fn from(project: DiscoverySearchHit) -> Self {
        Self {
            id: project.project_id,
            slug: project.slug,
            title: project.title,
            description: project.description,
            downloads: project.downloads.max(0) as u64,
            icon_url: project.icon_url.filter(|url| !url.trim().is_empty()),
            icon_bytes: None,
        }
    }
}

pub async fn search_discovery(
    client: &crate::net::HttpClient,
    kind: ContentKind,
    query: &str,
    game_version: &str,
    loader: ModLoader,
    offset: usize,
    limit: usize,
) -> Result<DiscoveryResults, crate::net::NetError> {
    let facets = discovery_facets(kind, game_version, loader);
    let query = query.trim();
    let index = if query.is_empty() {
        "downloads"
    } else {
        "relevance"
    };
    let url = format!(
        "{API_BASE}/search?query={}&facets={}&index={index}&offset={offset}&limit={limit}",
        url_encode(query),
        url_encode(&facets),
    );
    let results: DiscoverySearchResponse = client.get_json(&url).await?;
    let received = results.hits.len();

    Ok(DiscoveryResults {
        received,
        total_hits: results.total_hits.max(0) as usize,
        projects: results
            .hits
            .into_iter()
            .map(DiscoveryProject::from)
            .collect(),
    })
}

pub async fn search_modpacks(
    client: &crate::net::HttpClient,
    query: &str,
    offset: usize,
    limit: usize,
) -> Result<DiscoveryResults, crate::net::NetError> {
    let query = query.trim();
    let index = if query.is_empty() {
        "downloads"
    } else {
        "relevance"
    };
    let facets = r#"[["project_type:modpack"]]"#;
    let url = format!(
        "{API_BASE}/search?query={}&facets={}&index={index}&offset={offset}&limit={limit}",
        url_encode(query),
        url_encode(facets),
    );
    let results: DiscoverySearchResponse = client.get_json(&url).await?;
    let received = results.hits.len();
    Ok(DiscoveryResults {
        received,
        total_hits: results.total_hits.max(0) as usize,
        projects: results
            .hits
            .into_iter()
            .map(DiscoveryProject::from)
            .collect(),
    })
}

fn discovery_facets(kind: ContentKind, game_version: &str, loader: ModLoader) -> String {
    let mut facets = vec![vec![project_type_facet(kind)]];
    if !game_version.is_empty() {
        facets.push(vec![format!("versions:{game_version}")]);
    }
    if kind == ContentKind::Mod
        && let Some(loader) = loader_facet(loader)
    {
        facets.push(vec![format!("categories:{loader}")]);
    }
    serde_json::to_string(&facets).unwrap_or_else(|_| "[]".to_string())
}

fn project_type_facet(kind: ContentKind) -> String {
    match kind {
        ContentKind::DataPack => "all_project_types:datapack".to_owned(),
        _ => format!("project_type:{}", project_type(kind)),
    }
}

fn project_type(kind: ContentKind) -> &'static str {
    match kind {
        ContentKind::Mod => "mod",
        ContentKind::ResourcePack => "resourcepack",
        ContentKind::Shader => "shader",
        ContentKind::DataPack => "datapack",
    }
}

fn loader_facet(loader: ModLoader) -> Option<&'static str> {
    match loader {
        ModLoader::Vanilla => None,
        ModLoader::Fabric => Some("fabric"),
        ModLoader::Forge => Some("forge"),
        ModLoader::NeoForge => Some("neoforge"),
        ModLoader::Quilt => Some("quilt"),
    }
}

const API_BASE: &str = "https://api.modrinth.com/v2";

// hand-rolled percent encoding because pulling in a crate for RFC 3986
// unreserved chars felt like overkill
pub(crate) fn url_encode(s: &str) -> String {
    use std::fmt::Write;
    let mut encoded = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                write!(encoded, "%{byte:02X}").unwrap();
            }
        }
    }
    encoded
}

pub async fn fetch_project(
    client: &crate::net::HttpClient,
    slug_or_id: &str,
) -> Result<ProjectInfo, crate::net::NetError> {
    let url = format!("{}/project/{}", API_BASE, url_encode(slug_or_id));
    tracing::debug!("Fetching Modrinth project '{}'", slug_or_id);
    let project: ProjectInfo = client.get_json(&url).await?;
    tracing::debug!(
        "Fetched Modrinth project '{}' ({})",
        project.slug,
        project.id
    );
    Ok(project)
}

pub async fn fetch_versions(
    client: &crate::net::HttpClient,
    slug_or_id: &str,
) -> Result<Vec<VersionInfo>, crate::net::NetError> {
    let url = versions_url(API_BASE, slug_or_id);
    tracing::debug!("Fetching Modrinth versions for project '{}'", slug_or_id);
    let versions: Vec<VersionInfo> = client.get_json(&url).await?;
    tracing::debug!(
        "Fetched {} Modrinth version(s) for project '{}'",
        versions.len(),
        slug_or_id
    );
    Ok(versions)
}

fn versions_url(api_base: &str, slug_or_id: &str) -> String {
    format!("{api_base}/project/{}/version", url_encode(slug_or_id))
}

pub async fn fetch_content_versions(
    client: &crate::net::HttpClient,
    project_id: &str,
    kind: ContentKind,
    game_version: &str,
    loader: ModLoader,
) -> Result<Vec<VersionInfo>, crate::net::NetError> {
    let url = content_versions_url(API_BASE, project_id, kind, game_version, loader);
    tracing::debug!(
        "Fetching compatible Modrinth versions for '{}' ({}, {})",
        project_id,
        game_version,
        loader
    );
    client.get_json(&url).await
}

fn content_versions_url(
    api_base: &str,
    project_id: &str,
    kind: ContentKind,
    game_version: &str,
    loader: ModLoader,
) -> String {
    let mut params = vec![
        "include_changelog=false".to_owned(),
        format!(
            "game_versions={}",
            url_encode(&serde_json::to_string(&[game_version]).unwrap_or_default())
        ),
    ];
    if kind == ContentKind::Mod
        && let Some(loader) = loader_facet(loader)
    {
        params.push(format!(
            "loaders={}",
            url_encode(&serde_json::to_string(&[loader]).unwrap_or_default())
        ));
    } else if kind == ContentKind::DataPack {
        params.push(format!(
            "loaders={}",
            url_encode(&serde_json::to_string(&["datapack"]).unwrap_or_default())
        ));
    }
    format!(
        "{api_base}/project/{}/version?{}",
        url_encode(project_id),
        params.join("&")
    )
}

pub fn select_primary_file(version: &VersionInfo) -> Result<&VersionFile, crate::net::NetError> {
    version
        .files
        .iter()
        .find(|file| file.primary)
        .or_else(|| version.files.first())
        .ok_or_else(|| crate::net::NetError::Parse("No files in version".to_owned()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadOutcome {
    Downloaded(std::path::PathBuf),
    SkippedExisting(std::path::PathBuf),
}

pub async fn download_version_file(
    client: &crate::net::HttpClient,
    version: &VersionInfo,
    destination: &std::path::Path,
) -> Result<DownloadOutcome, crate::net::NetError> {
    let file = select_primary_file(version)?;
    validate_path_component(&file.filename, "provider filename")?;
    validate_path_component(&version.id, "provider version id")?;
    let path = destination.join(&file.filename);
    if path.exists() {
        if verify_version_file(&path, file)? {
            return Ok(DownloadOutcome::SkippedExisting(path));
        }
        return Err(crate::net::NetError::Parse(format!(
            "Existing file '{}' does not match the selected provider version",
            path.display()
        )));
    }
    let temporary = destination.join(format!(".{}.{}.rmcl-download", file.filename, version.id));
    if temporary.exists() {
        tokio::fs::remove_file(&temporary).await?;
    }
    let progress =
        crate::feedback::progress::ProgressTask::start(format!("Downloading {}", file.filename));
    crate::net::download_file(client, &file.url, &temporary, |current, total| {
        progress.set_progress(current, total);
    })
    .await?;
    if !verify_version_file(&temporary, file)? {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(crate::net::NetError::Parse(format!(
            "Downloaded file '{}' failed its size or hash verification",
            file.filename
        )));
    }
    tokio::fs::rename(&temporary, &path).await?;
    progress.finish();
    Ok(DownloadOutcome::Downloaded(path))
}

pub async fn download_version_file_for_update(
    client: &crate::net::HttpClient,
    version: &VersionInfo,
    destination: &std::path::Path,
    installed_path: &std::path::Path,
) -> Result<DownloadOutcome, crate::net::NetError> {
    let file = select_primary_file(version)?;
    validate_path_component(&file.filename, "provider filename")?;
    validate_path_component(&version.id, "provider version id")?;
    let target = destination.join(&file.filename);
    if target != installed_path {
        return download_version_file(client, version, destination).await;
    }

    let temporary = destination.join(format!(".{}.{}.rmcl-download", file.filename, version.id));
    let backup = destination.join(format!(".{}.{}.rmcl-backup", file.filename, version.id));
    if backup.exists() {
        if !installed_path.exists() {
            tokio::fs::rename(&backup, installed_path).await?;
        } else {
            tokio::fs::remove_file(&backup).await?;
        }
    }
    if temporary.exists() {
        tokio::fs::remove_file(&temporary).await?;
    }

    let progress =
        crate::feedback::progress::ProgressTask::start(format!("Downloading {}", file.filename));
    crate::net::download_file(client, &file.url, &temporary, |current, total| {
        progress.set_progress(current, total);
    })
    .await?;
    if !verify_version_file(&temporary, file)? {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(crate::net::NetError::Parse(format!(
            "Downloaded file '{}' failed its size or hash verification",
            file.filename
        )));
    }
    replace_installed_file(&temporary, &target, installed_path, &backup).await?;
    progress.finish();
    Ok(DownloadOutcome::Downloaded(target))
}

fn validate_path_component(value: &str, label: &str) -> Result<(), crate::net::NetError> {
    let mut components = std::path::Path::new(value).components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(crate::net::NetError::Parse(format!(
            "Invalid {label} '{value}'"
        )));
    }
    Ok(())
}

fn verify_version_file(
    path: &std::path::Path,
    expected: &VersionFile,
) -> Result<bool, crate::net::NetError> {
    let fingerprint = crate::instance::content::manifest::fingerprint(path)?;
    if expected.size > 0 && fingerprint.size != expected.size {
        return Ok(false);
    }
    for algorithm in ["sha512", "sha1"] {
        if let Some(expected_hash) = expected.hashes.get(algorithm)
            && fingerprint.hash(algorithm) != Some(expected_hash.as_str())
        {
            return Ok(false);
        }
    }
    Ok(true)
}

async fn replace_installed_file(
    temporary: &std::path::Path,
    target: &std::path::Path,
    installed_path: &std::path::Path,
    backup: &std::path::Path,
) -> Result<(), crate::net::NetError> {
    tokio::fs::rename(installed_path, &backup).await?;
    if let Err(error) = tokio::fs::rename(&temporary, &target).await {
        let _ = tokio::fs::rename(&backup, installed_path).await;
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error.into());
    }
    if let Err(error) = tokio::fs::remove_file(&backup).await {
        tracing::warn!("Failed to remove provider update backup: {error}");
    }
    Ok(())
}

pub async fn fetch_version(
    client: &crate::net::HttpClient,
    version_id: &str,
) -> Result<VersionInfo, crate::net::NetError> {
    let url = format!("{}/version/{}", API_BASE, url_encode(version_id));
    tracing::debug!("Fetching Modrinth version '{}'", version_id);
    let version: VersionInfo = client.get_json(&url).await?;
    tracing::debug!(
        "Fetched Modrinth version '{}' ({}) with {} file(s)",
        version.name,
        version.id,
        version.files.len()
    );
    Ok(version)
}

#[derive(Debug, serde::Serialize)]
struct VersionFilesRequest<'a> {
    hashes: &'a [String],
    algorithm: &'a str,
}

pub async fn resolve_version_files(
    client: &crate::net::HttpClient,
    hashes: &[String],
    algorithm: &str,
) -> Result<HashMap<String, VersionInfo>, crate::net::NetError> {
    if hashes.is_empty() {
        return Ok(HashMap::new());
    }
    client
        .post_json(
            &format!("{API_BASE}/version_files"),
            &VersionFilesRequest { hashes, algorithm },
        )
        .await
}

// grabs the primary file from a version, falling back to the first file
// if none is marked primary (some projects are sloppy about that).
// routes through the shared verified download (temp file + size/hash check
// + rename) so a truncated or corrupted .mrpack is never left in place.
pub async fn download_mrpack(
    client: &crate::net::HttpClient,
    version: &VersionInfo,
    dest: &std::path::Path,
) -> Result<std::path::PathBuf, crate::net::NetError> {
    match download_version_file(client, version, dest).await? {
        DownloadOutcome::Downloaded(path) | DownloadOutcome::SkippedExisting(path) => Ok(path),
    }
}

#[cfg(test)]
#[path = "tests/modrinth.rs"]
mod tests;
