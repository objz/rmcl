// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// instance lifecycle: create, delete, rename, launch, config, desktop shortcuts.
// the heaviest handler in the CLI since instances are the core concept.
use std::io;
use std::time::Duration;

use clap::ArgMatches;

use super::utils::{confirm, required_arg};
use crate::cli::output::{format_datetime, print_table};
use crate::instance::runtime::RunState;
use crate::instance::{InstanceManager, ModLoader, models::parse_resolution};

type CliResult = Result<(), Box<dyn std::error::Error>>;
const LOCAL_CONFIG_PROFILE: &str = "instance default";

pub async fn handle_instance(matches: &ArgMatches) -> CliResult {
    match matches.subcommand() {
        Some(("list", _)) => list_instances(),
        Some(("create", sub_matches)) => create_instance(sub_matches).await,
        Some(("delete", sub_matches)) => delete_instance(sub_matches),
        Some(("rename", sub_matches)) => rename_instance(sub_matches),
        Some(("launch", sub_matches)) => launch_instance(sub_matches).await,
        Some(("config", sub_matches)) => config_instance(sub_matches),
        Some(("profile", sub_matches)) => profile_instance(sub_matches),
        Some(("desktop", sub_matches)) => desktop_instance(sub_matches),
        _ => Ok(()),
    }
}

pub(crate) fn parse_loader(input: &str) -> Result<ModLoader, String> {
    match input.to_lowercase().as_str() {
        "vanilla" => Ok(ModLoader::Vanilla),
        "fabric" => Ok(ModLoader::Fabric),
        "forge" => Ok(ModLoader::Forge),
        "neoforge" => Ok(ModLoader::NeoForge),
        "quilt" => Ok(ModLoader::Quilt),
        _ => Err(format!(
            "unknown loader '{}'. Valid: vanilla, fabric, forge, neoforge, quilt",
            input
        )),
    }
}

fn manager() -> InstanceManager {
    let instances_dir = crate::config::SETTINGS.read().paths.resolve_instances_dir();
    let meta_dir = crate::config::SETTINGS.read().paths.resolve_meta_dir();
    InstanceManager::new(instances_dir, meta_dir)
}

fn list_instances() -> CliResult {
    let mut instances = manager().load_all();
    instances.sort_by_key(|instance| instance.name.to_lowercase());

    let rows = instances
        .into_iter()
        .map(|config| {
            vec![
                config.name,
                config.game_version,
                config.loader.to_string(),
                config
                    .last_played
                    .as_ref()
                    .map(format_datetime)
                    .unwrap_or_else(|| "-".to_string()),
            ]
        })
        .collect::<Vec<_>>();

    print_table(&["Name", "Version", "Loader", "Last Played"], &rows);
    Ok(())
}

async fn create_instance(matches: &ArgMatches) -> CliResult {
    let name = required_arg(matches, "name")?;
    let version = required_arg(matches, "version")?;
    let loader = parse_loader(required_arg(matches, "loader")?).map_err(io::Error::other)?;
    let loader_version = matches
        .get_one::<String>("loader-version")
        .map(String::as_str);
    let manager = manager();

    println!("Creating instance '{}'...", name);
    let config = manager
        .create(name, version, loader, loader_version)
        .await?;
    println!(
        "Created '{}'  ({} {})",
        config.name, config.game_version, config.loader
    );
    Ok(())
}

fn delete_instance(matches: &ArgMatches) -> CliResult {
    let name = required_arg(matches, "name")?;
    if !matches.get_flag("yes") && !confirm(&format!("Delete '{}'", name))? {
        println!("Cancelled.");
        return Ok(());
    }

    manager().delete(name)?;
    println!("Deleted '{}'.", name);
    Ok(())
}

fn rename_instance(matches: &ArgMatches) -> CliResult {
    let old_name = required_arg(matches, "old")?;
    let new_name = required_arg(matches, "new")?;
    manager().rename(old_name, new_name)?;
    println!("Renamed '{}' to '{}'.", old_name, new_name);
    Ok(())
}

async fn launch_instance(matches: &ArgMatches) -> CliResult {
    let name = required_arg(matches, "name")?;
    let manager = manager();
    let instances_dir = crate::config::SETTINGS.read().paths.resolve_instances_dir();
    let meta_dir = crate::config::SETTINGS.read().paths.resolve_meta_dir();
    let config = manager.load_one(name)?;

    crate::instance::launch::launch(&config, &instances_dir, &meta_dir, None)
        .await
        .map_err(|error| io::Error::other(format!("Launch failed: {}", error)))?;

    // poll until the game process exits. in CLI mode this blocks here
    // so the user gets a proper exit code at the end.
    loop {
        match crate::instance::runtime::get(name) {
            Some(RunState::Crashed(Some(code))) => {
                println!("Game exited with status {}", code);
                break;
            }
            Some(RunState::Crashed(None)) => {
                println!("Game was terminated");
                break;
            }
            Some(_) => tokio::time::sleep(Duration::from_millis(500)).await,
            None => {
                break;
            }
        }
    }

    Ok(())
}

fn desktop_instance(matches: &ArgMatches) -> CliResult {
    let name = required_arg(matches, "name")?;
    let config = manager().load_one(name)?;
    let enabled = crate::instance::desktop::toggle(&config)?;
    if enabled {
        println!("Desktop shortcut created for '{}'.", name);
    } else {
        println!("Desktop shortcut removed for '{}'.", name);
    }
    Ok(())
}

