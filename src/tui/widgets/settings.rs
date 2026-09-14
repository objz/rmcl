// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// settings panel: manages config profiles and shows compact instance info.
// detailed instance and launcher configuration opens in the TUI popups.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};
use tui_widget_list::{ListBuilder, ListState as TuiListState, ListView};

use crate::config::{
    SETTINGS,
    theme::{BORDER_STYLE, THEME},
};
use crate::instance::models::InstanceConfig;
use crate::tui::app::FocusedArea;

use super::styled_title;

const LOCAL_PROFILE_LABEL: &str = "instance default";

#[derive(Default)]
pub enum AddMode {
    #[default]
    None,
    ProfileName(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsPane {
    #[default]
    Profile,
    Info,
}

pub struct SettingsState {
    pub list_state: TuiListState,
    pub profiles: Vec<String>,
    pub add_mode: AddMode,
    pub pane: SettingsPane,
    meta_dir: PathBuf,
    active_profile: Option<String>,
    instance_name: Option<String>,
    java_key: Option<String>,
    java_label: String,
}

impl SettingsState {
    pub fn new(meta_dir: PathBuf) -> Self {
        let profiles = crate::instance::config_sync::list_profiles(&meta_dir).unwrap_or_else(|e| {
            tracing::warn!("Failed to load config sync profiles: {}", e);
            Vec::new()
        });
        let mut state = Self {
            list_state: TuiListState::default(),
            profiles,
            add_mode: AddMode::None,
            pane: SettingsPane::Profile,
            meta_dir,
            active_profile: None,
            instance_name: None,
            java_key: None,
            java_label: "unknown".to_string(),
        };
        state.select_active();
        state
    }

    fn reload_profiles(&mut self) {
        match crate::instance::config_sync::list_profiles(&self.meta_dir) {
            Ok(mut profiles) => {
                add_active_profile(&mut profiles, self.active_profile.as_deref());
                self.profiles = profiles;
            }
            Err(e) => tracing::warn!("Failed to reload config sync profiles: {}", e),
        }
    }

    fn select_active(&mut self) {
        self.list_state.selected = Some(
            self.active_profile
                .as_deref()
                .and_then(|active| self.profiles.iter().position(|profile| profile == active))
                .map(|idx| idx + 1)
                .unwrap_or(0),
        );
    }

    fn count(&self) -> usize {
        self.profiles.len() + 1
    }

    pub fn remove_profile(&mut self, profile: &str) {
        self.profiles.retain(|candidate| candidate != profile);
        if self.active_profile.as_deref() == Some(profile) {
            self.active_profile = None;
        }
        let last = self.count().saturating_sub(1);
        self.list_state.selected = Some(self.list_state.selected.unwrap_or(0).min(last));
    }

    fn update_for_instance(&mut self, instance: Option<&InstanceConfig>) {
        let instance_name = instance.map(|inst| inst.name.clone());
        let active_profile = instance.and_then(|inst| inst.config_sync_profile.clone());
        let active_profile = active_profile
            .filter(|profile| self.profiles.iter().any(|candidate| candidate == profile));
        if self.instance_name != instance_name || self.active_profile != active_profile {
            self.instance_name = instance_name;
            self.active_profile = active_profile;
            add_active_profile(&mut self.profiles, self.active_profile.as_deref());
            self.select_active();
        }

        let java_key = instance.map(java_path_key);
        if self.java_key != java_key {
            let java_source = instance.map(effective_java_path);
            self.java_label = java_source
                .as_deref()
                .map(java_version_label)
                .unwrap_or_else(|| "unknown".to_string());
            self.java_key = java_key;
        }
    }
}

fn add_active_profile(profiles: &mut Vec<String>, active_profile: Option<&str>) {
    if let Some(active) = active_profile
        && !profiles.iter().any(|profile| profile == active)
    {
        profiles.push(active.to_string());
        profiles.sort_unstable();
    }
}

fn java_path_key(instance: &InstanceConfig) -> String {
    instance
        .java_path
        .as_deref()
        .filter(|path| !path.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            SETTINGS
                .read()
                .paths
                .effective_java_path()
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "<auto>".to_owned())
}

fn effective_java_path(instance: &InstanceConfig) -> String {
    instance
        .java_path
        .clone()
        .or_else(|| {
            SETTINGS
                .read()
                .paths
                .effective_java_path()
                .map(str::to_owned)
        })
        .unwrap_or_else(crate::instance::java::detect_java_path)
}

fn java_version_label(java_path: &str) -> String {
    let Some(version) = crate::instance::java::java_version(Path::new(java_path)) else {
        return "unknown".to_string();
    };
    let major = crate::instance::java::java_major(Some(&version));
    if major == 0 {
        "unknown".to_string()
    } else {
        format!("jdk{major}")
    }
}

pub fn render(
    frame: &mut Frame,
    area: Rect,
    focused: FocusedArea,
    state: &mut SettingsState,
    instance: Option<&InstanceConfig>,
) {
    state.update_for_instance(instance);

    let theme = THEME.as_ref();
    let color = if focused == FocusedArea::Settings {
        theme.accent()
    } else {
        theme.border()
    };

    let mut block = Block::default()
        .title(styled_title("Settings", true))
        .borders(Borders::ALL)
        .border_type(BORDER_STYLE.to_border_type())
        .border_style(Style::default().fg(color));

    let keybind_line = if focused == FocusedArea::Settings {
        let keybinds: &[(&str, &str)] = match state.pane {
            SettingsPane::Profile => &[
                ("⏎", " select"),
                ("a", " add"),
                ("d", " del"),
                ("j/k", " move"),
                ("h/l", " tab"),
                ("Esc", " back"),
            ],
            SettingsPane::Info => &[
                ("e", " instance"),
                ("g", " launcher"),
                ("h/l", " tab"),
                ("Esc", " back"),
            ],
        };
        Some(super::popups::keybind_line_fitted(
            crate::config::settings::ShortcutHintScope::Settings,
            keybinds,
            area.width.saturating_sub(2),
        ))
    } else {
        None
    };
    if let Some(line) = keybind_line {
        block = block.title_bottom(line);
    }

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if instance.is_none() {
        frame.render_widget(
            Paragraph::new("No instance selected.")
                .style(Style::default().fg(THEME.as_ref().text_dim())),
            inner,
        );
    } else if inner.width >= 42 {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(50),
                Constraint::Length(3),
                Constraint::Percentage(50),
            ])
            .split(inner);
        render_profile_list(frame, chunks[0], focused, state);
        render_separator(frame, chunks[1], focused, state.pane);
        render_instance_info(frame, chunks[2], focused, state, instance);
    } else {
        render_profile_list(frame, inner, focused, state);
    }

