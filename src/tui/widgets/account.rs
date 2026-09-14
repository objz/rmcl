// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// account management panel: list, add (microsoft/offline), delete
// microsoft auth uses the device code flow, so it polls a shared mutex
// for the result while showing the user a code to enter in their browser

use std::sync::{Arc, Mutex};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};
use tui_widget_list::{ListBuilder, ListState as TuiListState, ListView};

use crate::auth::{self, AccountStore, AccountType, AuthResult, DeviceCodeInfo};
use crate::config::theme::{BORDER_STYLE, THEME};
use crate::tui::app::FocusedArea;

use super::styled_title;

#[derive(Default)]
pub enum AddMode {
    #[default]
    None,
    ChooseType,
    OfflineNameInput(String),
    OfflineBlocked,
    DeviceCodeWaiting {
        info: DeviceCodeInfo,
        pending: Arc<Mutex<Option<AuthResult>>>,
    },
}

pub struct AccountState {
    pub store: AccountStore,
    pub list_state: TuiListState,
    pub add_mode: AddMode,
}

impl Default for AccountState {
    fn default() -> Self {
        let store = AccountStore::load();
        let mut list_state = TuiListState::default();
        if !store.accounts.is_empty() {
            list_state.selected = Some(0);
        }
        Self {
            store,
            list_state,
            add_mode: AddMode::None,
        }
    }
}

impl AccountState {
    // called every tick to check if the background auth thread finished.
    // can't block on it because the TUI needs to keep rendering
    pub fn drain_auth_result(&mut self) {
        if let AddMode::DeviceCodeWaiting { pending, .. } = &self.add_mode {
            let result = match pending.lock() {
                Ok(mut slot) => slot.take(),
                _ => None,
            };

            if let Some(result) = result {
                match result {
                    AuthResult::Success(account) => {
                        self.store.add(account);
                        self.add_mode = AddMode::None;
                        if self.list_state.selected.is_none() && !self.store.accounts.is_empty() {
                            self.list_state.selected = Some(0);
                        }
                    }
                    AuthResult::Error(e) => {
                        tracing::error!("Microsoft auth failed: {}", e);
                        self.add_mode = AddMode::None;
                    }
                }
            }
        }
    }
}

pub fn handle_key(key_event: &KeyEvent, state: &mut AccountState) -> bool {
    match &state.add_mode {
        AddMode::ChooseType => match key_event.code {
            KeyCode::Char('m') | KeyCode::Char('1') => {
                let pending = auth::start_microsoft_auth();
                state.add_mode = AddMode::DeviceCodeWaiting {
                    info: DeviceCodeInfo {
                        user_code: String::new(),
                        verification_uri: String::new(),
                    },
                    pending,
                };
                true
            }
            KeyCode::Char('o') | KeyCode::Char('2') => {
                state.add_mode = if state.store.has_microsoft_account() {
                    AddMode::OfflineNameInput(String::new())
                } else {
                    AddMode::OfflineBlocked
                };
                true
            }
            KeyCode::Esc => {
                state.add_mode = AddMode::None;
                true
            }
            _ => true,
        },
        AddMode::OfflineNameInput(name) => match key_event.code {
            KeyCode::Enter => {
                let trimmed = name.trim().to_string();
                if !trimmed.is_empty() {
                    if state.store.has_microsoft_account() {
                        let account = auth::create_offline_account(&trimmed);
                        state.store.add(account);
                        if state.list_state.selected.is_none() && !state.store.accounts.is_empty() {
                            state.list_state.selected = Some(0);
                        }
                        state.add_mode = AddMode::None;
                    } else {
                        state.add_mode = AddMode::OfflineBlocked;
                    }
                } else {
                    state.add_mode = AddMode::None;
                }
                true
            }
            KeyCode::Char(c) => {
                let mut new_name = name.clone();
                new_name.push(c);
                state.add_mode = AddMode::OfflineNameInput(new_name);
                true
            }
            KeyCode::Backspace => {
                let mut new_name = name.clone();
                super::search::backspace(&mut new_name, key_event.modifiers);
                state.add_mode = AddMode::OfflineNameInput(new_name);
                true
            }
            KeyCode::Esc => {
                state.add_mode = AddMode::None;
                true
            }
            _ => true,
        },
        AddMode::OfflineBlocked => match key_event.code {
            KeyCode::Enter | KeyCode::Esc => {
                state.add_mode = AddMode::None;
                true
            }
            _ => true,
        },
        AddMode::DeviceCodeWaiting { .. } => match key_event.code {
            KeyCode::Esc => {
                state.add_mode = AddMode::None;
                true
            }
            _ => true,
        },
        AddMode::None => {
            let count = state.store.accounts.len();
            match key_event.code {
                KeyCode::Char('a') => {
                    state.add_mode = AddMode::ChooseType;
                    true
                }
                KeyCode::Enter => {
                    if let Some(idx) = state.list_state.selected {
                        state.store.set_active(idx);
                    }
                    true
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    if count > 0 {
                        let cur = state.list_state.selected.unwrap_or(0);
                        state.list_state.selected = Some((cur + 1).min(count - 1));
                    }
                    true
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    let cur = state.list_state.selected.unwrap_or(0);
                    state.list_state.selected = Some(cur.saturating_sub(1));
                    true
                }
                _ => false,
            }
        }
    }
}

