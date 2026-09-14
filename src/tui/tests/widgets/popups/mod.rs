// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::{
    confirm::{ConfirmTarget, confirm_popup_area},
    keybind_line_fitted, word_wrap_size,
};

#[test]
fn fitted_keybinds_use_terminal_width_and_omit_overflow() {
    let line = keybind_line_fitted(
        crate::config::settings::ShortcutHintScope::Accounts,
        &[("⏎", " select"), ("a", " add")],
        10,
    );

    assert_eq!(line.width(), 10);
    assert_eq!(line.to_string(), "[⏎] select");
}

#[test]
fn word_wrap_size_uses_terminal_width_for_unicode_text() {
    assert_eq!(word_wrap_size("Übergröße", 20), (9, 1));
    assert_eq!(word_wrap_size("世界 test", 6), (4, 2));
    assert_eq!(word_wrap_size("one\n\ntwo", 20), (3, 3));
}

#[test]
fn confirmation_area_uses_display_width_for_unicode_names() {
    let frame = ratatui::layout::Rect::new(0, 0, 100, 30);
    let ascii = ConfirmTarget::Instance {
        name: "a".repeat(30),
    };
    let unicode = ConfirmTarget::Instance {
        name: format!("é{}", "a".repeat(29)),
    };

    assert_eq!(
        confirm_popup_area(frame, &ascii),
        confirm_popup_area(frame, &unicode)
    );
}
