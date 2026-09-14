// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// split-pane log viewer: file list on the left, log content on the right.
// supports live log tailing when the instance is running, plus search
// filtering in both the file list and the viewer pane.
// log scanning runs on a background thread to avoid blocking the UI.

use std::path::Path;
use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use tui_widget_list::{ListBuilder, ListState as TuiListState, ListView};

use crate::config::theme::{BORDER_STYLE, THEME};
use crate::instance::launch::parser::LogLevel;
use crate::instance::logs::files::{LogFileEntry, read_log_file, scan_log_files};

type PendingLogs = Arc<Mutex<Option<(String, Vec<LogFileEntry>)>>>;

pub struct LogsState {
    pub entries: Vec<LogFileEntry>,
    pub list_state: TuiListState,
    pub loaded_for: Option<String>,
    pub loading: bool,
    pub viewer_focused: bool,
    pub viewer_lines: Vec<String>,
    pub viewer_scroll: usize,
    pub viewer_max_scroll: usize,
    pub scrollbar_state: ScrollbarState,
    pub viewer_scrollbar_state: ScrollbarState,
    pub search: super::search::SearchState,
    pub viewer_search: super::search::SearchState,
    selected_path: Option<std::path::PathBuf>,
    pending: PendingLogs,
    last_rescan: std::time::Instant,
    instances_dir_cache: Option<std::path::PathBuf>,
    was_live: bool,
}

impl Default for LogsState {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            list_state: TuiListState::default(),
            loaded_for: None,
            loading: false,
            viewer_focused: false,
            viewer_lines: Vec::new(),
            viewer_scroll: 0,
            viewer_max_scroll: 0,
            scrollbar_state: ScrollbarState::default(),
            viewer_scrollbar_state: ScrollbarState::default(),
            search: super::search::SearchState::default(),
            viewer_search: super::search::SearchState::default(),
            selected_path: None,
            pending: Arc::new(Mutex::new(None)),
            last_rescan: std::time::Instant::now(),
            instances_dir_cache: None,
            was_live: false,
        }
    }
}

impl LogsState {
    pub fn start_load(&mut self, instances_dir: &Path, instance_name: &str) {
        self.loading = true;
        self.loaded_for = Some(instance_name.to_string());
        self.instances_dir_cache = Some(instances_dir.to_path_buf());
        self.entries.clear();
        self.list_state = TuiListState::default();
        self.viewer_lines.clear();
        self.viewer_scroll = 0;
        self.viewer_focused = false;
        self.selected_path = None;
        self.last_rescan = std::time::Instant::now();

        let dir = instances_dir.to_path_buf();
        let tag = instance_name.to_string();
        let pending = self.pending.clone();

        tokio::spawn(async move {
            let scan_dir = dir.clone();
            let scan_name = tag.clone();
            let entries =
                tokio::task::spawn_blocking(move || scan_log_files(&scan_dir, &scan_name))
                    .await
                    .unwrap_or_default();

            if let Ok(mut slot) = pending.lock() {
                *slot = Some((tag, entries));
                crate::feedback::request_redraw();
            }
        });
    }

    pub fn drain_pending(&mut self) {
        let live_now = self.has_live();
        self.was_live = live_now;

        let taken = match self.pending.lock() {
            Ok(mut slot) => slot.take(),
            _ => None,
        };

        if let Some((instance_name, entries)) = taken
            && self.loaded_for.as_deref() == Some(&instance_name)
        {
            let prev_selected = self.list_state.selected;
            self.entries = entries;
            self.loading = false;

            let display_count = self.display_count();

            if display_count > 0 && prev_selected.is_none() {
                self.list_state.selected = Some(0);
                self.load_selected_content();
            } else if let Some(sel) = prev_selected
                && sel >= display_count
                && display_count > 0
            {
                self.list_state.selected = Some(display_count - 1);
            }
            self.update_scrollbar();
        }
    }

