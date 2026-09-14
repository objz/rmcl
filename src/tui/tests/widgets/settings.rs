// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn removing_selected_last_profile_clamps_selection() {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = SettingsState::new(tmp.path().to_path_buf());
    state.profiles = vec!["first".to_string(), "second".to_string()];
    state.active_profile = Some("second".to_string());
    state.list_state.selected = Some(2);

    state.remove_profile("second");

    assert_eq!(state.profiles, vec!["first"]);
    assert_eq!(state.active_profile, None);
    assert_eq!(state.list_state.selected, Some(1));
}

#[test]
fn info_pane_does_not_bind_desktop_toggle() {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = SettingsState::new(tmp.path().to_path_buf());
    state.pane = SettingsPane::Info;

    assert!(matches!(
        handle_key(&KeyEvent::from(KeyCode::Char('d')), &mut state, None),
        SettingsAction::None
    ));
}
