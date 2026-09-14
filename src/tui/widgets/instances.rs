// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// the instance list on the left side of the UI.
// handles search/filter, scrollbar sync, and inline renaming.
// each row shows instance name + "last played" or current run state.

use crate::config::theme::{BORDER_STYLE, THEME};
use crossterm::event::KeyCode;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use tui_widget_list::{ListBuilder, ListState as TuiListState, ListView};

use crate::instance::models::InstanceConfig;
use crate::instance::runtime::{RunState, get as get_run_state};
use crate::time::format_relative_time;
use crate::tui::app::FocusedArea;

use super::{WidgetKey, search::SearchState, styled_title};

type PendingModpackUpdate = (
    String,
    crate::instance::ProviderProject,
    Option<crate::net::modrinth::VersionInfo>,
);
static PENDING_MODPACK_UPDATES: LazyLock<Mutex<Vec<PendingModpackUpdate>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));
static MODPACK_UPDATE_SLOTS: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(4)));

pub fn spawn_modpack_update_checks(instances: &[InstanceConfig]) {
    if !crate::config::SETTINGS.read().general.check_modpack_updates {
        return;
    }
    for instance in instances {
        spawn_modpack_update_check(instance);
    }
}

pub fn spawn_modpack_update_check(instance: &InstanceConfig) {
    if !crate::config::SETTINGS.read().general.check_modpack_updates {
        return;
    }
    let Some(source) = instance.modpack_source.clone() else {
        return;
    };
    let instance = instance.clone();
    tokio::spawn(async move {
        let Ok(_permit) = MODPACK_UPDATE_SLOTS.clone().acquire_owned().await else {
            return;
        };
        let Ok(versions) = crate::instance::import::provider_versions(&source).await else {
            return;
        };
        let update = crate::instance::content::provider::newest_version(&versions)
            .cloned()
            .filter(|_| {
                crate::instance::content::provider::has_newer_compatible_version(
                    &versions,
                    &source.version_id,
                )
            });
        if let Ok(mut pending) = PENDING_MODPACK_UPDATES.lock() {
            pending.push((instance.name, source, update));
            crate::feedback::request_redraw();
        }
    });
}

#[derive(Debug, Default)]
pub struct State {
    pub instances: Vec<InstanceConfig>,
    pub list_state: TuiListState,
    pub scrollbar_state: ScrollbarState,
    pub show_popup: bool,
    pub show_import_popup: bool,
    pub search: SearchState,
    pub renaming: Option<String>,
    pub(crate) modpack_updates: HashMap<String, crate::net::modrinth::VersionInfo>,
}

impl State {
    pub fn with_instances(instances: Vec<InstanceConfig>) -> Self {
        let count = instances.len();
        let mut s = State {
            instances,
            list_state: TuiListState::default(),
            scrollbar_state: ScrollbarState::default(),
            show_popup: false,
            show_import_popup: false,
            search: SearchState::default(),
            renaming: None,
            modpack_updates: HashMap::new(),
        };
        if count > 0 {
            s.list_state.selected = Some(0);
        }
        s.update_scrollbar();
        s
    }

    pub fn selected_instance(&self) -> Option<&InstanceConfig> {
        let filtered = self.filtered_indices();
        self.list_state
            .selected
            .and_then(|i| filtered.get(i))
            .and_then(|&idx| self.instances.get(idx))
    }

    fn filtered_indices(&self) -> Vec<usize> {
        self.instances
            .iter()
            .enumerate()
            .filter(|(_, inst)| self.search.matches(&inst.name))
            .map(|(i, _)| i)
            .collect()
    }

    fn next(&mut self) {
        let count = self.filtered_indices().len();
        if count == 0 {
            return;
        }
        self.list_state.next();
        if self.list_state.selected.unwrap_or(0) >= count {
            self.list_state.selected = Some(0);
        }
        self.update_scrollbar();
    }

    fn previous(&mut self) {
        let count = self.filtered_indices().len();
        if count == 0 {
            return;
        }
        self.list_state.previous();
        if self.list_state.selected.is_none() {
            self.list_state.selected = Some(count.saturating_sub(1));
        }
        self.update_scrollbar();
    }

    fn update_scrollbar(&mut self) {
        let filtered = self.filtered_indices();
        let count = filtered.len();
        let items = count.saturating_sub(1);
        let index = self.list_state.selected.unwrap_or(0);

        if count == 0 {
            self.list_state.selected = None;
        } else if self.list_state.selected.is_none() {
            self.list_state.selected = Some(0);
        } else if index > items {
            self.list_state.selected = Some(items);
        }

        self.scrollbar_state =
            ScrollbarState::new(items).position(self.list_state.selected.unwrap_or(0));
    }

    pub fn wants_popup(&self) -> bool {
        self.show_popup
    }

    pub fn wants_import_popup(&self) -> bool {
        self.show_import_popup
    }

    pub fn remove_instance(&mut self, name: &str) {
        let before = self.instances.len();
        self.instances.retain(|i| i.name != name);
        let after = self.instances.len();
        if after < before {
            self.modpack_updates.remove(name);
            self.update_scrollbar();
        }
    }

    pub fn drain_modpack_updates(&mut self) {
        let Ok(mut pending) = PENDING_MODPACK_UPDATES.lock() else {
            return;
        };
        for (name, source, update) in pending.drain(..) {
            if self
                .instances
                .iter()
                .find(|instance| instance.name == name)
                .and_then(|instance| instance.modpack_source.as_ref())
                != Some(&source)
            {
                continue;
            }
            if let Some(update) = update {
                self.modpack_updates.insert(name, update);
            } else {
                self.modpack_updates.remove(&name);
            }
        }
    }