// the device code arrives asynchronously from the auth thread,
// so it gets pulled out of a global mutex once it's ready
pub fn drain_device_code(state: &mut AccountState) {
    if let AddMode::DeviceCodeWaiting { info, .. } = &mut state.add_mode
        && info.user_code.is_empty()
        && let Ok(mut slot) = auth::DEVICE_CODE_DISPLAY.lock()
        && let Some(dc_info) = slot.take()
    {
        info.user_code = dc_info.user_code;
        info.verification_uri = dc_info.verification_uri;
    }
}

pub fn render(frame: &mut Frame, area: Rect, focused: FocusedArea, state: &mut AccountState) {
    let theme = THEME.as_ref();
    let color = if focused == FocusedArea::Account {
        theme.accent()
    } else {
        theme.border()
    };

    let mut block = Block::default()
        .title(styled_title("Accounts", true))
        .borders(Borders::ALL)
        .border_type(BORDER_STYLE.to_border_type())
        .border_style(Style::default().fg(color));

    let keybind_line = if focused == FocusedArea::Account {
        Some(super::popups::keybind_line_fitted(
            crate::config::settings::ShortcutHintScope::Accounts,
            &[
                ("⏎", " select"),
                ("a", " add"),
                ("d", " del"),
                ("Esc", " back"),
            ],
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

    if state.store.accounts.is_empty() {
        frame.render_widget(
            Paragraph::new("No accounts.").style(Style::default().fg(theme.text_dim())),
            inner,
        );
    } else {
        let active = state.store.active_account().map(|a| a.uuid.clone());
        render_account_list(frame, inner, state, focused, active.as_deref());
    }

    match &state.add_mode {
        AddMode::ChooseType => render_choose_popup(frame),
        AddMode::OfflineNameInput(name) => render_offline_popup(frame, name),
        AddMode::OfflineBlocked => render_offline_blocked_popup(frame),
        AddMode::DeviceCodeWaiting { info, .. } => render_device_code_popup(frame, info),
        AddMode::None => {}
    }
}

fn render_account_list(
    frame: &mut Frame,
    area: Rect,
    state: &mut AccountState,
    focused: FocusedArea,
    active_uuid: Option<&str>,
) {
    let is_focused = focused == FocusedArea::Account;
    let accounts: Vec<(String, AccountType, bool)> = state
        .store
        .accounts
        .iter()
        .map(|a| {
            (
                a.username.clone(),
                a.account_type.clone(),
                active_uuid == Some(&a.uuid),
            )
        })
        .collect();

    let count = accounts.len();

    let builder = ListBuilder::new(move |context| {
        let theme = THEME.as_ref();
        let (username, acc_type, is_active) = &accounts[context.index];
        let show_selected = is_focused && context.is_selected;

        let bg = if show_selected {
            theme.stripe()
        } else {
            theme.background()
        };

        let active_marker = if *is_active { "\u{25b8} " } else { "  " };

        let style = if show_selected {
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD)
        } else if *is_active {
            Style::default()
                .fg(theme.text())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text())
        };

        let mut spans = vec![
            Span::styled(active_marker, Style::default().fg(theme.success())),
            Span::styled(username.clone(), style),
        ];

        if *acc_type == AccountType::Offline {
            let offline_style = if show_selected {
                Style::default()
                    .fg(theme.accent())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text_dim())
            };
            spans.push(Span::styled(" (Offline)", offline_style));
        }

        let item = ratatui::text::Text::from(Line::from(spans)).style(Style::default().bg(bg));
        (item, 1)
    });

    let list = ListView::new(builder, count);
    frame.render_stateful_widget(list, area, &mut state.list_state);
}

// center a popup of given size within the terminal. nothing fancy
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

