// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// curseforge api client. responses are mapped into the same app-facing
// project/version types used by discovery, so the tui stays provider-neutral.

use serde::Deserialize;

use crate::instance::{ContentKind, ModLoader};
use crate::net::modrinth::{
    DependencyType, DiscoveryProject, DiscoveryResults, ProjectInfo, VersionDependency,
    VersionFile, VersionInfo, VersionType, url_encode,
};
use crate::net::{HttpClient, NetError};

const API_BASE: &str = "https://api.curseforge.com/v1";
const MINECRAFT_GAME_ID: u32 = 432;
const MODS_CLASS_ID: u32 = 6;
const RESOURCE_PACKS_CLASS_ID: u32 = 12;
const SHADERS_CLASS_ID: u32 = 6552;
const DATA_PACKS_CLASS_ID: u32 = 6945;
pub const MODPACKS_CLASS_ID: u32 = 4471;

pub fn api_key() -> Option<&'static str> {
    option_env!("CURSEFORGE_API_KEY")
        .map(str::trim)
        .filter(|key| !key.is_empty())
}

#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    data: T,
    #[serde(default)]
    pagination: Pagination,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pagination {
    total_count: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Mod {
    id: u64,
    #[serde(default)]
    class_id: u32,
    name: String,
    slug: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    download_count: u64,
    allow_mod_distribution: Option<bool>,
    logo: Option<Logo>,
    #[serde(default)]
    categories: Vec<Category>,
}

#[derive(Debug, Deserialize)]
struct Category {
    name: String,
    slug: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Logo {
    thumbnail_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct File {
    id: u64,
    mod_id: u64,
    display_name: String,
    file_name: String,
    #[serde(default)]
    file_date: String,
    #[serde(default)]
    file_length: u64,
    download_url: Option<String>,
    #[serde(default)]
    game_versions: Vec<String>,
    #[serde(default = "default_release_type")]
    release_type: u8,
    #[serde(default)]
    dependencies: Vec<FileDependency>,
    #[serde(default)]
    hashes: Vec<FileHash>,
}

fn default_release_type() -> u8 {
    1
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileDependency {
    mod_id: u64,
    relation_type: u8,
}

#[derive(Debug, Deserialize)]
struct FileHash {
    value: String,
    algo: u8,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FingerprintResponse {
    exact_matches: Vec<FingerprintMatch>,
}

#[derive(Debug, Deserialize)]
struct FingerprintMatch {
    id: u32,
    file: File,
}

#[derive(Debug, serde::Serialize)]
struct FingerprintRequest<'a> {
    fingerprints: &'a [u32],
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct FilesRequest<'a> {
    file_ids: &'a [u64],
}

async fn get<T: serde::de::DeserializeOwned>(
    client: &HttpClient,
    api_key: &str,
    url: &str,
) -> Result<T, NetError> {
    let response = client
        .inner()
        .get(url)
        .header("x-api-key", api_key)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(NetError::StatusError {
            status: response.status().as_u16(),
            url: url.to_owned(),
        });
    }
    Ok(response.json().await?)
}

async fn post<B: serde::Serialize + ?Sized, T: serde::de::DeserializeOwned>(
    client: &HttpClient,
    api_key: &str,
    url: &str,
    body: &B,
) -> Result<T, NetError> {
    let response = client
        .inner()
        .post(url)
        .header("x-api-key", api_key)
        .json(body)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(NetError::StatusError {
            status: response.status().as_u16(),
            url: url.to_owned(),
        });
    }
    Ok(response.json().await?)
}

#[allow(clippy::too_many_arguments)]
pub async fn search_discovery(
    client: &HttpClient,
    api_key: &str,
    kind: ContentKind,
    query: &str,
    game_version: &str,
    loader: ModLoader,
    offset: usize,
    limit: usize,
) -> Result<DiscoveryResults, NetError> {
    search(
        client,
        api_key,
        class_id(kind),
        query,
        game_version,
        (kind == ContentKind::Mod).then_some(loader),
        offset,
        limit,
    )
    .await
}

pub async fn search_modpacks(
    client: &HttpClient,
    api_key: &str,
    query: &str,
    offset: usize,
    limit: usize,
) -> Result<DiscoveryResults, NetError> {
    search(
        client,
        api_key,
        MODPACKS_CLASS_ID,
        query,
        "",
        None,
        offset,
        limit,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn search(
    client: &HttpClient,
    api_key: &str,
    class_id: u32,
    query: &str,
    game_version: &str,
    loader: Option<ModLoader>,
    offset: usize,
    limit: usize,
) -> Result<DiscoveryResults, NetError> {
    let loader = loader.and_then(loader_type);
    let mut projects = Vec::new();
    let mut received = 0;
    let mut total_hits = 0;
    while received < limit {
        let page_size = (limit - received).min(50);
        let mut params = vec![
            format!("gameId={MINECRAFT_GAME_ID}"),
            format!("classId={class_id}"),
            format!("index={}", offset + received),
            format!("pageSize={page_size}"),
            "sortField=6".to_owned(),
            "sortOrder=desc".to_owned(),
        ];
        if !query.trim().is_empty() {
            params.push(format!("searchFilter={}", url_encode(query.trim())));
        }
        if !game_version.is_empty() {
            params.push(format!("gameVersion={}", url_encode(game_version)));
        }
        if let Some(loader) = loader {
            params.push(format!("modLoaderType={loader}"));
        }
        let response: ApiResponse<Vec<Mod>> = get(
            client,
            api_key,
            &format!("{API_BASE}/mods/search?{}", params.join("&")),
        )
        .await?;
        total_hits = response.pagination.total_count;
        let page_received = response.data.len();
        received += page_received;
        projects.extend(response.data);
        if page_received < page_size {
            break;
        }
    }
    Ok(DiscoveryResults {
        received,
        total_hits,
        projects: projects.into_iter().filter_map(discovery_project).collect(),
    })
}

fn discovery_project(project: Mod) -> Option<DiscoveryProject> {
    project
        .allow_mod_distribution
        .unwrap_or(true)
        .then(|| DiscoveryProject {
            id: project.id.to_string(),
            slug: project.slug,
            title: project.name,
            description: project.summary,
            downloads: project.download_count,
            icon_url: project
                .logo
                .map(|logo| logo.thumbnail_url)
                .filter(|url| !url.trim().is_empty()),
            icon_bytes: None,
        })
}

pub async fn fetch_project(
    client: &HttpClient,
    api_key: &str,
    project_id: &str,
) -> Result<ProjectInfo, NetError> {
    let project: ApiResponse<Mod> = get(
        client,
        api_key,
        &format!("{API_BASE}/mods/{}", url_encode(project_id)),
    )
    .await?;
    let description: ApiResponse<String> = get(
        client,
        api_key,
        &format!(
            "{API_BASE}/mods/{}/description?raw=true",
            url_encode(project_id)
        ),
    )
    .await?;
    Ok(project_info(project.data, description.data))
}

fn project_info(project: Mod, body: String) -> ProjectInfo {
    ProjectInfo {
        id: project.id.to_string(),
        slug: project.slug,
        title: project.name,
        description: project.summary,
        body,
        icon_url: project.logo.map(|logo| logo.thumbnail_url),
        categories: project
            .categories
            .into_iter()
            .map(|category| {
                if category.slug.is_empty() {
                    category.name
                } else {
                    category.slug
                }
            })
            .collect(),
        additional_categories: Vec::new(),
        project_type: match project.class_id {
            MODS_CLASS_ID => "mod",
            RESOURCE_PACKS_CLASS_ID => "resourcepack",
            SHADERS_CLASS_ID => "shader",
            DATA_PACKS_CLASS_ID => "datapack",
            _ => "",
        }
        .to_owned(),
        loaders: Vec::new(),
    }
}

pub async fn fetch_versions(
    client: &HttpClient,
    api_key: &str,
    project_id: &str,
    game_version: &str,
    loader: Option<ModLoader>,
) -> Result<Vec<VersionInfo>, NetError> {
    fetch_versions_from(client, api_key, API_BASE, project_id, game_version, loader).await
}

async fn fetch_versions_from(
    client: &HttpClient,
    api_key: &str,
    api_base: &str,
    project_id: &str,
    game_version: &str,
    loader: Option<ModLoader>,
) -> Result<Vec<VersionInfo>, NetError> {
    let loader = loader.and_then(loader_type);
    let mut files = Vec::new();
    loop {
        let mut params = vec![format!("index={}", files.len()), "pageSize=50".to_owned()];
        if !game_version.is_empty() {
            params.push(format!("gameVersion={}", url_encode(game_version)));
        }
        if let Some(loader) = loader {
            params.push(format!("modLoaderType={loader}"));
        }
        let response: ApiResponse<Vec<File>> = get(
            client,
            api_key,
            &format!(
                "{api_base}/mods/{}/files?{}",
                url_encode(project_id),
                params.join("&")
            ),
        )
        .await?;
        let total_count = response.pagination.total_count.min(10_000);
        let received = response.data.len();
        files.extend(response.data);
        if received < 50 || files.len() >= total_count {
            break;
        }
    }
    Ok(files.into_iter().map(version_info).collect())
}

pub async fn fetch_file_versions(
    client: &HttpClient,
    api_key: &str,
    file_ids: &[u64],
) -> Result<Vec<VersionInfo>, NetError> {
    let mut versions = Vec::with_capacity(file_ids.len());
    for ids in file_ids.chunks(50) {
        let response: ApiResponse<Vec<File>> = post(
            client,
            api_key,
            &format!("{API_BASE}/mods/files"),
            &FilesRequest { file_ids: ids },
        )
        .await?;
        versions.extend(response.data.into_iter().map(version_info));
    }
    Ok(versions)
}

pub async fn resolve_fingerprints(
    client: &HttpClient,
    api_key: &str,
    fingerprints: &[u32],
) -> Result<Vec<(u32, String, String)>, NetError> {
    if fingerprints.is_empty() {
        return Ok(Vec::new());
    }
    let response: ApiResponse<FingerprintResponse> = post(
        client,
        api_key,
        &format!("{API_BASE}/fingerprints/{MINECRAFT_GAME_ID}"),
        &FingerprintRequest { fingerprints },
    )
    .await?;
    Ok(response
        .data
        .exact_matches
        .into_iter()
        .map(|matched| {
            (
                matched.id,
                matched.file.mod_id.to_string(),
                matched.file.id.to_string(),
            )
        })
        .collect())
}

pub async fn ensure_download_url(
    client: &HttpClient,
    api_key: &str,
    version: &mut VersionInfo,
) -> Result<(), NetError> {
    ensure_download_url_from(client, api_key, API_BASE, version).await
}

async fn ensure_download_url_from(
    client: &HttpClient,
    api_key: &str,
    api_base: &str,
    version: &mut VersionInfo,
) -> Result<(), NetError> {
    let Some(file) = version.files.first_mut() else {
        return Err(NetError::Parse("No files in version".to_owned()));
    };
    if !file.url.is_empty() {
        return Ok(());
    }
    let response: ApiResponse<String> = get(
        client,
        api_key,
        &format!(
            "{api_base}/mods/{}/files/{}/download-url",
            url_encode(&version.project_id),
            url_encode(&version.id)
        ),
    )
    .await
    .map_err(|error| match error {
        NetError::StatusError { status: 403, .. } => NetError::Parse(
            "CurseForge does not permit this file to be downloaded by third-party launchers; use the CurseForge website or app"
                .to_owned(),
        ),
        error => error,
    })?;
    file.url = response.data;
    Ok(())
}

fn version_info(file: File) -> VersionInfo {
    let loaders = file
        .game_versions
        .iter()
        .filter_map(|version| match version.to_ascii_lowercase().as_str() {
            "fabric" | "forge" | "neoforge" | "quilt" => Some(version.to_ascii_lowercase()),
            _ => None,
        })
        .collect();
    let hashes = file
        .hashes
        .into_iter()
        .filter_map(|hash| match hash.algo {
            1 => Some(("sha1".to_owned(), hash.value)),
            2 => Some(("md5".to_owned(), hash.value)),
            _ => None,
        })
        .collect();
    VersionInfo {
        id: file.id.to_string(),
        project_id: file.mod_id.to_string(),
        name: file.display_name.clone(),
        version_number: file.display_name,
        game_versions: file.game_versions,
        loaders,
        version_type: match file.release_type {
            1 => VersionType::Release,
            2 => VersionType::Beta,
            3 => VersionType::Alpha,
            _ => VersionType::Unknown,
        },
        dependencies: file
            .dependencies
            .into_iter()
            .map(|dependency| VersionDependency {
                version_id: None,
                project_id: Some(dependency.mod_id.to_string()),
                file_name: None,
                dependency_type: match dependency.relation_type {
                    2 => DependencyType::Optional,
                    3 => DependencyType::Required,
                    5 => DependencyType::Incompatible,
                    1 | 6 => DependencyType::Embedded,
                    _ => DependencyType::Unknown,
                },
            })
            .collect(),
        date_published: file.file_date,
        files: vec![VersionFile {
            url: file.download_url.unwrap_or_default(),
            filename: file.file_name,
            size: file.file_length,
            primary: true,
            hashes,
        }],
    }
}

fn class_id(kind: ContentKind) -> u32 {
    match kind {
        ContentKind::Mod => MODS_CLASS_ID,
        ContentKind::ResourcePack => RESOURCE_PACKS_CLASS_ID,
        ContentKind::Shader => SHADERS_CLASS_ID,
        ContentKind::DataPack => DATA_PACKS_CLASS_ID,
    }
}

fn loader_type(loader: ModLoader) -> Option<u8> {
    match loader {
        ModLoader::Vanilla => None,
        ModLoader::Forge => Some(1),
        ModLoader::Fabric => Some(4),
        ModLoader::Quilt => Some(5),
        ModLoader::NeoForge => Some(6),
    }
}

#[cfg(test)]
#[path = "tests/curseforge.rs"]
mod tests;
