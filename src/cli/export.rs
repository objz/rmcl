use clap::{Arg, ArgAction, ArgGroup, ArgMatches, Command};

use crate::instance::InstanceManager;
use crate::instance::export::{ExportOptions, export_curseforge};

pub fn command() -> Command {
    Command::new("export")
        .about("Export a CurseForge-compatible modpack ZIP")
        .arg(Arg::new("instance").help("Instance name"))
        .arg(
            Arg::new("directory")
                .long("directory")
                .requires_all(["minecraft", "loader"])
                .help("Standalone Minecraft game folder"),
        )
        .group(
            ArgGroup::new("source")
                .args(["instance", "directory"])
                .required(true),
        )
        .arg(Arg::new("output").long("output").short('o').required(true))
        .arg(Arg::new("name").long("name"))
        .arg(Arg::new("version").long("version").default_value("1.0.0"))
        .arg(Arg::new("author").long("author").default_value(""))
        .arg(
            Arg::new("minecraft")
                .long("minecraft")
                .requires("directory"),
        )
        .arg(
            Arg::new("loader")
                .long("loader")
                .requires("directory")
                .value_parser(["vanilla", "forge", "fabric", "neoforge", "quilt"]),
        )
        .arg(
            Arg::new("loader-version")
                .long("loader-version")
                .requires("directory"),
        )
        .arg(
            Arg::new("include")
                .long("include")
                .action(ArgAction::Append)
                .help("Additional game-relative file or folder; repeatable"),
        )
        .arg(
            Arg::new("bundle-unmatched")
                .long("bundle-unmatched")
                .action(ArgAction::SetTrue)
                .help("Bundle custom/unmatched mods in overrides"),
        )
}

pub async fn handle_export(matches: &ArgMatches) -> Result<(), Box<dyn std::error::Error>> {
    let mut lock = None;
    let (root, name, game_version, loader, loader_version) = if let Some(directory) =
        matches.get_one::<String>("directory")
    {
        let game_version = matches
            .get_one::<String>("minecraft")
            .ok_or("--directory requires --minecraft")?
            .clone();
        let loader = super::instance::parse_loader(super::utils::required_arg(matches, "loader")?)?;
        let root = crate::config::settings::resolve_path(directory);
        let name = root
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("Modpack")
            .to_owned();
        (
            root,
            name,
            game_version,
            loader,
            matches.get_one::<String>("loader-version").cloned(),
        )
    } else {
        let name = super::utils::required_arg(matches, "instance")?;
        let settings = crate::config::SETTINGS.read();
        let manager = InstanceManager::new(
            settings.paths.resolve_instances_dir(),
            settings.paths.resolve_meta_dir(),
        );
        drop(settings);
        lock = Some(crate::instance::runtime::lock_instance(
            &manager.instances_dir,
            name,
        )?);
        let config = manager.load_one(name)?;
        (
            crate::storage::InstancePaths::new(manager.instances_dir.join(name)).minecraft(),
            config.name,
            config.game_version,
            config.loader,
            config.loader_version,
        )
    };
    let options = ExportOptions {
        name: matches.get_one::<String>("name").cloned().unwrap_or(name),
        version: super::utils::required_arg(matches, "version")?.to_owned(),
        author: super::utils::required_arg(matches, "author")?.to_owned(),
        game_version,
        loader,
        loader_version,
        include: matches
            .get_many::<String>("include")
            .into_iter()
            .flatten()
            .map(std::path::PathBuf::from)
            .collect(),
        bundle_unmatched: matches.get_flag("bundle-unmatched"),
    };
    let output =
        crate::config::settings::resolve_path(super::utils::required_arg(matches, "output")?);
    println!(
        "Exporting '{}': identifying mods and packaging selected files...",
        options.name
    );
    export_curseforge(&root, &output, &options)
        .await
        .map_err(|error| error.to_string())?;
    drop(lock);
    println!("Exported '{}' to {}", options.name, output.display());
    Ok(())
}