fn render_choose_popup(frame: &mut Frame) {
    use super::popups::base::PopupFrame;
    let theme = THEME.as_ref();
    let area = popup_area(frame, 40, 7);

    let border_color = theme.text_dim();
    let dim_color = theme.text_dim();
    let accent_color = theme.success();
    let text_color = theme.text();

    PopupFrame {
        title: Line::from(" Add Account ").centered(),
        border_color,
        bg: Some(theme.surface()),
        keybinds: Some(Line::from(Span::styled(
            " Esc: cancel ",
            Style::default().fg(dim_color),
        ))),
        search_line: None,
        content: Box::new(move |inner, buf| {
            let text = vec![
                Line::from(""),
                Line::from(vec![
                    Span::styled(
                        " [m] ",
                        Style::default()
                            .fg(accent_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("Microsoft Account", Style::default().fg(text_color)),
                ]),
                Line::from(vec![
                    Span::styled(
                        " [o] ",
                        Style::default()
                            .fg(accent_color)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled("Offline Account", Style::default().fg(text_color)),
                ]),
            ];
            Paragraph::new(text).render(inner, buf);
        }),
    }
    .render(area, frame.buffer_mut());
}

fn render_offline_popup(frame: &mut Frame, name: &str) {
    use super::popups::{base::PopupFrame, keybind_line};
    let theme = THEME.as_ref();
    let area = popup_area(frame, 40, 5);
    let name = name.to_string();

    let border_color = theme.text_dim();
    let bg_color = theme.surface();
    let dim_color = theme.text_dim();
    let text_color = theme.text();

    PopupFrame {
        title: Line::from(Span::styled(
            " Offline Account ",
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        ))
        .centered(),
        border_color,
        bg: Some(bg_color),
        keybinds: Some(keybind_line(&[("Enter", " confirm"), ("Esc", " cancel")])),
        search_line: None,
        content: Box::new(move |inner, buf| {
            let line = if name.is_empty() {
                Line::from(vec![
                    Span::styled("Username...", Style::default().fg(dim_color)),
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

fn render_offline_blocked_popup(frame: &mut Frame) {
    use super::popups::{base::PopupFrame, keybind_line};
    let theme = THEME.as_ref();
    let area = popup_area(frame, 58, 5);

    let border_color = theme.text_dim();
    let bg_color = theme.surface();
    let text_color = theme.text();

    PopupFrame {
        title: Line::from(Span::styled(
            " Offline Account ",
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        ))
        .centered(),
        border_color,
        bg: Some(bg_color),
        keybinds: Some(keybind_line(&[("Enter", " close")])),
        search_line: None,
        content: Box::new(move |inner, buf| {
            Paragraph::new("Add a Microsoft account that owns Minecraft first.")
                .style(Style::default().fg(text_color))
                .render(inner, buf);
        }),
    }
    .render(area, frame.buffer_mut());
}

fn render_device_code_popup(frame: &mut Frame, info: &DeviceCodeInfo) {
    use super::popups::{base::PopupFrame, keybind_line};
    let theme = THEME.as_ref();
    let area = popup_area(frame, 50, 8);
    let uri = info.verification_uri.clone();
    let code = info.user_code.clone();

    let border_color = theme.text_dim();
    let bg_color = theme.surface();
    let dim_color = theme.text_dim();
    let accent_color = theme.success();

    PopupFrame {
        title: Line::from(Span::styled(
            " Microsoft Login ",
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        ))
        .centered(),
        border_color,
        bg: Some(bg_color),
        keybinds: Some(keybind_line(&[("Esc", " cancel")])),
        search_line: None,
        content: Box::new(move |inner, buf| {
            let text = if code.is_empty() {
                vec![Line::from(Span::styled(
                    "Connecting to Microsoft...",
                    Style::default().fg(dim_color),
                ))]
            } else {
                vec![
                    Line::from(Span::styled(
                        "Open this URL in your browser:",
                        Style::default().fg(dim_color),
                    )),
                    Line::from(Span::styled(
                        uri.as_str(),
                        Style::default()
                            .fg(accent_color)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("Enter code: ", Style::default().fg(dim_color)),
                        Span::styled(
                            code.as_str(),
                            Style::default()
                                .fg(accent_color)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(""),
                    Line::from(Span::styled(
                        "Waiting for authentication...",
                        Style::default().fg(dim_color),
                    )),
                ]
            };
            Paragraph::new(text).render(inner, buf);
        }),
    }
    .render(area, frame.buffer_mut());
}

#[cfg(test)]
#[path = "../tests/widgets/account.rs"]
mod tests;