    // periodically re-scan log files in case new ones appeared while playing.
    pub fn try_rescan(&mut self) {
        if self.last_rescan.elapsed() < std::time::Duration::from_secs(2) {
            return;
        }
        self.last_rescan = std::time::Instant::now();

        let (Some(dir), Some(name)) = (&self.instances_dir_cache, &self.loaded_for) else {
            return;
        };
        if !matches!(
            crate::instance::runtime::get(name),
            Some(
                crate::instance::runtime::RunState::Authenticating
                    | crate::instance::runtime::RunState::Starting
                    | crate::instance::runtime::RunState::Running
            )
        ) {
            return;
        }

        let dir = dir.clone();
        let tag = name.clone();
        let pending = self.pending.clone();

        tokio::spawn(async move {
            let scan_dir = dir.clone();
            let scan_name = tag.clone();
            let entries =
                tokio::task::spawn_blocking(move || scan_log_files(&scan_dir, &scan_name))
                    .await
                    .unwrap_or_default();

            if let Ok(mut slot) = pending.lock() {
                *slot = Some((tag, entries));
                crate::feedback::request_redraw();
            }
        });
    }

    // when an instance is active or has just crashed, a synthetic "Live" entry
    // is injected at index 0 so parsed live log styling is retained.
    fn has_live(&self) -> bool {
        let name = self.loaded_for.as_deref().unwrap_or("");
        matches!(
            crate::instance::runtime::get(name),
            Some(crate::instance::runtime::RunState::Running)
                | Some(crate::instance::runtime::RunState::Starting)
                | Some(crate::instance::runtime::RunState::Crashed(_))
        )
    }

    fn display_count(&self) -> usize {
        self.display_indices().len()
    }

    fn live_display_name(&self) -> String {
        self.entries
            .first()
            .map(|entry| entry.name.trim_end_matches(".log").to_owned())
            .unwrap_or_else(|| "Live".to_owned())
    }

    fn display_indices(&self) -> Vec<Option<usize>> {
        let mut indices = Vec::new();
        if self.has_live() && self.search.matches(&self.live_display_name()) {
            indices.push(None);
        }
        indices.extend(
            self.entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| self.search.matches(entry.name.trim_end_matches(".log")))
                .map(|(index, _)| Some(index)),
        );
        indices
    }

    fn is_live_selected(&self) -> bool {
        self.list_state
            .selected
            .and_then(|selected| self.display_indices().get(selected).copied())
            == Some(None)
    }

    fn file_index_for_selected(&self) -> Option<usize> {
        let selected = self.list_state.selected?;
        self.display_indices().get(selected).copied().flatten()
    }

    fn load_selected_content(&mut self) {
        if self.is_live_selected() {
            self.selected_path = None;
            self.viewer_lines.clear();
            self.viewer_scroll = 0;
            return;
        }

        let path = self
            .file_index_for_selected()
            .and_then(|i| self.entries.get(i))
            .map(|e| e.path.clone());

        if path == self.selected_path {
            return;
        }
        self.selected_path = path.clone();
        self.viewer_scroll = 0;

        if let Some(path) = path {
            self.viewer_lines = read_log_file(&path);
        } else {
            self.viewer_lines.clear();
        }
    }

    fn update_scrollbar(&mut self) {
        let count = self.display_count();
        let max = count.saturating_sub(1);
        if count == 0 {
            self.list_state.selected = None;
        } else if self
            .list_state
            .selected
            .is_none_or(|selected| selected >= count)
        {
            self.list_state.selected = Some(max);
        }
        let pos = self.list_state.selected.unwrap_or(0);
        self.scrollbar_state = ScrollbarState::new(max).position(pos);
    }

    fn update_viewer_scrollbar(&mut self, visible_height: usize, line_count: usize) {
        self.viewer_max_scroll = line_count.saturating_sub(visible_height);
        if self.viewer_scroll > self.viewer_max_scroll {
            self.viewer_scroll = self.viewer_max_scroll;
        }
        self.viewer_scrollbar_state =
            ScrollbarState::new(self.viewer_max_scroll).position(self.viewer_scroll);
    }

    pub fn pending_delete(
        &self,
    ) -> Option<crate::tui::widgets::content::list::PendingContentDelete> {
        let index = self.file_index_for_selected()?;
        let entry = self.entries.get(index)?;
        Some(crate::tui::widgets::content::list::PendingContentDelete {
            name: entry.name.clone(),
            path: entry.path.clone(),
        })
    }

    pub fn remove_path(&mut self, path: &Path) {
        self.entries.retain(|entry| entry.path != path);
        let display_count = self.display_count();
        if display_count == 0 {
            self.list_state.selected = None;
            self.viewer_focused = false;
            self.selected_path = None;
            self.viewer_lines.clear();
            self.viewer_scroll = 0;
        } else if let Some(sel) = self.list_state.selected {
            self.list_state.selected = Some(sel.min(display_count.saturating_sub(1)));
            self.load_selected_content();
        }
        self.update_scrollbar();
    }
}

