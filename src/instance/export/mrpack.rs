// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;

use super::*;
use crate::instance::import::mrpack::{MrpackFile, MrpackIndex};
use crate::net::modrinth::VersionInfo;

pub(super) async fn prepare(
    plan: &ExportPlan,
    progress: &ProgressTask,
) -> ExportResult<ArchiveContents> {
    manifest(plan, &resolve(&plan.files, progress).await?)
}

pub(super) async fn resolve(
    files: &BTreeMap<PathBuf, FileFingerprint>,
    progress: &ProgressTask,
) -> ExportResult<HashMap<String, VersionInfo>> {
    let hashes = files
        .values()
        .filter_map(|file| file.hash("sha512").map(str::to_owned))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let client = crate::net::HttpClient::new();
    let mut matches = HashMap::new();
    for (index, chunk) in hashes.chunks(100).enumerate() {
        progress.set_sub_action("Identifying Modrinth content");
        progress.set_progress((index * 100) as u64, hashes.len() as u64);
        matches
            .extend(crate::net::modrinth::resolve_version_files(&client, chunk, "sha512").await?);
    }
    Ok(matches)
}

pub(super) fn manifest(
    plan: &ExportPlan,
    matches: &HashMap<String, VersionInfo>,
) -> ExportResult<ArchiveContents> {
    let mut referenced = BTreeSet::new();
    let mut files = Vec::new();
    for (path, hash) in &plan.files {
        let Some(sha512) = hash.hash("sha512") else {
            continue;
        };
        let Some(version) = matches.get(sha512) else {
            continue;
        };
        let Some(file) = version.files.iter().find(|file| {
            file.hashes
                .get("sha512")
                .is_some_and(|expected| expected.eq_ignore_ascii_case(sha512))
                && file.hashes.get("sha1").is_some_and(|expected| {
                    hash.hash("sha1")
                        .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
                })
                && file.size == hash.size
                && reqwest::Url::parse(&file.url).is_ok_and(|url| {
                    url.scheme() == "https" && url.host_str() == Some("cdn.modrinth.com")
                })
        }) else {
            continue;
        };
        files.push(MrpackFile {
            path: path
                .to_str()
                .ok_or("Export paths must be UTF-8")?
                .replace('\\', "/"),
            hashes: ["sha1", "sha512"]
                .into_iter()
                .map(|algorithm| {
                    (
                        algorithm.to_owned(),
                        hash.hash(algorithm).unwrap().to_owned(),
                    )
                })
                .collect(),
            // Installed client content does not establish server compatibility.
            env: HashMap::from([
                ("client".to_owned(), "required".to_owned()),
                ("server".to_owned(), "unsupported".to_owned()),
            ]),
            downloads: vec![file.url.clone()],
            file_size: hash.size,
        });
        referenced.insert(path.clone());
    }
    let options = &plan.options;
    let mut dependencies = HashMap::from([("minecraft".to_owned(), options.game_version.clone())]);
    let loader = match options.loader {
        ModLoader::Vanilla => None,
        ModLoader::Forge => Some("forge"),
        ModLoader::Fabric => Some("fabric-loader"),
        ModLoader::NeoForge => Some("neoforge"),
        ModLoader::Quilt => Some("quilt-loader"),
    };
    if let Some(loader) = loader {
        dependencies.insert(
            loader.to_owned(),
            options
                .loader_version
                .clone()
                .ok_or("Missing loader version")?,
        );
    }
    let index = MrpackIndex {
        format_version: 1,
        game: "minecraft".to_owned(),
        version_id: options.version.clone(),
        name: options.name.clone(),
        dependencies,
        files,
    };
    Ok(ArchiveContents {
        manifests: vec![("modrinth.index.json", serde_json::to_vec_pretty(&index)?)],
        referenced,
        prefix: "client-overrides",
    })
}
