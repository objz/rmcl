// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// modpack importing from modrinth (remote slug/url) or local files (.mrpack, mmc zips).
// resolves the input, downloads if needed, then hands off to the import engine.
use clap::ArgMatches;

use crate::instance::InstanceManager;
use crate::instance::import::{ImportInput, parse_import_input};
use crate::net::modrinth;

type CliResult = Result<(), Box<dyn std::error::Error>>;

pub async fn handle_import(matches: &ArgMatches) -> CliResult {
    let input = matches.get_one::<String>("source").unwrap();
    let override_name = matches.get_one::<String>("name");
    let override_version = matches.get_one::<String>("version");

    let instances_dir = crate::config::SETTINGS.read().paths.resolve_instances_dir();
    let meta_dir = crate::config::SETTINGS.read().paths.resolve_meta_dir();
    let manager = InstanceManager::new(instances_dir, meta_dir);
    let client = crate::net::HttpClient::new();

    let parsed = parse_import_input(input);

    let archive_path = match parsed {
        ImportInput::LocalFile(ref path) => {
            let resolved = crate::config::settings::resolve_path(path);
            if !resolved.exists() {
                return Err(format!("File not found: {}", resolved.display()).into());
            }
            resolved
        }
        ImportInput::ProjectSlug(ref slug) => {
            println!("Fetching project '{slug}'...");
            let project = modrinth::fetch_project(&client, slug)
                .await
                .map_err(|e| format!("Failed to fetch project: {e}"))?;
            println!("Found: {}", project.title);

            let versions = modrinth::fetch_versions(&client, slug)
                .await
                .map_err(|e| format!("Failed to fetch versions: {e}"))?;

            let version = select_version(&versions, override_version.map(String::as_str))?;

            println!(
                "Using version {} ({})",
                version.version_number,
                version.game_versions.first().unwrap_or(&"?".to_string())
            );

            let tmp_dir = crate::storage::MetadataPaths::new(&manager.meta_dir).temporary();
            std::fs::create_dir_all(&tmp_dir)?;
            modrinth::download_mrpack(&client, version, &tmp_dir)
                .await
                .map_err(|e| format!("Failed to download .mrpack: {e}"))?
        }
        ImportInput::VersionId {
            slug: _,
            ref version_id,
        } => {
            println!("Fetching version '{version_id}'...");
            let version = modrinth::fetch_version(&client, version_id)
                .await
                .map_err(|e| format!("Failed to fetch version: {e}"))?;

            println!(
                "Found: {} ({})",
                version.version_number,
                version.game_versions.first().unwrap_or(&"?".to_string())
            );

            let tmp_dir = crate::storage::MetadataPaths::new(&manager.meta_dir).temporary();
            std::fs::create_dir_all(&tmp_dir)?;
            modrinth::download_mrpack(&client, &version, &tmp_dir)
                .await
                .map_err(|e| format!("Failed to download .mrpack: {e}"))?
        }
    };

    let mut summary = crate::instance::import::build_summary(&archive_path)
        .map_err(|e| format!("Invalid modpack: {e}"))?;

    if let Some(name) = override_name {
        summary.name = name.clone();
    }

    println!(
        "Importing '{}' - {} {} ({} mods, {} overrides)",
        summary.name,
        summary.game_version,
        summary.loader,
        summary.mod_count,
        summary.override_count
    );

    let config = crate::instance::import::execute_import(&summary, &manager)
        .await
        .map_err(|e| format!("Import failed: {e}"))?;

    println!("Instance '{}' created successfully.", config.name);
    Ok(())
}

fn select_version<'a>(
    versions: &'a [modrinth::VersionInfo],
    requested: Option<&str>,
) -> Result<&'a modrinth::VersionInfo, String> {
    requested.map_or_else(
        || {
            versions
                .first()
                .ok_or_else(|| "No versions found for this modpack".to_owned())
        },
        |name| {
            versions
                .iter()
                .find(|version| version.version_number == name || version.name == name)
                .ok_or_else(|| format!("Version '{name}' not found"))
        },
    )
}

#[cfg(test)]
#[path = "tests/import.rs"]
mod tests;
