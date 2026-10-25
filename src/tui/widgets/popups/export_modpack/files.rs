// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::BTreeMap;
use std::path::PathBuf;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, ListState, StatefulWidget},
};

use super::State;
use crate::config::theme::THEME;

#[derive(Default)]
pub(super) struct Browser {
    pub directory: PathBuf,
    pub selected: usize,
}

pub(super) struct Entry {
    pub path: PathBuf,
    pub directory: bool,
    pub total: usize,
    pub included: usize,
}

impl State {
    pub(super) fn file_entries(&self) -> Vec<Entry> {
        let Some(scan) = self.scan.as_ref() else {
            return Vec::new();
        };
        let mut children = BTreeMap::new();
        for path in scan.paths() {
            let Ok(relative) = path.strip_prefix(&self.browser.directory) else {
                continue;
            };
            let Some(component) = relative.components().next() else {
                continue;
            };
            let child = self.browser.directory.join(component.as_os_str());
            let counts = children
                .entry(child.clone())
                .or_insert((path != &child, 0, 0));
            counts.1 += 1;
            counts.2 += usize::from(self.selected_files.contains(path));
        }
        let mut entries = children
            .into_iter()
            .map(|(path, (directory, total, included))| Entry {
                path,
                directory,
                total,
                included,
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            right
                .directory
                .cmp(&left.directory)
                .then_with(|| left.path.cmp(&right.path))
        });
        entries
    }

    pub(super) fn toggle_file(&mut self) {
        let before = self.unmatched_files(&self.selected_files);
        let entries = self.file_entries();
        let adding = entries
            .get(self.browser.selected)
            .is_some_and(|entry| entry.included < entry.total);
        if let Some(Entry {
            path,
            total,
            included,
            ..
        }) = entries.get(self.browser.selected)
        {
            let all_selected = total == included;
            if let Some(scan) = self.scan.as_ref() {
                for file in scan.paths().filter(|file| file.starts_with(path)) {
                    if all_selected {
                        self.selected_files.remove(file);
                    } else {
                        self.selected_files.insert(file.clone());
                    }
                }
            }
        }
        let unidentified = self.referenced_paths().is_none()
            && self
                .scan
                .as_ref()
                .is_some_and(|scan| scan.needs_references(self.format, &self.selected_files));
        if !self
            .unmatched_files(&self.selected_files)
            .is_subset(&before)
            || adding && unidentified
        {
            self.bundle_unmatched = false;
        }
    }

    pub(super) fn enter_directory(&mut self) {
        if let Some(Entry {
            path,
            directory: true,
            ..
        }) = self.file_entries().get(self.browser.selected)
        {
            self.browser.directory = path.clone();
            self.browser.selected = 0;
        }
    }
}

pub(super) fn render(state: &State, area: Rect, buffer: &mut Buffer) {
    let theme = THEME.as_ref();
    let all = state
        .scan
        .as_ref()
        .map_or_else(Default::default, |scan| scan.paths().cloned().collect());
    let unmatched = state.unmatched_files(&all);
    let entries = state
        .file_entries()
        .into_iter()
        .enumerate()
        .map(|(index, entry)| {
            let Entry {
                path,
                directory,
                total,
                included,
            } = entry;
            let check = if included == total {
                "✓"
            } else if included == 0 {
                " "
            } else {
                "−"
            };
            let name = format!(
                "{}{suffix}",
                path.file_name().unwrap_or_default().to_string_lossy(),
                suffix = if directory { "/" } else { "" }
            );
            let has_unmatched = unmatched
                .range(path.clone()..)
                .next()
                .is_some_and(|file| file.starts_with(&path));
            let mut spans = vec![
                Span::styled(
                    format!("[{check}] "),
                    Style::default().fg(if included > 0 {
                        theme.success()
                    } else {
                        theme.text_dim()
                    }),
                ),
                Span::styled(
                    name,
                    Style::default()
                        .fg(if has_unmatched {
                            theme.warning()
                        } else if included > 0 || directory {
                            theme.text()
                        } else {
                            theme.text_dim()
                        })
                        .add_modifier(if directory {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ),
            ];
            if directory {
                let count = Span::styled(
                    format!("{included}/{total}"),
                    Style::default().fg(theme.text_dim()),
                );
                let width = spans.iter().map(Span::width).sum::<usize>();
                let gap = (area.width.saturating_sub(2) as usize)
                    .saturating_sub(width + count.width())
                    .max(2);
                spans.push(Span::raw(" ".repeat(gap)));
                spans.push(count);
            }
            let item = ListItem::new(Line::from(spans));
            if index == state.browser.selected {
                item.style(Style::default().bg(theme.stripe()))
            } else {
                item
            }
        })
        .collect::<Vec<_>>();
    let list = List::new(entries)
        .style(Style::default().fg(theme.text()))
        .highlight_symbol(Span::styled("▌ ", Style::default().fg(theme.accent())))
        .highlight_style(Style::default().add_modifier(Modifier::BOLD));
    let mut selected = ListState::default().with_selected(Some(state.browser.selected));
    StatefulWidget::render(list, area, buffer, &mut selected);
}
