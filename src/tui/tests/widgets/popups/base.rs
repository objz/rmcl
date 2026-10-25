// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use std::num::NonZeroU16;

use super::*;

#[test]
fn summary_columns_align_and_preserve_badge_styles() {
    let theme = crate::config::theme::THEME.as_ref();
    let badge = crate::tui::widgets::status_badge(".zip", theme.text_dim());
    let rows = [
        summary_row("Format", Line::from(badge)),
        summary_row("Pack name", "Example"),
    ];
    let mut buffer = Buffer::empty(Rect::new(0, 0, 50, 3));
    use ratatui::widgets::Paragraph;
    Paragraph::new(vec![
        summary_heading("Pack"),
        rows[0].clone(),
        rows[1].clone(),
    ])
    .render(buffer.area, &mut buffer);
    assert_eq!(buffer[(2, 0)].symbol(), "P");
    assert!(buffer[(2, 0)].modifier.contains(Modifier::BOLD));
    assert_eq!(buffer[(20, 2)].symbol(), "E");
    assert_eq!(buffer[(21, 1)].symbol(), ".");
    assert_eq!(buffer[(21, 1)].bg, theme.text_dim());
    assert_eq!(buffer[(21, 1)].fg, theme.background());
}

#[test]
fn popup_cells_covering_terminal_images_are_always_redrawn() {
    let area = Rect::new(0, 0, 4, 3);
    let mut buffer = Buffer::empty(area);
    buffer[(1, 1)].set_diff_option(CellDiffOption::Skip);
    buffer[(2, 1)]
        .set_symbol("\x1b_Gimage\x1b\\")
        .set_diff_option(CellDiffOption::ForcedWidth(NonZeroU16::new(1).unwrap()));
    let popup = PopupFrame {
        title: Line::from("Popup"),
        border_color: Color::White,
        bg: Some(Color::Black),
        keybinds: None,
        search_line: None,
        content: Box::new(|_, _| {}),
    };

    popup.render(area, &mut buffer);

    assert_eq!(buffer[(1, 1)].diff_option, CellDiffOption::AlwaysUpdate);
    assert_eq!(buffer[(2, 1)].diff_option, CellDiffOption::AlwaysUpdate);
    assert_eq!(buffer[(0, 1)].diff_option, CellDiffOption::None);
}
