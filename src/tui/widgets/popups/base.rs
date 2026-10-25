// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use ratatui::{
    buffer::{Buffer, CellDiffOption},
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Widget},
};

use crate::config::{settings::ShortcutHintScope, theme::BORDER_STYLE};

type ContentFn<'a> = Box<dyn Fn(Rect, &mut Buffer) + 'a>;

pub struct PopupFrame<'a> {
    pub title: Line<'a>,
    pub border_color: Color,
    pub bg: Option<Color>,
    pub keybinds: Option<Line<'a>>,
    pub search_line: Option<Line<'a>>,
    pub content: ContentFn<'a>,
}

impl<'a> Widget for PopupFrame<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let covered_image_cells = area
            .rows()
            .flat_map(|row| row.columns())
            .filter(|position| {
                buf.cell(*position).is_some_and(|cell| {
                    matches!(cell.diff_option, CellDiffOption::Skip)
                        || matches!(cell.diff_option, CellDiffOption::ForcedWidth(_))
                            && cell.symbol().contains('\x1b')
                })
            })
            .collect::<Vec<_>>();

        Clear.render(area, buf);

        if let Some(bg) = self.bg {
            buf.set_style(area, Style::default().bg(bg));
        }

        let mut block = Block::bordered()
            .title_top(self.title)
            .border_type(BORDER_STYLE.to_border_type())
            .border_style(Style::default().fg(self.border_color));

        if let Some(sl) = self.search_line {
            block = block.title_top(sl.alignment(Alignment::Right));
        }

        if let Some(kb) = self.keybinds
            && super::shortcut_hints_visible(ShortcutHintScope::Popups)
        {
            block = block.title_bottom(kb.alignment(Alignment::Right));
        }

        let inner = block.inner(area);
        block.render(area, buf);
        (self.content)(inner, buf);

        // External image protocols can repaint unchanged terminal cells. Ratatui's
        // AlwaysUpdate option keeps only the popup cells covering an image above it.
        for position in covered_image_cells {
            if let Some(cell) = buf.cell_mut(position)
                && !matches!(
                    cell.diff_option,
                    CellDiffOption::Skip | CellDiffOption::ForcedWidth(_)
                )
            {
                cell.set_diff_option(CellDiffOption::AlwaysUpdate);
            }
        }
    }
}

pub fn summary_heading(title: &str) -> Line<'static> {
    let theme = crate::config::theme::THEME.as_ref();
    Line::styled(
        format!("  {title}"),
        Style::default()
            .fg(theme.text())
            .add_modifier(Modifier::BOLD),
    )
}

pub fn summary_row<'a>(label: &str, value: impl Into<Line<'a>>) -> Line<'a> {
    let theme = crate::config::theme::THEME.as_ref();
    let mut spans = vec![Span::styled(
        format!("  {label:<18}"),
        Style::default().fg(theme.text_dim()),
    )];
    spans.extend(value.into().spans);
    Line::from(spans).style(Style::default().fg(theme.text()))
}

#[cfg(test)]
#[path = "../../tests/widgets/popups/base.rs"]
mod tests;
