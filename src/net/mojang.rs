// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;

use super::{HttpClient, NetError, download_file};
use crate::feedback::progress::{clear, set_action, set_progress, set_sub_action};
use crate::launch_profile::{model, system::JavaPlatform};

pub use model::{Artifact, LibraryDownloads, LibraryExtract};

const MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const ASSETS_BASE_URL: &str = "https://resources.download.minecraft.net";
const MAX_CONCURRENT_DOWNLOADS: usize = 10;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VersionManifest {
    pub latest: LatestVersions,
    pub versions: Vec<VersionEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LatestVersions {
    pub release: String,
    pub snapshot: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VersionEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub version_type: String,
    pub url: String,
    pub sha1: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionMeta {
    pub id: String,
    pub main_class: String,
    pub asset_index: AssetIndex,
    pub downloads: VersionDownloads,
    pub libraries: Vec<Library>,
    pub java_version: Option<JavaVersion>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AssetIndex {
    pub id: String,
    pub url: String,
    pub sha1: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VersionDownloads {
    pub client: Download,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Download {
    pub url: String,
    pub sha1: String,
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Library {
    pub name: String,
    pub downloads: LibraryDownloads,
    pub rules: Option<Vec<crate::launch_profile::rules::Rule>>,
    // os name -> natives classifier (e.g. "linux" -> "natives-linux").
    // present on pre-1.13-era libraries whose native code ships in
    // separate per-platform jars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub natives: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extract: Option<LibraryExtract>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersion {
    pub major_version: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AssetIndexContent {
    pub objects: HashMap<String, AssetObject>,
    #[serde(default, rename = "virtual")]
    pub virtual_assets: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

pub async fn fetch_version_manifest(client: &HttpClient) -> Result<VersionManifest, NetError> {
    fetch_version_manifest_from(client, MANIFEST_URL).await
}

pub async fn fetch_version_manifest_from(
    client: &HttpClient,
    url: &str,
) -> Result<VersionManifest, NetError> {
    tracing::debug!("Fetching Mojang version manifest from {}", url);
    let manifest: VersionManifest = client.get_json(url).await?;
    tracing::debug!(
        "Fetched Mojang manifest with {} version(s); latest release={} snapshot={}",
        manifest.versions.len(),
        manifest.latest.release,
        manifest.latest.snapshot
    );
    Ok(manifest)
}

// fetches and parses a version's metadata. also returns the raw response
// bytes so the caller can write the upstream JSON byte-for-byte to disk
// - used by the install path so we don't lose data (e.g. arguments.jvm)
// by re-serializing through our narrow VersionMeta struct.
pub async fn fetch_version_meta_with_raw(
    client: &HttpClient,
    entry: &VersionEntry,
) -> Result<(VersionMeta, Vec<u8>), NetError> {
    tracing::debug!(
        "Fetching Mojang version meta '{}' from {}",
        entry.id,
        entry.url
    );
    let (meta, raw): (VersionMeta, Vec<u8>) =
        client.get_json_with_raw(&entry.url, "version meta").await?;
    if !sha1_matches(&raw, &entry.sha1) || meta.id != entry.id {
        return Err(NetError::Parse(format!(
            "Version metadata for '{}' does not match its manifest",
            entry.id
        )));
    }
    Ok((meta, raw))
}

fn sha1_matches(bytes: &[u8], expected: &str) -> bool {
    use sha1::Digest;
    format!("{:x}", sha1::Sha1::digest(bytes)).eq_ignore_ascii_case(expected)
}

fn sha1_hex(path: &Path) -> Option<String> {
    use sha1::Digest;
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = sha1::Sha1::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => hasher.update(&buffer[..read]),
            Err(_) => return None,
        }
    }
    Some(format!("{:x}", hasher.finalize()))
}

// guards against partial files left by killed downloads: those used to be
// trusted forever on the strength of an exists() check alone. size is
// checked first so truncated files skip the hashing cost.
fn verify_cached(path: &Path, expected_sha1: &str, expected_size: u64) -> bool {
    if !path.exists() {
        return false;
    }
    if expected_size > 0 {
        let actual = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
        if actual != expected_size {
            return false;
        }
    }
    sha1_hex(path).is_some_and(|hash| hash.eq_ignore_ascii_case(expected_sha1))
}

pub async fn download_client_jar(
    client: &HttpClient,
    meta: &VersionMeta,
    meta_dir: &Path,
) -> Result<(), NetError> {
    let jar_path = crate::storage::MetadataPaths::new(meta_dir)
        .versions()
        .join(&meta.id)
        .join(format!("{}.jar", meta.id));

    if verify_cached(
        &jar_path,
        &meta.downloads.client.sha1,
        meta.downloads.client.size,
    ) {
        tracing::info!("Client JAR already cached: {}", meta.id);
        tracing::trace!("Cached client JAR path: {}", jar_path.display());
        return Ok(());
    }

    set_action(format!("Downloading Minecraft {}...", meta.id));
    tracing::info!(
        "Downloading Minecraft client JAR {} to {}",
        meta.id,
        jar_path.display()
    );

    let result = download_file(
        client,
        &meta.downloads.client.url,
        &jar_path,
        |current, total| {
            set_progress(current, total);
        },
    )
    .await;

    clear();

    // a fresh download that doesn't match the manifest is worse than no
    // download: delete it so the next attempt doesn't trust it.
    if result.is_ok()
        && !verify_cached(
            &jar_path,
            &meta.downloads.client.sha1,
            meta.downloads.client.size,
        )
    {
        let _ = tokio::fs::remove_file(&jar_path).await;
        return Err(NetError::Parse(format!(
            "Downloaded client JAR {} failed its sha1 verification",
            meta.id
        )));
    }
    result
}

pub async fn download_libraries(
    client: &HttpClient,
    meta: &VersionMeta,
    meta_dir: &Path,
) -> Result<(), NetError> {
    let java = crate::instance::java::resolve_java_path(None);
    let runtime = crate::instance::java::probe_java(Path::new(&java)).await?;
    download_libraries_for_platform(client, meta, meta_dir, &runtime.platform).await
}

pub async fn download_libraries_for_platform(
    client: &HttpClient,
    meta: &VersionMeta,
    meta_dir: &Path,
    platform: &JavaPlatform,
) -> Result<(), NetError> {
    if !crate::storage::safe_relative_path(Path::new(&meta.id))
        || Path::new(&meta.id).components().count() != 1
    {
        return Err(NetError::Parse(format!(
            "Invalid Minecraft version ID: {}",
            meta.id
        )));
    }
    let libraries: Vec<_> = meta
        .libraries
        .iter()
        .map(|lib| model::Library {
            name: lib.name.clone(),
            downloads: Some(lib.downloads.clone()),
            rules: lib.rules.clone(),
            natives: lib.natives.clone(),
            extract: lib.extract.clone(),
            ..Default::default()
        })
        .collect();
    download_profile_libraries(
        client,
        &libraries,
        &crate::storage::MetadataPaths::new(meta_dir).libraries(),
        &platform.natives_directory(meta_dir, &meta.id),
        platform,
        &crate::launch_profile::rules::FeatureSet::default(),
    )
    .await
}

pub(crate) async fn download_profile_libraries(
    client: &HttpClient,
    libraries: &[model::Library],
    lib_dir: &Path,
    natives_dir: &Path,
    platform: &JavaPlatform,
    features: &crate::launch_profile::rules::FeatureSet,
) -> Result<(), NetError> {
    set_action("Downloading libraries...");
    tracing::debug!(
        "Resolving {} libraries for Java {} {}",
        libraries.len(),
        platform.os_name,
        platform.arch
    );

    let rule_ctx = crate::launch_profile::rules::RuleContext {
        os_name: platform.os_name,
        os_version: &platform.os_version,
        arch: &platform.arch,
        features,
    };

    let mut downloads: Vec<(String, PathBuf, String, String, u64)> = Vec::new();
    // natives jars to unpack once every download has landed:
    // (jar path inside the library cache, extract.exclude prefixes)
    let mut natives_jars: Vec<(PathBuf, Vec<String>)> = Vec::new();
    for library in libraries {
        if let Some(rules) = &library.rules
            && !crate::launch_profile::rules::evaluate(rules, &rule_ctx)
        {
            tracing::trace!("Skipping library {} due to platform rules", library.name);
            continue;
        }

        if let Some(artifact) = library.downloads.as_ref().and_then(|d| d.artifact.as_ref()) {
            let rel = library_artifact_path(library)?.expect("artifact has a path");
            let destination = lib_dir.join(&rel);
            if verify_cached(&destination, &artifact.sha1, artifact.size) {
                tracing::trace!("Library already cached: {}", rel.display());
            } else {
                downloads.push((
                    artifact.url.clone(),
                    destination,
                    rel.to_string_lossy().into_owned(),
                    artifact.sha1.clone(),
                    artifact.size,
                ));
            }
        }

        // native classifier jar (pre-1.13 era libraries). the jar itself is
        // cached alongside the other libraries; extraction is isolated by Java architecture.
        if let Some(classifier) = library
            .natives
            .as_ref()
            .and_then(|n| n.get(platform.os_name))
            .map(|classifier| classifier.replace("${arch}", &platform.bitness.to_string()))
        {
            let info = library
                .downloads
                .as_ref()
                .and_then(|downloads| downloads.classifiers.as_ref())
                .and_then(|classifiers| classifiers.get(&classifier))
                .ok_or_else(|| {
                    NetError::Parse(format!(
                        "Library {} declares missing native classifier {classifier}",
                        library.name
                    ))
                })?;
            // the maven fallback must carry the classifier: it selects
            // <artifact>-<version>-<classifier>.jar, not the base jar.
            let rel = if !info.path.is_empty() {
                info.path.clone()
            } else {
                crate::instance::loader::maven::maven_coord_to_path(&format!(
                    "{}:{}",
                    library.name, classifier
                ))
                .ok_or_else(|| {
                    NetError::Parse(format!("Invalid native library path for {}", library.name))
                })?
            };
            if !crate::storage::safe_relative_path(Path::new(&rel)) {
                return Err(NetError::Parse(format!(
                    "Invalid native library path for {}",
                    library.name
                )));
            }
            let destination = lib_dir.join(&rel);
            if !verify_cached(&destination, &info.sha1, info.size) {
                downloads.push((
                    info.url.clone(),
                    destination.clone(),
                    rel,
                    info.sha1.clone(),
                    info.size,
                ));
            }
            let exclude = library
                .extract
                .as_ref()
                .and_then(|extract| extract.exclude.clone())
                .unwrap_or_default();
            natives_jars.push((destination, exclude));
        }
    }

    let result = if downloads.is_empty() {
        tracing::info!("All libraries already cached");
        Ok(())
    } else {
        tracing::debug!("Downloading {} missing libraries", downloads.len());
        run_parallel_downloads(client, downloads, false).await
    };
    clear();
    result?;

    if !natives_jars.is_empty() {
        std::fs::create_dir_all(natives_dir)?;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(natives_dir.join(".extract.lock"))?;
        lock.lock()?;
        for (jar, exclude) in &natives_jars {
            extract_natives(jar, natives_dir, exclude)?;
        }
    }

    Ok(())
}

pub(crate) fn library_artifact_path(library: &model::Library) -> Result<Option<PathBuf>, NetError> {
    let recorded_path = match &library.downloads {
        Some(downloads) => match &downloads.artifact {
            Some(artifact) => &artifact.path,
            // A classifier-only library belongs in the extraction directory, not the classpath.
            None => return Ok(None),
        },
        None => "",
    };
    let path = if recorded_path.is_empty() {
        crate::instance::loader::maven::maven_coord_to_path(&library.name)
    } else {
        Some(recorded_path.to_owned())
    }
    .filter(|path| crate::storage::safe_relative_path(Path::new(path)))
    .ok_or_else(|| NetError::Parse(format!("Invalid library path for {}", library.name)))?;
    Ok(Some(PathBuf::from(path)))
}

fn extract_natives(jar: &Path, dest: &Path, exclude: &[String]) -> Result<(), NetError> {
    use std::io::Read;
    let file = std::fs::File::open(jar)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| NetError::Parse(format!("Invalid natives jar {}: {e}", jar.display())))?;
    std::fs::create_dir_all(dest)?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| {
            NetError::Parse(format!(
                "Corrupt entry in natives jar {}: {e}",
                jar.display()
            ))
        })?;
        if entry.is_dir() {
            continue;
        }
        let Some(rel) = entry.enclosed_name() else {
            tracing::warn!(
                "Skipping unsafe path in natives jar {}: {}",
                jar.display(),
                entry.name()
            );
            continue;
        };
        // zip entry names use '/', but PathBuf renders them with the host
        // separator ('\'), so normalize before matching exclude prefixes.
        let name = rel.to_string_lossy().replace('\\', "/");
        if exclude.iter().any(|prefix| name.starts_with(prefix)) {
            continue;
        }
        let out_path = dest.join(&rel);
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        // Reusing a loaded DLL must not truncate or replace it on another launch.
        let cached = match std::fs::read(&out_path) {
            Ok(cached) => Some(cached),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(NetError::Parse(format!(
                    "Could not read native {}: {error}",
                    out_path.display()
                )));
            }
        };
        if cached.as_deref() != Some(bytes.as_slice()) {
            crate::storage::write_atomic(&out_path, &bytes)?;
        }
    }
    Ok(())
}

pub async fn download_assets(
    client: &HttpClient,
    meta: &VersionMeta,
    meta_dir: &Path,
) -> Result<(), NetError> {
    download_assets_from(client, meta, meta_dir, ASSETS_BASE_URL).await
}

pub async fn download_assets_from(
    client: &HttpClient,
    meta: &VersionMeta,
    meta_dir: &Path,
    assets_base: &str,
) -> Result<(), NetError> {
    set_action("Downloading assets...");
    if !crate::storage::safe_relative_path(Path::new(&meta.asset_index.id))
        || Path::new(&meta.asset_index.id).components().count() != 1
    {
        clear();
        return Err(NetError::Parse("Invalid asset index ID".to_owned()));
    }
    let index_path = crate::storage::MetadataPaths::new(meta_dir)
        .assets()
        .join("indexes")
        .join(format!("{}.json", meta.asset_index.id));
    let cached = match tokio::fs::read(&index_path).await {
        Ok(bytes) if sha1_matches(&bytes, &meta.asset_index.sha1) => {
            serde_json::from_slice(&bytes).ok()
        }
        Ok(_) => {
            tracing::debug!(
                "Cached asset index at {} does not match the requested version; fetching again",
                index_path.display()
            );
            None
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let asset_index: AssetIndexContent = if let Some(index) = cached {
        index
    } else {
        tracing::debug!(
            "Fetching asset index {} from {}",
            meta.asset_index.id,
            meta.asset_index.url
        );
        let bytes = match client.get_bytes(&meta.asset_index.url).await {
            Ok(bytes) => bytes,
            Err(e) => {
                clear();
                return Err(e);
            }
        };
        if !sha1_matches(&bytes, &meta.asset_index.sha1) {
            clear();
            return Err(NetError::Parse(format!(
                "Asset index '{}' failed its SHA-1 verification",
                meta.asset_index.id
            )));
        }
        let index = serde_json::from_slice(&bytes)
            .map_err(|error| NetError::Parse(format!("Invalid asset index: {error}")))?;
        crate::storage::write_atomic(&index_path, &bytes)?;
        index
    };

    let mut downloads = Vec::new();
    let mut hashes = HashSet::new();
    for object in asset_index.objects.values() {
        if object.hash.len() != 40 || !object.hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            clear();
            return Err(NetError::Parse(format!(
                "Invalid asset hash: {}",
                object.hash
            )));
        }

        let prefix = &object.hash[..2];
        let url = format!("{}/{}/{}", assets_base, prefix, object.hash);
        let destination = crate::storage::MetadataPaths::new(meta_dir)
            .assets()
            .join("objects")
            .join(prefix)
            .join(&object.hash);

        if !hashes.insert(&object.hash) || verify_cached(&destination, &object.hash, object.size) {
            continue;
        }

        downloads.push((
            url,
            destination,
            object.hash.clone(),
            object.hash.clone(),
            object.size,
        ));
    }

    if downloads.is_empty() {
        tracing::info!("All assets already cached");
    } else if let Err(error) = run_parallel_downloads(client, downloads, true).await {
        clear();
        return Err(error);
    }
    if asset_index.virtual_assets {
        let assets = crate::storage::MetadataPaths::new(meta_dir).assets();
        for (name, object) in &asset_index.objects {
            if !crate::storage::safe_relative_path(Path::new(name)) {
                clear();
                return Err(NetError::Parse(format!(
                    "Invalid virtual asset path: {name}"
                )));
            }
            let destination = assets.join("virtual").join(&meta.asset_index.id).join(name);
            if !verify_cached(&destination, &object.hash, object.size) {
                let source = assets
                    .join("objects")
                    .join(&object.hash[..2])
                    .join(&object.hash);
                let bytes = tokio::fs::read(source).await?;
                crate::storage::write_atomic(&destination, &bytes)?;
            }
        }
    }
    clear();
    Ok(())
}

// Continue other downloads after one fails; report the first error afterward.
async fn run_parallel_downloads(
    client: &HttpClient,
    downloads: Vec<(String, PathBuf, String, String, u64)>,
    report_count_progress: bool,
) -> Result<(), NetError> {
    let total_downloads = downloads.len() as u64;
    tracing::debug!(
        "Starting {} parallel download job(s), max_concurrent={}",
        total_downloads,
        MAX_CONCURRENT_DOWNLOADS
    );
    let mut completed = 0;
    let mut queue = downloads.into_iter();
    let mut set = JoinSet::new();

    for _ in 0..MAX_CONCURRENT_DOWNLOADS {
        let next_job = match queue.next() {
            Some(job) => job,
            None => break,
        };

        spawn_download_task(&mut set, client, next_job);
    }

    let mut first_error: Option<NetError> = None;

    while let Some(join_result) = set.join_next().await {
        match join_result {
            Ok(Ok(label)) => {
                completed += 1;
                if report_count_progress {
                    set_progress(completed, total_downloads);
                }
                set_sub_action(label);
            }
            Ok(Err(e)) => {
                tracing::debug!("Download failed: {}", e);
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
            Err(e) => {
                tracing::debug!("Task panicked: {}", e);
                if first_error.is_none() {
                    first_error = Some(NetError::TaskFailed(format!("Join error: {}", e)));
                }
            }
        }

        let next_job = match queue.next() {
            Some(job) => job,
            None => continue,
        };

        spawn_download_task(&mut set, client, next_job);
    }

    match first_error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn spawn_download_task(
    set: &mut JoinSet<Result<String, NetError>>,
    client: &HttpClient,
    job: (String, PathBuf, String, String, u64),
) {
    let (url, destination, label, sha1, size) = job;
    let task_client = client.clone();

    set.spawn(async move {
        tracing::trace!(
            "Starting parallel download '{}' to {}",
            label,
            destination.display()
        );
        let result = download_file(&task_client, &url, &destination, |_current, _total| {}).await;
        result?;
        if !verify_cached(&destination, &sha1, size) {
            tokio::fs::remove_file(&destination).await?;
            return Err(NetError::Parse(format!(
                "Downloaded '{label}' failed its SHA-1 or size verification"
            )));
        }
        Ok({
            tracing::trace!("Finished parallel download '{}'", label);
            label
        })
    });
}
