// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::popup_area;
use crate::config::SETTINGS;
use ratatui::layout::Rect;

fn frame() -> Rect {
    Rect::new(0, 0, 80, 24)
}

#[test]
fn returns_none_after_dismiss_timeout() {
    let past_dismiss = SETTINGS.read().ui.error_auto_dismiss_ms as u128 + 1;
    assert!(popup_area(frame(), "msg", 0, past_dismiss).is_none());
}

#[test]
fn returns_none_after_slide_out_finishes() {
    let settings = SETTINGS.read();
    let after_slide = settings
        .ui
        .error_slide_start_ms
        .saturating_add(settings.ui.error_fly_out_ms) as u128;
    drop(settings);

    assert!(popup_area(frame(), "msg", 0, after_slide).is_none());
}

#[test]
fn returns_some_inside_dismiss_window() {
    assert!(popup_area(frame(), "msg", 0, 0).is_some());
}

#[test]
fn returns_none_when_vertical_room_too_small() {
    // base_y = 22 leaves only height 24 - 22 - 1 = 1 row of usable space,
    // less than the minimum 3 needed for border + content + border.
    assert!(popup_area(frame(), "msg", 22, 0).is_none());
}

#[test]
fn clamps_popup_width_to_frame() {
    // a wider-than-the-frame message gets clamped so the popup fits
    // inside frame.width minus the right-edge padding (saturating_sub(4)).
    let huge = "x".repeat(200);
    let area = popup_area(frame(), &huge, 0, 0).unwrap();
    assert!(area.width <= frame().width.saturating_sub(4));
}

#[test]
fn anchors_popup_to_right_edge() {
    let area = popup_area(frame(), "msg", 0, 0).unwrap();
    // popup_w is added to base_x to reach frame.width - 2 (right-edge gutter)
    assert_eq!(area.x + area.width + 2, frame().width);
}