pub fn handle_key(key_event: &KeyEvent, state: &mut LogsState) -> bool {
    let shift = key_event.modifiers.contains(KeyModifiers::SHIFT);

    if state.viewer_focused {
        if state.viewer_search.active {
            match key_event.code {
                KeyCode::Enter => {
                    state.viewer_search.confirm();
                    state.viewer_scroll = 0;
                }
                KeyCode::Esc => {
                    state.viewer_search.deactivate();
                    state.viewer_scroll = 0;
                }
                KeyCode::Backspace => {
                    state.viewer_search.backspace(key_event.modifiers);
                    state.viewer_scroll = 0;
                }
                KeyCode::Char(c) => {
                    state.viewer_search.push(c);
                    state.viewer_scroll = 0;
                }
                _ => {}
            }
            return true;
        }

        if key_event.code == KeyCode::Char('/') {
            state.viewer_search.activate();
            state.viewer_scroll = 0;
            return true;
        }

        match key_event.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if state.viewer_scroll < state.viewer_max_scroll {
                    state.viewer_scroll += 1;
                    state.viewer_scrollbar_state =
                        ScrollbarState::new(state.viewer_max_scroll).position(state.viewer_scroll);
                }
                true
            }
            KeyCode::Char('k') | KeyCode::Up => {
                state.viewer_scroll = state.viewer_scroll.saturating_sub(1);
                state.viewer_scrollbar_state =
                    ScrollbarState::new(state.viewer_max_scroll).position(state.viewer_scroll);
                true
            }
            KeyCode::Char('G') => {
                state.viewer_scroll = state.viewer_max_scroll;
                state.viewer_scrollbar_state =
                    ScrollbarState::new(state.viewer_max_scroll).position(state.viewer_scroll);
                true
            }
            KeyCode::Char('g') => {
                state.viewer_scroll = 0;
                state.viewer_scrollbar_state =
                    ScrollbarState::new(state.viewer_max_scroll).position(state.viewer_scroll);
                true
            }
            KeyCode::Esc => {
                state.viewer_focused = false;
                true
            }
            KeyCode::Char('H') | KeyCode::Left if shift => {
                state.viewer_focused = false;
                true
            }
            _ => false,
        }
    } else {
        if state.search.active {
            match key_event.code {
                KeyCode::Enter => {
                    state.search.confirm();
                    state.list_state.selected = Some(0);
                    state.load_selected_content();
                    state.update_scrollbar();
                }
                KeyCode::Esc => {
                    state.search.deactivate();
                    state.list_state.selected = Some(0);
                    state.load_selected_content();
                    state.update_scrollbar();
                }
                KeyCode::Backspace => {
                    state.search.backspace(key_event.modifiers);
                    state.list_state.selected = Some(0);
                    state.load_selected_content();
                    state.update_scrollbar();
                }
                KeyCode::Char(c) => {
                    state.search.push(c);
                    state.list_state.selected = Some(0);
                    state.load_selected_content();
                    state.update_scrollbar();
                }
                _ => {}
            }
            return true;
        }

        if key_event.code == KeyCode::Char('/') {
            state.search.activate();
            state.list_state.selected = Some(0);
            state.update_scrollbar();
            return true;
        }

        let display_count = state.display_count();
        match key_event.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if display_count == 0 {
                    return true;
                }
                let current = state.list_state.selected.unwrap_or(0);
                state.list_state.selected = Some((current + 1).min(display_count - 1));
                state.load_selected_content();
                state.update_scrollbar();
                true
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let current = state.list_state.selected.unwrap_or(0);
                state.list_state.selected = Some(current.saturating_sub(1));
                state.load_selected_content();
                state.update_scrollbar();
                true
            }
            KeyCode::Enter => {
                state.viewer_focused = true;
                true
            }
            KeyCode::Char('L') | KeyCode::Right if shift => {
                state.viewer_focused = true;
                true
            }
            _ => false,
        }
    }
}