fn config_instance(matches: &ArgMatches) -> CliResult {
    let name = required_arg(matches, "name")?;
    let manager = manager();

    if let Some(set_value) = matches.get_one::<String>("set") {
        let (key, value) = set_value
            .split_once('=')
            .ok_or_else(|| io::Error::other("--set must be in key=value format"))?;

        let mut config = manager.load_one(name)?;
        apply_config_update(&mut config, key, value)?;
        manager.save(&config)?;
        println!("Updated '{}' {}.", name, key);
        return Ok(());
    }

    let config = manager.load_one(name)?;
    let rows = vec![
        vec!["name".to_string(), config.name],
        vec!["game-version".to_string(), config.game_version],
        vec!["loader".to_string(), config.loader.to_string()],
        vec![
            "loader-version".to_string(),
            config.loader_version.unwrap_or_else(|| "-".to_string()),
        ],
        vec!["created".to_string(), format_datetime(&config.created)],
        vec![
            "last-played".to_string(),
            config
                .last_played
                .as_ref()
                .map(format_datetime)
                .unwrap_or_else(|| "-".to_string()),
        ],
        vec![
            "java-path".to_string(),
            config.java_path.unwrap_or_else(|| "-".to_string()),
        ],
        vec![
            "memory-max".to_string(),
            config.memory_max.unwrap_or_else(|| "-".to_string()),
        ],
        vec![
            "memory-min".to_string(),
            config.memory_min.unwrap_or_else(|| "-".to_string()),
        ],
        vec![
            "jvm-args".to_string(),
            if config.jvm_args.is_empty() {
                "-".to_string()
            } else {
                config.jvm_args.join(" ")
            },
        ],
        vec![
            "resolution".to_string(),
            config
                .resolution
                .map(|(width, height)| format!("{}x{}", width, height))
                .unwrap_or_else(|| "-".to_string()),
        ],
        vec![
            "config-sync-profile".to_string(),
            display_config_profile(config.config_sync_profile.as_deref()).to_string(),
        ],
    ];

    print_table(&["Field", "Value"], &rows);
    Ok(())
}

fn profile_instance(matches: &ArgMatches) -> CliResult {
    let name = required_arg(matches, "name")?;
    let manager = manager();
    let mut config = manager.load_one(name)?;
    let profiles = crate::instance::config_sync::list_profiles(&manager.meta_dir)?;
    if config
        .config_sync_profile
        .as_ref()
        .is_some_and(|profile| !profiles.iter().any(|candidate| candidate == profile))
    {
        config.config_sync_profile = None;
        manager.save(&config)?;
    }

    let Some(profile) = matches.get_one::<String>("profile") else {
        let rows = vec![
            vec![
                "current".to_string(),
                display_config_profile(config.config_sync_profile.as_deref()).to_string(),
            ],
            vec![
                "available".to_string(),
                if profiles.is_empty() {
                    LOCAL_CONFIG_PROFILE.to_string()
                } else {
                    format!("{}, {}", LOCAL_CONFIG_PROFILE, profiles.join(", "))
                },
            ],
        ];
        print_table(&["Field", "Value"], &rows);
        return Ok(());
    };

    let target = if is_local_profile_alias(profile) {
        None
    } else {
        Some(profile.as_str())
    };
    let instance_dir = manager.instances_dir.join(&config.name);
    let selected = crate::instance::config_sync::switch_profile(
        &config.name,
        config.config_sync_profile.as_deref(),
        target,
        &manager.meta_dir,
        &instance_dir,
    )?;
    config.config_sync_profile = selected;
    manager.save(&config)?;

    println!(
        "Updated '{}' config profile to {}.",
        name,
        display_config_profile(config.config_sync_profile.as_deref())
    );
    Ok(())
}

fn display_config_profile(profile: Option<&str>) -> &str {
    profile.unwrap_or(LOCAL_CONFIG_PROFILE)
}

fn is_local_profile_alias(profile: &str) -> bool {
    let profile = profile.trim();
    profile.eq_ignore_ascii_case("none")
        || profile.eq_ignore_ascii_case("default")
        || profile.eq_ignore_ascii_case("local")
        || profile.eq_ignore_ascii_case("instance-default")
        || profile.eq_ignore_ascii_case("instance default")
}

// maps --set key=value pairs to config fields. empty value = clear the field.
fn apply_config_update(
    config: &mut crate::instance::InstanceConfig,
    key: &str,
    value: &str,
) -> CliResult {
    match key {
        "memory-max" => {
            config.memory_max = crate::instance::normalize_memory_value(value);
        }
        "memory-min" => {
            config.memory_min = crate::instance::normalize_memory_value(value);
        }
        "java-path" => {
            config.java_path = if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            }
        }
        "jvm-args" => {
            config.jvm_args = value.split_whitespace().map(String::from).collect();
        }
        "resolution" => {
            config.resolution = if value.is_empty() {
                None
            } else {
                Some(parse_resolution(value).map_err(io::Error::other)?)
            };
        }
        _ => {
            return Err(io::Error::other(format!("unknown key '{}'", key)).into());
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "tests/instance.rs"]
mod tests;