    pub fn add_instance(&mut self, instance: InstanceConfig) {
        self.instances.push(instance);
        self.list_state.selected = self.filtered_indices().len().checked_sub(1);
        self.update_scrollbar();
    }

    pub fn selected_modpack_update(&self) -> Option<crate::net::modrinth::VersionInfo> {
        self.selected_instance()
            .and_then(|instance| self.modpack_updates.get(&instance.name))
            .cloned()
    }

    pub fn replace_instance(&mut self, old_name: &str, instance: InstanceConfig) {
        if let Some(existing) = self
            .instances
            .iter_mut()
            .find(|i| i.name == old_name || i.name == instance.name)
        {
            *existing = instance;
        } else {
            self.instances.push(instance);
        }
        self.update_scrollbar();
    }
}

impl WidgetKey for State {
    fn handle_key(&mut self, key_event: &crossterm::event::KeyEvent) {
        if self.search.active {
            match key_event.code {
                KeyCode::Enter => {
                    self.search.confirm();
                    self.list_state.selected = Some(0);
                    self.update_scrollbar();
                }
                KeyCode::Esc => {
                    self.search.deactivate();
                    self.list_state.selected = Some(0);
                    self.update_scrollbar();
                }
                KeyCode::Backspace => {
                    self.search.backspace(key_event.modifiers);
                    self.list_state.selected = Some(0);
                    self.update_scrollbar();
                }
                KeyCode::Char(c) => {
                    self.search.push(c);
                    self.list_state.selected = Some(0);
                    self.update_scrollbar();
                }
                _ => {}
            }
            return;
        }

        match key_event.code {
            KeyCode::Char('/') => {
                self.search.activate();
                self.list_state.selected = Some(0);
                self.update_scrollbar();
            }
            KeyCode::Char('a') => {
                self.show_popup = true;
                self.update_scrollbar();
            }
            KeyCode::Char('m') => {
                self.show_import_popup = true;
            }
            KeyCode::Char('d') => {}
            KeyCode::Char('j') | KeyCode::Down => self.next(),
            KeyCode::Char('k') | KeyCode::Up => self.previous(),
            _ => {}
        }
    }
}

pub fn render(frame: &mut Frame, area: Rect, focused: FocusedArea, state: &mut State) {
    let theme = THEME.as_ref();
    let color = if focused == FocusedArea::Instances {
        theme.accent()
    } else {
        theme.border()
    };

    let mut block = Block::default()
        .title(styled_title("Instances", true))
        .borders(Borders::ALL)
        .border_type(BORDER_STYLE.to_border_type())
        .border_style(Style::default().fg(color));

    if let Some(search_line) = state.search.title_line() {
        block = block.title_top(search_line);
    }

    let scrollbar_area = Rect {
        x: area.x + area.width.saturating_sub(1),
        y: area.y + 1,
        width: 1,
        height: area.height.saturating_sub(2),
    };

    let filtered = state.filtered_indices();
    let count = filtered.len();

    let builder = ListBuilder::new(|context| {
        let theme = THEME.as_ref();
        let idx = filtered[context.index];
        let instance = &state.instances[idx];

        let stripe_bg = if context.index % 2 == 0 {
            theme.background()
        } else {
            theme.stripe()
        };

        let (name_style, meta_style, bg) = if context.is_selected {
            (
                Style::default()
                    .fg(theme.accent())
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(theme.text_dim()),
                stripe_bg,
            )
        } else {
            (
                Style::default()
                    .fg(theme.text())
                    .add_modifier(Modifier::BOLD),
                Style::default().fg(theme.text_dim()),
                stripe_bg,
            )
        };

        let selector = if context.is_selected {
            Span::styled("\u{258c} ", Style::default().fg(theme.accent()))
        } else {
            Span::raw("  ")
        };

        let is_renaming = context.is_selected && state.renaming.is_some();
        let name_line = if is_renaming {
            let rename_val = state.renaming.as_deref().unwrap_or("");
            Line::from(vec![
                selector.clone(),
                Span::styled(rename_val, Style::default().fg(theme.text())),
                Span::styled(
                    "\u{2588}",
                    Style::default()
                        .fg(theme.text_dim())
                        .add_modifier(Modifier::SLOW_BLINK),
                ),
            ])
        } else {
            let mut spans = vec![selector.clone()];
            spans.extend(state.search.highlight_spans(&instance.name, name_style));
            Line::from(spans)
        };

        let (meta_text, meta_text_style) = match get_run_state(&instance.name) {
            Some(RunState::Authenticating) => (
                "Authenticating".to_string(),
                Style::default().fg(theme.success()),
            ),
            Some(RunState::Running) | Some(RunState::Starting) => {
                ("Playing".to_string(), Style::default().fg(theme.success()))
            }
            _ => (format_relative_time(instance.last_played), meta_style),
        };

        let meta_line = Line::from(vec![
            selector.clone(),
            Span::styled(meta_text, meta_text_style),
        ]);

        let item = Text::from(vec![name_line, meta_line]).style(Style::default().bg(bg));
        (item, 2)
    });

    let list = ListView::new(builder, count).block(block);

    frame.render_stateful_widget(list, area, &mut state.list_state);

    frame.render_stateful_widget(
        Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("\u{25b2}"))
            .style(
                Style::default()
                    .fg(theme.text_dim())
                    .add_modifier(Modifier::BOLD),
            )
            .thumb_symbol("\u{2551}")
            .track_symbol(Some(""))
            .end_symbol(Some("\u{25bc}")),
        scrollbar_area,
        &mut state.scrollbar_state,
    );
}

#[cfg(test)]
#[path = "../tests/widgets/instances.rs"]
mod tests;
