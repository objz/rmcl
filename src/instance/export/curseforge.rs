// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;
use crate::instance::import::curseforge::{Manifest, ManifestFile, ManifestLoader, Minecraft};
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
) -> ExportResult<BTreeMap<u32, VersionInfo>> {
    let fingerprints = files
        .iter()
        .filter(|(path, _)| reference_candidate(path, PackFormat::CurseForge))
        .filter_map(|(_, file)| {
            file.hash("curseforge")
                .and_then(|value| value.parse::<u32>().ok())
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut matches = BTreeMap::new();
    if !fingerprints.is_empty() {
        let key =
            crate::net::curseforge::api_key().ok_or("CurseForge API key is not configured")?;
        let client = crate::net::HttpClient::new();
        for (index, chunk) in fingerprints.chunks(100).enumerate() {
            progress.set_sub_action("Identifying CurseForge mods");
            progress.set_progress((index * 100) as u64, fingerprints.len() as u64);
            matches.extend(
                crate::net::curseforge::resolve_fingerprint_versions(&client, key, chunk).await?,
            );
        }
    }
    Ok(matches)
}

pub(super) fn manifest(
    plan: &ExportPlan,
    matches: &BTreeMap<u32, VersionInfo>,
) -> ExportResult<ArchiveContents> {
    let mut referenced = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for (path, hash) in &plan.files {
        if !reference_candidate(path, PackFormat::CurseForge) {
            continue;
        }
        let Some(version) = hash
            .hash("curseforge")
            .and_then(|hash| hash.parse().ok())
            .and_then(|hash| matches.get(&hash))
        else {
            continue;
        };
        if version.files.iter().any(|file| {
            file.hashes.get("sha1").is_some_and(|expected| {
                hash.hash("sha1")
                    .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
            })
        }) {
            ids.insert((
                version.project_id.parse::<u64>()?,
                version.id.parse::<u64>()?,
            ));
            referenced.insert(path.clone());
        }
    }
    let options = &plan.options;
    let mod_loaders = if options.loader == ModLoader::Vanilla {
        Vec::new()
    } else {
        vec![ManifestLoader {
            id: format!(
                "{}-{}",
                options.loader.to_string().to_lowercase(),
                options
                    .loader_version
                    .as_deref()
                    .ok_or("Missing loader version")?
            ),
            primary: true,
        }]
    };
    let manifest = Manifest {
        minecraft: Minecraft {
            version: options.game_version.clone(),
            mod_loaders,
        },
        manifest_type: "minecraftModpack".to_owned(),
        manifest_version: 1,
        name: options.name.clone(),
        version: options.version.clone(),
        author: options.author.clone(),
        files: ids
            .into_iter()
            .map(|(project_id, file_id)| ManifestFile {
                project_id,
                file_id,
                required: true,
            })
            .collect(),
        overrides: "overrides".to_owned(),
    };
    Ok(ArchiveContents {
        manifests: vec![("manifest.json", serde_json::to_vec_pretty(&manifest)?)],
        referenced,
        prefix: "overrides",
    })
}