    if let AddMode::ProfileName(name) = &state.add_mode {
        render_add_profile_popup(frame, name);
    }
}

fn render_separator(frame: &mut Frame, area: Rect, focused: FocusedArea, pane: SettingsPane) {
    let theme = THEME.as_ref();
    let color = if focused == FocusedArea::Settings && pane == SettingsPane::Info {
        theme.accent()
    } else {
        theme.border()
    };
    let line = if area.width >= 3 {
        " │ \n".repeat(area.height as usize)
    } else {
        "│\n".repeat(area.height as usize)
    };
    frame.render_widget(Paragraph::new(line).style(Style::default().fg(color)), area);
}

fn render_instance_info(
    frame: &mut Frame,
    area: Rect,
    focused: FocusedArea,
    state: &SettingsState,
    instance: Option<&InstanceConfig>,
) {
    let theme = THEME.as_ref();
    let label_style = Style::default().fg(theme.text_dim());
    let value_style = Style::default()
        .fg(theme.text())
        .add_modifier(Modifier::BOLD);

    let Some(inst) = instance else {
        frame.render_widget(
            Paragraph::new("No instance selected.").style(Style::default().fg(theme.text_dim())),
            area,
        );
        return;
    };

    let settings = SETTINGS.read();
    let memory_min = inst
        .memory_min
        .as_deref()
        .unwrap_or(&settings.defaults.memory_min);
    let memory_max = inst
        .memory_max
        .as_deref()
        .unwrap_or(&settings.defaults.memory_max);
    let active_style = if focused == FocusedArea::Settings && state.pane == SettingsPane::Info {
        value_style.fg(theme.accent())
    } else {
        value_style
    };
    let shortcut = if crate::instance::desktop::exists(&inst.name) {
        "enabled"
    } else {
        "disabled"
    };
    let lines = vec![
        Line::from(vec![
            Span::styled("Memory   ", label_style),
            Span::styled(format!("{memory_min} - {memory_max}"), active_style),
        ]),
        Line::from(vec![
            Span::styled("Java     ", label_style),
            Span::styled(state.java_label.as_str(), active_style),
        ]),
        Line::from(vec![
            Span::styled("Shortcut ", label_style),
            Span::styled(shortcut, active_style),
        ]),
    ];

    frame.render_widget(Paragraph::new(lines), area);
}

