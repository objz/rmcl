// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// base frame that all popups render inside. handles the border, title bar,
// keybind footer, and optional search indicator. content is injected via closure
// so each popup type only worries about its inner area.

use ratatui::{
    buffer::{Buffer, CellDiffOption},
    layout::{Alignment, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph, Widget},
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

        // clear first so the popup doesn't layer on top of whatever was underneath
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

pub fn render_summary(rows: &[(&str, &str)], area: Rect, buf: &mut Buffer) {
    let theme = crate::config::theme::THEME.as_ref();
    let label_style = Style::default().fg(theme.text_dim());
    let lines = rows
        .iter()
        .map(|(label, value)| {
            Line::from(vec![
                Span::styled(format!("{label}: "), label_style),
                Span::styled(*value, Style::default().fg(theme.text())),
            ])
        })
        .collect::<Vec<_>>();
    Paragraph::new(lines).render(area, buf);
}

#[cfg(test)]
#[path = "../../tests/widgets/popups/base.rs"]
mod tests;
