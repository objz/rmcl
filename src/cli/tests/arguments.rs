// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::build_command;

#[test]
fn parses_instance_list_subcommand() {
    let matches = build_command()
        .try_get_matches_from(["rmcl", "instance", "list"])
        .expect("command should parse");
    let instance = matches
        .subcommand_matches("instance")
        .expect("instance subcommand should be present");
    assert_eq!(
        instance.subcommand_name(),
        Some("list"),
        "instance list should resolve to the list subcommand"
    );
}

#[test]
fn exports_accept_each_format_and_preserve_curseforge_default() {
    for format in [None, Some("curseforge"), Some("mrpack"), Some("mmc")] {
        let mut args = vec!["rmcl", "export", "Example", "--output", "pack.zip"];
        if let Some(format) = format {
            args.extend(["--format", format]);
        }
        let matches = build_command().try_get_matches_from(args).unwrap();
        assert_eq!(
            matches
                .subcommand_matches("export")
                .unwrap()
                .get_one::<String>("format")
                .map(String::as_str),
            Some(format.unwrap_or("curseforge"))
        );
    }
    assert!(
        build_command()
            .try_get_matches_from([
                "rmcl", "export", "Example", "--output", "pack.zip", "--format", "unknown"
            ])
            .is_err()
    );
}