fn render_profile_list(
    frame: &mut Frame,
    area: Rect,
    focused: FocusedArea,
    state: &mut SettingsState,
) {
    let is_focused = focused == FocusedArea::Settings;
    let pane = state.pane;
    let active_profile = state.active_profile.clone();
    let profiles = state.profiles.clone();
    let count = profiles.len() + 1;
    let last = count.saturating_sub(1);
    state.list_state.selected = Some(state.list_state.selected.unwrap_or(0).min(last));

    let builder = ListBuilder::new(move |context| {
        let theme = THEME.as_ref();
        let name = if context.index == 0 {
            LOCAL_PROFILE_LABEL.to_string()
        } else {
            profiles.get(context.index - 1).cloned().unwrap_or_default()
        };
        let is_active = if context.index == 0 {
            active_profile.is_none()
        } else {
            active_profile.as_deref() == Some(name.as_str())
        };
        let show_selected = is_focused && pane == SettingsPane::Profile && context.is_selected;
        let marker = if is_active { "\u{25b8} " } else { "  " };
        let background = if show_selected {
            theme.stripe()
        } else {
            theme.background()
        };
        let style = if show_selected {
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD)
        } else if is_active {
            Style::default()
                .fg(theme.text())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text())
        };
        let line = Line::from(vec![
            Span::styled(marker, Style::default().fg(theme.success())),
            Span::styled(name, style),
        ]);
        (
            ratatui::text::Text::from(line).style(Style::default().bg(background)),
            1,
        )
    });

    frame.render_stateful_widget(ListView::new(builder, count), area, &mut state.list_state);
}

fn render_add_profile_popup(frame: &mut Frame, name: &str) {
    use super::popups::{base::PopupFrame, keybind_line};
    let theme = THEME.as_ref();
    let area = popup_area(frame, 42, 5);
    let name = name.to_string();

    let border_color = theme.text_dim();
    let bg_color = theme.surface();
    let dim_color = theme.text_dim();
    let text_color = theme.text();

    PopupFrame {
        title: Line::from(Span::styled(
            " Config Profile ",
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        ))
        .centered(),
        border_color,
        bg: Some(bg_color),
        keybinds: Some(keybind_line(&[("Enter", " create"), ("Esc", " cancel")])),
        search_line: None,
        content: Box::new(move |inner, buf| {
            let line = if name.is_empty() {
                Line::from(vec![
                    Span::styled("Profile name...", Style::default().fg(dim_color)),
                    Span::styled(
                        "\u{2588}",
                        Style::default()
                            .fg(border_color)
                            .add_modifier(Modifier::SLOW_BLINK),
                    ),
                ])
            } else {
                Line::from(vec![
                    Span::styled(name.as_str(), Style::default().fg(text_color)),
                    Span::styled(
                        "\u{2588}",
                        Style::default()
                            .fg(border_color)
                            .add_modifier(Modifier::SLOW_BLINK),
                    ),
                ])
            };
            Paragraph::new(line).render(inner, buf);
        }),
    }
    .render(area, frame.buffer_mut());
}

