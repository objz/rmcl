// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn review_groups_pack_runtime_and_files_without_losing_versions() {
    let summary = crate::instance::import::ImportSummary {
        name: "Example Pack".into(),
        pack_version: "2.0".into(),
        game_version: "1.21.1".into(),
        loader: crate::instance::models::ModLoader::Fabric,
        loader_version: Some("0.16.0".into()),
        mod_count: 42,
        override_count: 3,
        format: crate::instance::import::PackFormat::Mrpack,
        archive_path: "pack.mrpack".into(),
        source: None,
    };
    let lines = review_lines(&summary, "1.0", "2.0");
    assert_eq!(lines.len(), 11);
    for (index, title) in [(0, "Pack"), (4, "Runtime"), (8, "Files")] {
        assert_eq!(lines[index].to_string(), format!("  {title}"));
    }
    let text = lines
        .iter()
        .map(Line::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    for value in [
        "Example Pack",
        "1.0 → 2.0",
        "1.21.1",
        "Fabric 0.16.0",
        "42",
        "3",
    ] {
        assert!(text.contains(value));
    }
    let summary = Paragraph::new(lines).wrap(Wrap { trim: false });
    assert_eq!(summary.line_count(66), 11);
}
