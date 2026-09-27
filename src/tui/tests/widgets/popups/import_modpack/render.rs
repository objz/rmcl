// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;
use crate::tests::TEST_LOCK;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

// serialise against parallel tests of the same global IMPORT_STATE.
fn reset_import_state(step: ImportStep) {
    let mut guard = IMPORT_STATE.lock().expect("IMPORT_STATE lock");
    *guard = ImportWizardState::default();
    guard.step = step;
}

#[test]
fn import_modpack_renders_input_step() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_import_state(ImportStep::Input);

    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render(f, f.area(), FocusedArea::ImportPopup))
        .unwrap();
    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn modpack_discovery_renders_active_search() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_import_state(ImportStep::Discover);
    {
        let mut state = DISCOVERY_STATE.lock().expect("DISCOVERY_STATE lock");
        *state = crate::tui::widgets::content::DiscoveryState::new_modpacks();
        state.search.activate();
        for character in "fabric".chars() {
            state.search.push(character);
        }
    }

    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    let picker = ratatui_image::picker::Picker::halfblocks();
    terminal
        .draw(|frame| render_with_picker(frame, frame.area(), FocusedArea::ImportPopup, &picker))
        .unwrap();

    let rendered = format!("{}", terminal.backend());
    assert!(rendered.contains("/ fabric\u{2588}"));
    assert!(rendered.contains("[v] versions"));
    assert!(rendered.contains("[i] import"));
    assert_eq!(
        terminal.backend().buffer().cell((1, 1)).unwrap().bg,
        crate::config::theme::THEME.as_ref().surface()
    );
}

#[test]
fn modpack_project_page_only_shows_page_actions() {
    let hints = discovery_keybinds(true, false, false);

    assert!(hints.contains(&("v", " versions")));
    assert!(!hints.iter().any(|(_, action)| *action == " pages"));
    assert!(!hints.iter().any(|(_, action)| *action == " search"));
    assert!(!hints.iter().any(|(_, action)| *action == " import"));
}

#[test]
fn modpack_discovery_shows_page_navigation() {
    assert!(discovery_keybinds(false, false, false).contains(&(" [/] ", " pages")));
}

#[test]
fn import_modpack_renders_fetching_step() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_import_state(ImportStep::Fetching);

    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render(f, f.area(), FocusedArea::ImportPopup))
        .unwrap();
    insta::assert_snapshot!(terminal.backend());
}

// Version step: pre-populate versions as LoadState::Loaded with synthetic
// VersionInfo entries so render walks the list path without triggering
// any network helpers.
#[test]
fn import_modpack_renders_version_step() {
    use crate::net::modrinth::VersionInfo;

    let _serial = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    {
        let mut guard = IMPORT_STATE.lock().expect("IMPORT_STATE lock");
        *guard = ImportWizardState::default();
        guard.step = ImportStep::Version;
        guard.project_title = Some("Synthetic Pack".into());
        guard.versions = LoadState::Loaded(vec![
            VersionInfo {
                id: "v1".into(),
                project_id: "project".into(),
                name: "1.0.0".into(),
                version_number: "1.0.0".into(),
                game_versions: vec!["1.20.1".into()],
                loaders: vec!["fabric".into()],
                version_type: crate::net::modrinth::VersionType::Release,
                dependencies: Vec::new(),
                date_published: String::new(),
                files: vec![],
            },
            VersionInfo {
                id: "v2".into(),
                project_id: "project".into(),
                name: "0.9.0".into(),
                version_number: "0.9.0".into(),
                game_versions: vec!["1.20.1".into()],
                loaders: vec!["fabric".into()],
                version_type: crate::net::modrinth::VersionType::Release,
                dependencies: Vec::new(),
                date_published: String::new(),
                files: vec![],
            },
        ]);
    }

    let backend = TestBackend::new(60, 14);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render(f, f.area(), FocusedArea::ImportPopup))
        .unwrap();
    insta::assert_snapshot!(terminal.backend());
}

// Confirm step: needs a populated ImportSummary so the render path
// doesn't bail. ImportSummary is constructed manually with synthetic
// values; archive_path is a fake tempdir-ish path that never gets read.
#[test]
fn import_modpack_renders_confirm_step() {
    use crate::instance::import::{ImportSummary, PackFormat};
    use crate::instance::models::ModLoader;
    use std::path::PathBuf;

    let _serial = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    {
        let mut guard = IMPORT_STATE.lock().expect("IMPORT_STATE lock");
        *guard = ImportWizardState::default();
        guard.step = ImportStep::Confirm;
        guard.summary = Some(ImportSummary {
            name: "Synthetic Pack".into(),
            pack_version: "1.0.0".into(),
            game_version: "1.20.1".into(),
            loader: ModLoader::Fabric,
            loader_version: Some("0.15.0".into()),
            mod_count: 42,
            override_count: 3,
            format: PackFormat::Mrpack,
            archive_path: PathBuf::from("/tmp/synthetic.mrpack"),
            source: None,
        });
    }

    let backend = TestBackend::new(60, 14);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render(f, f.area(), FocusedArea::ImportPopup))
        .unwrap();
    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn modpack_confirmation_area_fits_its_summary() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_import_state(ImportStep::Confirm);

    assert_eq!(popup_rect(Rect::new(0, 0, 100, 30)).height, 8);
}

// Confirm step with loader_version=None: covers the branch where the
// pack didn't declare a loader version (rare upstream, but happens for
// older mmc packs). render_confirm_step has to handle the Option.
#[test]
fn import_modpack_renders_confirm_step_without_loader_version() {
    use crate::instance::import::{ImportSummary, PackFormat};
    use crate::instance::models::ModLoader;
    use std::path::PathBuf;

    let _serial = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    {
        let mut guard = IMPORT_STATE.lock().expect("IMPORT_STATE lock");
        *guard = ImportWizardState::default();
        guard.step = ImportStep::Confirm;
        guard.summary = Some(ImportSummary {
            name: "Vanilla Pack".into(),
            pack_version: "2.0".into(),
            game_version: "1.20.1".into(),
            loader: ModLoader::Vanilla,
            loader_version: None,
            mod_count: 0,
            override_count: 12,
            format: PackFormat::Mmc,
            archive_path: PathBuf::from("/tmp/vanilla.zip"),
            source: None,
        });
    }

    let backend = TestBackend::new(60, 14);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render(f, f.area(), FocusedArea::ImportPopup))
        .unwrap();
    insta::assert_snapshot!(terminal.backend());
}