fn popup_area(frame: &Frame, width: u16, height: u16) -> Rect {
    let area = frame.area();
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

pub enum SettingsAction {
    None,
    OpenInstance,
    OpenGlobal,
    SelectProfile(Option<String>),
    ConfirmDeleteProfile(String),
    Error(String),
}

pub fn handle_key(
    key_event: &KeyEvent,
    state: &mut SettingsState,
    instance: Option<&InstanceConfig>,
) -> SettingsAction {
    if let AddMode::ProfileName(name) = &state.add_mode {
        match key_event.code {
            KeyCode::Enter => {
                let name = name.trim().to_string();
                state.add_mode = AddMode::None;
                if name.is_empty() {
                    return SettingsAction::None;
                }
                return match crate::instance::config_sync::create_profile(&state.meta_dir, &name) {
                    Ok(profile) => {
                        state.reload_profiles();
                        state.active_profile = Some(profile);
                        state.select_active();
                        SettingsAction::SelectProfile(state.active_profile.clone())
                    }
                    Err(e) => SettingsAction::Error(e.to_string()),
                };
            }
            KeyCode::Esc => {
                state.add_mode = AddMode::None;
                return SettingsAction::None;
            }
            KeyCode::Backspace => {
                let mut new_name = name.clone();
                super::search::backspace(&mut new_name, key_event.modifiers);
                state.add_mode = AddMode::ProfileName(new_name);
                return SettingsAction::None;
            }
            KeyCode::Char(c) => {
                let mut new_name = name.clone();
                new_name.push(c);
                state.add_mode = AddMode::ProfileName(new_name);
                return SettingsAction::None;
            }
            _ => return SettingsAction::None,
        }
    }

    match key_event.code {
        KeyCode::Char('h') | KeyCode::Left => {
            state.pane = SettingsPane::Profile;
            SettingsAction::None
        }
        KeyCode::Char('l') | KeyCode::Right => {
            state.pane = SettingsPane::Info;
            SettingsAction::None
        }
        KeyCode::Enter if state.pane == SettingsPane::Profile => {
            let selected = state.list_state.selected.unwrap_or(0);
            let profile = if selected == 0 {
                None
            } else {
                state.profiles.get(selected - 1).cloned()
            };
            SettingsAction::SelectProfile(profile)
        }
        KeyCode::Char('a') if state.pane == SettingsPane::Profile => {
            state.add_mode = AddMode::ProfileName(String::new());
            SettingsAction::None
        }
        KeyCode::Char('d') if state.pane == SettingsPane::Profile => {
            let selected = state.list_state.selected.unwrap_or(0);
            if selected == 0 {
                SettingsAction::None
            } else if let Some(profile) = state.profiles.get(selected - 1) {
                SettingsAction::ConfirmDeleteProfile(profile.clone())
            } else {
                SettingsAction::None
            }
        }
        KeyCode::Char('j') | KeyCode::Down if state.pane == SettingsPane::Profile => {
            let count = state.count();
            if count > 0 {
                let cur = state.list_state.selected.unwrap_or(0);
                state.list_state.selected = Some((cur + 1).min(count - 1));
            }
            SettingsAction::None
        }
        KeyCode::Char('k') | KeyCode::Up if state.pane == SettingsPane::Profile => {
            let cur = state.list_state.selected.unwrap_or(0);
            state.list_state.selected = Some(cur.saturating_sub(1));
            SettingsAction::None
        }
        KeyCode::Char('e') if state.pane == SettingsPane::Info => {
            if instance.is_some() {
                SettingsAction::OpenInstance
            } else {
                SettingsAction::None
            }
        }
        KeyCode::Char('g') if state.pane == SettingsPane::Info => SettingsAction::OpenGlobal,
        _ => SettingsAction::None,
    }
}

#[cfg(test)]
#[path = "../tests/widgets/settings.rs"]
mod tests;
