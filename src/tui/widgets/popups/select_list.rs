// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// Shared selection list used by the new-instance wizard and settings pickers.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
    widgets::{List, ListItem, ListState, StatefulWidget},
};

use crate::config::theme::THEME;
use crate::instance::models::ModLoader;

pub(crate) const MOD_LOADERS: [ModLoader; 5] = [
    ModLoader::Vanilla,
    ModLoader::Fabric,
    ModLoader::Forge,
    ModLoader::NeoForge,
    ModLoader::Quilt,
];

pub(crate) fn render(items: Vec<ListItem<'_>>, selected: usize, area: Rect, buffer: &mut Buffer) {
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .fg(THEME.as_ref().accent())
                .bg(THEME.as_ref().stripe())
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(Span::styled(
            "▶ ",
            Style::default()
                .fg(THEME.as_ref().accent())
                .add_modifier(Modifier::BOLD),
        ));
    let mut state = ListState::default().with_selected(Some(selected));
    StatefulWidget::render(list, area, buffer, &mut state);
}

pub(crate) fn render_styled(
    items: Vec<ListItem<'_>>,
    selected: usize,
    area: Rect,
    buffer: &mut Buffer,
) {
    let theme = THEME.as_ref();
    let items = items
        .into_iter()
        .enumerate()
        .map(|(index, item)| {
            if index == selected {
                item.style(Style::default().bg(theme.stripe()))
            } else {
                item
            }
        })
        .collect::<Vec<_>>();
    let list = List::new(items)
        .highlight_style(Style::default().add_modifier(Modifier::BOLD))
        .highlight_symbol(Span::styled(
            "▶ ",
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        ));
    let mut state = ListState::default().with_selected(Some(selected));
    StatefulWidget::render(list, area, buffer, &mut state);
}

#[cfg(test)]
mod tests {
    use ratatui::text::Line;

    use super::*;
    use crate::tui::widgets::status_badge;

    #[test]
    fn styled_list_preserves_badges_on_the_selected_row() {
        let theme = THEME.as_ref();
        let area = Rect::new(0, 0, 30, 1);
        let mut buffer = Buffer::empty(area);
        let badge = status_badge("Auto", theme.success());
        let items = vec![ListItem::new(Line::from(vec![Span::raw("Java  "), badge]))];

        render_styled(items, 0, area, &mut buffer);

        let badge_cell = buffer.cell((8, 0)).unwrap();
        assert_eq!(badge_cell.bg, theme.success());
        assert_eq!(badge_cell.fg, theme.background());
    }
}
