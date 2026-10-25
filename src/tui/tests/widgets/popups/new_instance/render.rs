// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;
use crate::tests::TEST_LOCK;
use ratatui::Terminal;
use ratatui::backend::TestBackend;

// Render re-acquires WIZARD_STATE, so TEST_LOCK guards setup and rendering together.
fn reset_wizard_state(step: WizardStep) {
    let mut guard = WIZARD_STATE.lock().expect("WIZARD_STATE lock");
    *guard = WizardState::default();
    guard.step = step;
}

#[test]
fn new_instance_renders_name_step() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_wizard_state(WizardStep::Name);

    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render(f, f.area(), FocusedArea::Popup))
        .unwrap();
    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn new_instance_renders_loader_step() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_wizard_state(WizardStep::Loader);

    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render(f, f.area(), FocusedArea::Popup))
        .unwrap();
    insta::assert_snapshot!(terminal.backend());
}

// Loaded fixtures keep rendering from spawning network tasks.
#[test]
fn new_instance_renders_version_step() {
    use crate::instance::loader::GameVersion;

    let _serial = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    {
        let mut guard = WIZARD_STATE.lock().expect("WIZARD_STATE lock");
        *guard = WizardState::default();
        guard.step = WizardStep::Version;
        guard.versions = LoadState::Loaded(vec![
            GameVersion {
                id: "1.20.1".into(),
                stable: true,
            },
            GameVersion {
                id: "1.19.4".into(),
                stable: true,
            },
            GameVersion {
                id: "1.18.2".into(),
                stable: true,
            },
        ]);
    }

    let backend = TestBackend::new(60, 14);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render(f, f.area(), FocusedArea::Popup))
        .unwrap();
    let cells = terminal.backend().buffer().content();
    let marker = cells.iter().position(|cell| cell.symbol() == "▶").unwrap();
    assert_eq!(cells[marker + 2].fg, THEME.as_ref().text());
    assert!(cells[marker + 2].modifier.contains(Modifier::BOLD));
    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn new_instance_renders_loader_version_step() {
    use crate::instance::loader::GameVersion;

    let _serial = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    {
        let mut guard = WIZARD_STATE.lock().expect("WIZARD_STATE lock");
        *guard = WizardState::default();
        guard.step = WizardStep::LoaderVersion;
        guard.loader_idx = 2; // Forge
        guard.versions = LoadState::Loaded(vec![GameVersion {
            id: "1.20.1".into(),
            stable: true,
        }]);
        guard.loader_versions =
            LoadState::Loaded(vec!["47.2.0".into(), "47.1.0".into(), "47.0.50".into()]);
    }

    let backend = TestBackend::new(60, 14);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render(f, f.area(), FocusedArea::Popup))
        .unwrap();
    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn new_instance_renders_confirm_step() {
    use crate::instance::loader::GameVersion;
    use tui_prompts::TextState;

    let _serial = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    {
        let mut guard = WIZARD_STATE.lock().expect("WIZARD_STATE lock");
        *guard = WizardState::default();
        guard.step = WizardStep::Confirm;
        guard.loader_idx = 1; // Fabric
        guard.versions = LoadState::Loaded(vec![GameVersion {
            id: "1.20.1".into(),
            stable: true,
        }]);
        guard.loader_versions = LoadState::Loaded(vec!["0.15.0".into()]);
        guard.name_state = TextState::new().with_value("MyPack");
    }

    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render(f, f.area(), FocusedArea::Popup))
        .unwrap();
    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn new_instance_confirmation_area_fits_its_summary() {
    let _serial = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    reset_wizard_state(WizardStep::Confirm);

    assert_eq!(popup_rect(Rect::new(0, 0, 100, 30)).height, 9);
}
