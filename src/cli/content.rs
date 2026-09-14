// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// unified content management for mods, resource packs, and shaders.
// all three content types share the same list/enable/disable workflow,
// just with different scanner functions plugged in.
use std::io;
use std::path::Path;

use clap::ArgMatches;

use super::utils::{require_instance, required_arg};
use crate::cli::output::print_table;
use crate::instance::content::entry::ContentEntry;

type CliResult = Result<(), Box<dyn std::error::Error>>;
type Scanner = fn(&Path, &str) -> Vec<ContentEntry>;

pub fn handle_mod(matches: &ArgMatches) -> CliResult {
    handle_content(matches, "mod", crate::instance::content::mods::scan_mods)
}

pub fn handle_pack(matches: &ArgMatches) -> CliResult {
    handle_content(
        matches,
        "pack",
        crate::instance::content::resource_packs::scan_resource_packs,
    )
}

pub fn handle_shader(matches: &ArgMatches) -> CliResult {
    handle_content(
        matches,
        "shader",
        crate::instance::content::shaders::scan_shaders,
    )
}

fn handle_content(matches: &ArgMatches, kind: &str, scan: Scanner) -> CliResult {
    match matches.subcommand() {
        Some(("list", sub_matches)) => list_entries(required_arg(sub_matches, "instance")?, scan),
        Some(("enable", sub_matches)) => toggle_entry(
            required_arg(sub_matches, "instance")?,
            required_arg(sub_matches, kind)?,
            true,
            kind,
            scan,
        ),
        Some(("disable", sub_matches)) => toggle_entry(
            required_arg(sub_matches, "instance")?,
            required_arg(sub_matches, kind)?,
            false,
            kind,
            scan,
        ),
        _ => Ok(()),
    }
}

// match by filename stem (no extension) so users don't have to type ".jar"
pub(crate) fn find_entry_by_stem<'a>(
    entries: &'a [ContentEntry],
    target: &str,
) -> Option<&'a ContentEntry> {
    entries
        .iter()
        .find(|entry| entry.file_stem.eq_ignore_ascii_case(target))
}

fn list_entries(instance: &str, scan: Scanner) -> CliResult {
    let instances_dir = crate::config::SETTINGS.read().paths.resolve_instances_dir();
    require_instance(&instances_dir, instance)?;
    let rows = scan(&instances_dir, instance)
        .into_iter()
        .map(|entry| {
            vec![
                entry.name,
                if entry.enabled {
                    "enabled".to_string()
                } else {
                    "disabled".to_string()
                },
            ]
        })
        .collect::<Vec<_>>();

    print_table(&["Name", "State"], &rows);
    Ok(())
}

fn toggle_entry(
    instance: &str,
    target: &str,
    should_enable: bool,
    kind: &str,
    scan: Scanner,
) -> CliResult {
    let instances_dir = crate::config::SETTINGS.read().paths.resolve_instances_dir();
    require_instance(&instances_dir, instance)?;
    let entries = scan(&instances_dir, instance);
    let entry = find_entry_by_stem(&entries, target)
        .ok_or_else(|| io::Error::other(format!("{} '{}' not found", kind, target)))?;

    if entry.enabled == should_enable {
        println!(
            "Already {}d.",
            if should_enable { "enable" } else { "disable" }
        );
        return Ok(());
    }

    crate::instance::content::entry::toggle_entry(entry)?;
    println!(
        "{}d '{}'.",
        if should_enable { "Enable" } else { "Disable" },
        entry.name
    );
    Ok(())
}

#[cfg(test)]
#[path = "tests/content.rs"]
mod tests;
