// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use std::path::PathBuf;

use ratatui::{Terminal, backend::TestBackend};

use super::*;

fn entry(name: &str, path: &str) -> ContentEntry {
    ContentEntry {
        file_stem: name.to_lowercase(),
        name: name.to_owned(),
        source_slug: None,
        installed_path: Some(PathBuf::from(path)),
        provider_project: None,
        world_details: None,
        title_suffix: None,
        footer_label: None,
        footer_change: None,
        description: "Existing project description".to_owned(),
        enabled: true,
        icon_bytes: None,
        provider_icon: false,
        provider_description: false,
        path: PathBuf::from(path),
        icon_lines: Some(crate::instance::content::fallback_icon()),
    }
}

fn version(number: &str) -> crate::net::modrinth::VersionInfo {
    crate::net::modrinth::VersionInfo {
        id: number.to_owned(),
        project_id: "project".to_owned(),
        name: number.to_owned(),
        version_number: number.to_owned(),
        game_versions: Vec::new(),
        loaders: Vec::new(),
        version_type: crate::net::modrinth::VersionType::Release,
        dependencies: Vec::new(),
        date_published: String::new(),
        files: Vec::new(),
    }
}

fn plan() -> BulkUpdatePlan {
    BulkUpdatePlan {
        dependency_plan: crate::instance::content::dependencies::DependencyPlan {
            items: Vec::new(),
            root_count: 0,
            optional_dependencies: 0,
        },
        roots: vec![crate::instance::content::updates::PlannedRootUpdate {
            title: "Example Mod".to_owned(),
            installed_path: PathBuf::from("mods/example.jar"),
            current_version: "1.0".to_owned(),
            target: version("2.0"),
        }],
        conflicts: Vec::new(),
    }
}

fn snapshot() -> UpdateSnapshot {
    UpdateSnapshot {
        game_version: "1.21.1".to_owned(),
        loader: crate::instance::ModLoader::Fabric,
        inventory: Vec::new(),
        checked_at: 0,
        updates: Vec::new(),
        failures: Vec::new(),
    }
}

#[test]
fn update_review_reuses_content_rows() {
    let mut state = State::checking(
        "Instance".to_owned(),
        ContentKind::Mod,
        None,
        vec![entry("Example Mod", "mods/example.jar")],
    );
    state.push(PendingResult::Prepared(snapshot(), plan()));
    state.drain();
    let picker = ratatui_image::picker::Picker::halfblocks();
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();

    terminal
        .draw(|frame| render(frame, &mut state, &picker))
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
    assert_eq!(state.list.entries[0].title_suffix, None);
    assert!(!state.list.warning_descriptions);
}

#[test]
fn update_conflicts_explain_why_the_item_was_not_updated() {
    let mut plan = plan();
    plan.conflicts
        .push(crate::instance::content::updates::UpdateConflict {
            title: "Example Mod".to_owned(),
            installed_path: PathBuf::from("mods/example.jar"),
            reason: "Parse error: Conflicting selected versions for 'Library': '1.0' and '2.0'"
                .to_owned(),
        });
    let mut state = State::checking(
        "Instance".to_owned(),
        ContentKind::Mod,
        None,
        vec![entry("Example Mod", "mods/example.jar")],
    );
    state.push(PendingResult::Prepared(snapshot(), plan));
    state.drain();

    assert_eq!(
        state.list.entries[0].title_suffix.as_deref(),
        Some("Skipped")
    );
    assert_eq!(
        state.list.entries[0].description,
        "Other selected updates require different versions of Library"
    );
    assert!(state.list.warning_descriptions);
}

#[test]
fn update_errors_only_show_the_cause() {
    for (error, expected) in [
        (
            "Parse error: No compatible dependency version found for project 'library'",
            "No compatible dependency version found for project 'library'",
        ),
        (
            "Parse error: Conflicting required versions for 'Library': '1.0' and '2.0'",
            "Incompatible required versions of Library",
        ),
        ("Provider unavailable", "Provider unavailable"),
    ] {
        assert_eq!(user_conflict_reason(error), expected);
    }
}
