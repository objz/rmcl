// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// viewing minecraft log files from the CLI, with optional `--follow` for tailing
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::ArgMatches;

use crate::cli::output::print_table;

type CliResult = Result<(), Box<dyn std::error::Error>>;

pub async fn handle_log(matches: &ArgMatches) -> CliResult {
    match matches.subcommand() {
        Some(("list", sub_matches)) => list_logs(required_arg(sub_matches, "instance")?),
        Some(("show", sub_matches)) => show_log(sub_matches).await,
        _ => Ok(()),
    }
}

fn list_logs(instance: &str) -> CliResult {
    let instances_dir = crate::config::SETTINGS.read().paths.resolve_instances_dir();
    require_instance(&instances_dir, instance)?;
    let rows = crate::instance::logs::files::scan_log_files(&instances_dir, instance)
        .into_iter()
        .map(|entry| {
            let size = std::fs::metadata(&entry.path)
                .map(|meta| meta.len())
                .unwrap_or(0);
            vec![entry.name, size.to_string()]
        })
        .collect::<Vec<_>>();

    print_table(&["File", "Size"], &rows);
    Ok(())
}

async fn show_log(matches: &ArgMatches) -> CliResult {
    let instance = required_arg(matches, "instance")?;
    let file = matches.get_one::<String>("file").map(String::as_str);
    let follow = matches.get_flag("follow");
    let instances_dir = crate::config::SETTINGS.read().paths.resolve_instances_dir();
    require_instance(&instances_dir, instance)?;
    let path = resolve_log_path(&instances_dir, instance, file)?;

    let lines = crate::instance::logs::files::read_log_file(&path);
    for line in &lines {
        println!("{}", line);
    }

    // ghetto tail -f: re-read the whole file and print new lines.
    // not efficient, but log files are small and this is simple.
    if follow {
        let mut last_len = lines.len();
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            let new_lines = crate::instance::logs::files::read_log_file(&path);
            for line in new_lines.iter().skip(last_len) {
                println!("{}", line);
            }
            last_len = new_lines.len();
        }
    }

    Ok(())
}

// if no file is specified, grab the most recent log (first from the sorted scan)
pub(crate) fn resolve_log_path(
    instances_dir: &Path,
    instance: &str,
    file: Option<&str>,
) -> Result<PathBuf, io::Error> {
    if let Some(name) = file {
        let path = crate::instance::logs::files::log_dir(instances_dir, instance).join(name);
        if !path.exists() {
            return Err(io::Error::other(format!("log '{}' not found", name)));
        }
        return Ok(path);
    }

    crate::instance::logs::files::scan_log_files(instances_dir, instance)
        .into_iter()
        .next()
        .map(|entry| entry.path)
        .ok_or_else(|| io::Error::other(format!("no log files found for '{}'", instance)))
}

use super::utils::{require_instance, required_arg};

#[cfg(test)]
#[path = "tests/log.rs"]
mod tests;