pub fn render(frame: &mut Frame, area: Rect, state: &mut LogsState, is_focused: bool) {
    let theme = THEME.as_ref();
    if state.loading {
        frame.render_widget(
            Paragraph::new("Loading logs...").style(Style::default().fg(theme.text_dim())),
            area,
        );
        return;
    }

    let has_live = state.has_live();
    let display_count = state.display_count();

    if display_count == 0 {
        frame.render_widget(
            Paragraph::new("No logs yet.").style(Style::default().fg(theme.text_dim())),
            area,
        );
        return;
    }

    if state.list_state.selected.is_none() && display_count > 0 {
        state.list_state.selected = Some(0);
        state.load_selected_content();
    }

    let [list_area, viewer_area] =
        Layout::horizontal([Constraint::Length(30), Constraint::Min(0)]).areas(area);

    render_list(frame, list_area, state, is_focused);
    render_viewer(frame, viewer_area, state, is_focused, has_live);
}

fn render_list(frame: &mut Frame, area: Rect, state: &mut LogsState, is_focused: bool) {
    let theme = THEME.as_ref();
    let list_focused = is_focused && !state.viewer_focused;
    let border_color = if list_focused {
        theme.accent()
    } else {
        theme.border()
    };

    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_type(BORDER_STYLE.to_border_type())
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let display_count = state.display_count();

    let entries_snapshot: Vec<(String, bool)> = state
        .display_indices()
        .into_iter()
        .map(|index| match index {
            Some(index) => (
                state.entries[index]
                    .name
                    .trim_end_matches(".log")
                    .to_owned(),
                false,
            ),
            None => (state.live_display_name(), true),
        })
        .collect();
    let search = &state.search;
    let success = theme.success();
    let accent = theme.accent();
    let text = theme.text();
    let background = theme.background();
    let stripe = theme.stripe();

    let builder = ListBuilder::new(move |context| {
        let (name, is_live) = &entries_snapshot[context.index];
        let show_selected = list_focused && context.is_selected;

        let style = if *is_live && show_selected {
            Style::default().fg(success).add_modifier(Modifier::BOLD)
        } else if *is_live {
            Style::default().fg(success)
        } else if show_selected {
            Style::default().fg(accent).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(text)
        };

        let bg = if context.index % 2 == 0 {
            background
        } else {
            stripe
        };

        let selector = if show_selected {
            Span::styled("\u{258c} ", Style::default().fg(accent))
        } else {
            Span::raw("  ")
        };
        let mut spans = vec![selector];
        spans.extend(search.highlight_spans(name, style));
        let item = ratatui::text::Text::from(Line::from(spans)).style(Style::default().bg(bg));
        (item, 1)
    });

    let list = ListView::new(builder, display_count);
    frame.render_stateful_widget(list, inner, &mut state.list_state);

    let scrollbar_area = Rect {
        x: inner.x + inner.width.saturating_sub(0),
        y: inner.y + 1,
        width: 1,
        height: inner.height.saturating_sub(2),
    };
    frame.render_stateful_widget(
        Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("\u{25b2}"))
            .style(
                Style::default()
                    .fg(theme.accent())
                    .add_modifier(Modifier::BOLD),
            )
            .thumb_symbol("\u{2551}")
            .track_symbol(Some(""))
            .end_symbol(Some("\u{25bc}")),
        scrollbar_area,
        &mut state.scrollbar_state,
    );
}

