// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// cli entry point and clap command definitions.
// no subcommand = launch TUI, otherwise dispatch to the appropriate handler.
mod account;
mod content;
mod import;
mod instance;
mod log;
mod output;
mod utils;
mod version;

use clap::{Arg, ArgAction, ArgGroup, Command};

pub async fn init() {
    let matches = build_command().get_matches();

    // no subcommand means the user just ran `rmcl` bare, so fall through to TUI mode
    if matches.subcommand().is_none() {
        // force-init the theme so it's ready before the TUI renders
        let _ = &*crate::config::theme::THEME;
        if let Err(e) = crate::tui::show().await {
            tracing::error!("TUI error: {}", e);
        }
        return;
    }

    let instances_dir = crate::config::SETTINGS.read().paths.resolve_instances_dir();
    let meta_dir = crate::config::SETTINGS.read().paths.resolve_meta_dir();
    let config = crate::config::get_config_path().join("config.toml");
    if crate::layout_migration::is_needed(&instances_dir, &meta_dir) {
        if let Err(error) =
            crate::layout_migration::run(&instances_dir, &meta_dir, &config, |progress| {
                eprintln!(
                    "{}: {} ({}/{})",
                    progress.phase, progress.item, progress.current, progress.total
                );
            })
        {
            eprintln!("error: layout migration failed safely: {error}");
            std::process::exit(1);
        }
    } else if let Err(error) = crate::layout_migration::initialize_new_layout(&meta_dir) {
        eprintln!("error: cannot initialize metadata layout: {error}");
        std::process::exit(1);
    } else if let Err(error) = crate::config::upgrade_config_file(&config) {
        eprintln!("warning: cannot upgrade launcher config: {error}");
    }
    if crate::layout_migration::cache_rebuild_pending(&meta_dir) {
        let manager = crate::instance::InstanceManager::new(&instances_dir, &meta_dir);
        let instances = manager.load_all();
        for (index, instance) in instances.iter().enumerate() {
            eprintln!(
                "Rebuilding runtime cache: {} ({}/{})",
                instance.name,
                index + 1,
                instances.len()
            );
            if let Err(error) = manager.repair_runtime_cache(instance).await {
                eprintln!(
                    "error: runtime cache rebuild stopped safely for '{}': {error}",
                    instance.name
                );
                std::process::exit(1);
            }
        }
        if let Err(error) = crate::layout_migration::finish_cache_rebuild(&meta_dir) {
            eprintln!("error: cannot finish runtime cache migration: {error}");
            std::process::exit(1);
        }
    }

    let result = match matches.subcommand() {
        Some(("instance", sub_matches)) => instance::handle_instance(sub_matches).await,
        Some(("mod", sub_matches)) => content::handle_mod(sub_matches),
        Some(("pack", sub_matches)) => content::handle_pack(sub_matches),
        Some(("shader", sub_matches)) => content::handle_shader(sub_matches),
        Some(("account", sub_matches)) => account::handle_account(sub_matches).await,
        Some(("log", sub_matches)) => log::handle_log(sub_matches).await,
        Some(("version", sub_matches)) => version::handle_version(sub_matches).await,
        Some(("import", sub_matches)) => import::handle_import(sub_matches).await,
        _ => Ok(()),
    };

    if let Err(e) = result {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}

fn build_command() -> Command {
    Command::new("rmcl")
        .about("Minecraft CLI Launcher")
        .version(env!("CARGO_PKG_VERSION"))
        .subcommand_required(false)
        .arg_required_else_help(false)
        .subcommand(
            Command::new("instance")
                .about("Manage launcher instances")
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommand(Command::new("list").about("List instances"))
                .subcommand(
                    Command::new("create")
                        .about("Create an instance")
                        .arg_required_else_help(true)
                        .arg(Arg::new("name").required(true).action(ArgAction::Set))
                        .arg(
                            Arg::new("version")
                                .long("version")
                                .required(true)
                                .action(ArgAction::Set),
                        )
                        .arg(
                            Arg::new("loader")
                                .long("loader")
                                .required(true)
                                .action(ArgAction::Set),
                        )
                        .arg(
                            Arg::new("loader-version")
                                .long("loader-version")
                                .action(ArgAction::Set),
                        ),
                )
                .subcommand(
                    Command::new("delete")
                        .about("Delete an instance")
                        .arg_required_else_help(true)
                        .arg(Arg::new("name").required(true).action(ArgAction::Set))
                        .arg(Arg::new("yes").long("yes").action(ArgAction::SetTrue)),
                )
                .subcommand(
                    Command::new("rename")
                        .about("Rename an instance")
                        .arg_required_else_help(true)
                        .arg(Arg::new("old").required(true).action(ArgAction::Set))
                        .arg(Arg::new("new").required(true).action(ArgAction::Set)),
                )
                .subcommand(
                    Command::new("launch")
                        .about("Launch an instance")
                        .arg_required_else_help(true)
                        .arg(Arg::new("name").required(true).action(ArgAction::Set)),
                )
                .subcommand(
                    Command::new("config")
                        .about("Show or update instance config")
                        .arg_required_else_help(true)
                        .arg(Arg::new("name").required(true).action(ArgAction::Set))
                        .arg(Arg::new("set").long("set").action(ArgAction::Set)),
                )
                .subcommand(
                    Command::new("profile")
                        .about("Show or set an instance config-sync profile")
                        .arg_required_else_help(true)
                        .arg(Arg::new("name").required(true).action(ArgAction::Set))
                        .arg(Arg::new("profile").action(ArgAction::Set)),
                )
                .subcommand(
                    Command::new("desktop")
                        .about("Toggle desktop shortcut for an instance")
                        .arg_required_else_help(true)
                        .arg(Arg::new("name").required(true).action(ArgAction::Set)),
                ),
        )
        .subcommand(build_content_command("mod", "mods"))
        .subcommand(build_content_command("pack", "resource packs"))
        .subcommand(build_content_command("shader", "shaders"))
        .subcommand(
            Command::new("account")
                .about("Manage accounts")
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommand(Command::new("list").about("List accounts"))
                .subcommand(
                    Command::new("add")
                        .about("Add an account")
                        .arg(
                            Arg::new("microsoft")
                                .long("microsoft")
                                .action(ArgAction::SetTrue),
                        )
                        .arg(Arg::new("offline").long("offline").action(ArgAction::Set))
                        .group(
                            ArgGroup::new("account_source")
                                .args(["microsoft", "offline"])
                                .required(true),
                        ),
                )
                .subcommand(
                    Command::new("delete")
                        .about("Delete an account")
                        .arg_required_else_help(true)
                        .arg(Arg::new("username").required(true).action(ArgAction::Set))
                        .arg(Arg::new("yes").long("yes").action(ArgAction::SetTrue)),
                )
                .subcommand(
                    Command::new("use")
                        .about("Set active account")
                        .arg_required_else_help(true)
                        .arg(Arg::new("username").required(true).action(ArgAction::Set)),
                ),
        )
        .subcommand(
            Command::new("log")
                .about("View instance logs")
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommand(
                    Command::new("list")
                        .about("List log files")
                        .arg_required_else_help(true)
                        .arg(Arg::new("instance").required(true).action(ArgAction::Set)),
                )
                .subcommand(
                    Command::new("show")
                        .about("Show a log file")
                        .arg_required_else_help(true)
                        .arg(Arg::new("instance").required(true).action(ArgAction::Set))
                        .arg(Arg::new("file").long("file").action(ArgAction::Set))
                        .arg(Arg::new("follow").long("follow").action(ArgAction::SetTrue)),
                ),
        )
        .subcommand(
            Command::new("version")
                .about("List available game versions")
                .subcommand_required(true)
                .arg_required_else_help(true)
                .subcommand(
                    Command::new("list")
                        .about("List available versions")
                        .arg(Arg::new("loader").long("loader").action(ArgAction::Set))
                        .arg(
                            Arg::new("snapshots")
                                .long("snapshots")
                                .action(ArgAction::SetTrue),
                        ),
                ),
        )
        .subcommand(
            Command::new("import")
                .about("Import a modpack")
                .arg_required_else_help(true)
                .arg(
                    Arg::new("source")
                        .required(true)
                        .action(ArgAction::Set)
                        .help("Modrinth URL, project slug, or local .mrpack file path"),
                )
                .arg(
                    Arg::new("version")
                        .long("version")
                        .action(ArgAction::Set)
                        .help("Modpack version to import (default: latest)"),
                )
                .arg(
                    Arg::new("name")
                        .long("name")
                        .action(ArgAction::Set)
                        .help("Override instance name"),
                ),
        )
}

// mods, resource packs, and shaders all share the same list/enable/disable shape
fn build_content_command(name: &'static str, about: &'static str) -> Command {
    Command::new(name)
        .about(format!("Manage {}", about))
        .subcommand_required(true)
        .arg_required_else_help(true)
        .subcommand(
            Command::new("list")
                .about(format!("List {}", about))
                .arg_required_else_help(true)
                .arg(Arg::new("instance").required(true).action(ArgAction::Set)),
        )
        .subcommand(
            Command::new("enable")
                .about(format!("Enable a {} entry", name))
                .arg_required_else_help(true)
                .arg(Arg::new("instance").required(true).action(ArgAction::Set))
                .arg(Arg::new(name).required(true).action(ArgAction::Set)),
        )
        .subcommand(
            Command::new("disable")
                .about(format!("Disable a {} entry", name))
                .arg_required_else_help(true)
                .arg(Arg::new("instance").required(true).action(ArgAction::Set))
                .arg(Arg::new(name).required(true).action(ArgAction::Set)),
        )
}

#[cfg(test)]
#[path = "tests/arguments.rs"]
mod tests;