fn render_viewer(
    frame: &mut Frame,
    area: Rect,
    state: &mut LogsState,
    _is_focused: bool,
    has_live: bool,
) {
    let theme = THEME.as_ref();
    let is_live = has_live && state.list_state.selected == Some(0);

    let all_lines: Vec<ViewerLine> = if is_live {
        let name = state.loaded_for.as_deref().unwrap_or("");
        crate::instance::logs::live::get_entries(name)
            .into_iter()
            .map(|line| ViewerLine {
                text: line.text,
                level: Some(line.level),
            })
            .collect()
    } else {
        state
            .viewer_lines
            .iter()
            .cloned()
            .map(|text| ViewerLine { text, level: None })
            .collect()
    };

    let lines: Vec<&ViewerLine> = all_lines
        .iter()
        .filter(|l| state.viewer_search.matches(&l.text))
        .collect();

    let visible_height = area.height as usize;
    // auto-scroll: if the user was already at the bottom, keep following
    // new lines as they come in (like `tail -f` behavior)
    let was_at_bottom = state.viewer_scroll >= state.viewer_max_scroll.saturating_sub(1);
    state.update_viewer_scrollbar(visible_height, lines.len());

    if is_live && was_at_bottom && !state.viewer_search.active {
        state.viewer_scroll = state.viewer_max_scroll;
        state.viewer_scrollbar_state =
            ScrollbarState::new(state.viewer_max_scroll).position(state.viewer_scroll);
    }

    if lines.is_empty() {
        return;
    }

    let search = &state.viewer_search;
    let styled_lines: Vec<Line> = lines
        .iter()
        .skip(state.viewer_scroll)
        .take(visible_height)
        .map(|line| {
            search.highlight_line(
                &line.text,
                line.level
                    .map(log_level_style)
                    .unwrap_or_else(|| line_level_style(&line.text)),
            )
        })
        .collect();

    frame.render_widget(Paragraph::new(styled_lines), area);

    let scrollbar_area = Rect {
        x: area.x + area.width.saturating_sub(0),
        y: area.y + 1,
        width: 1,
        height: area.height.saturating_sub(2),
    };
    frame.render_stateful_widget(
        Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("\u{25b2}"))
            .style(
                Style::default()
                    .fg(theme.accent())
                    .add_modifier(Modifier::BOLD),
            )
            .thumb_symbol("\u{2551}")
            .track_symbol(Some(""))
            .end_symbol(Some("\u{25bc}")),
        scrollbar_area,
        &mut state.viewer_scrollbar_state,
    );
}

struct ViewerLine {
    text: String,
    level: Option<LogLevel>,
}

fn log_level_style(level: LogLevel) -> Style {
    let theme = THEME.as_ref();
    match level {
        LogLevel::Error => Style::default().fg(theme.error()),
        LogLevel::Warn => Style::default().fg(theme.warning()),
        LogLevel::Debug | LogLevel::Trace => Style::default().fg(theme.text_dim()),
        LogLevel::Info => Style::default().fg(theme.text()),
    }
}

// color-code log lines by severity so errors actually stand out
// instead of drowning in a wall of white text
fn line_level_style(line: &str) -> Style {
    let theme = THEME.as_ref();
    let upper = line.to_uppercase();
    if upper.contains("ERROR") || upper.contains("FATAL") || upper.contains("[STDERR]") {
        Style::default().fg(theme.error())
    } else if upper.contains("WARN") {
        Style::default().fg(theme.warning())
    } else if upper.contains("DEBUG") || upper.contains("TRACE") {
        Style::default().fg(theme.text_dim())
    } else {
        Style::default().fg(theme.text())
    }
}

#[cfg(test)]
#[path = "../tests/widgets/logs_viewer.rs"]
mod tests;
