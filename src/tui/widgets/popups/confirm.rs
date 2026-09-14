// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// "are you sure?" popup for destructive actions. uses global state so the
// confirmation target persists across render frames.

use std::sync::LazyLock;
use std::sync::Mutex;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
};

use crate::config::theme::THEME;

static CONFIRM_STATE: LazyLock<Mutex<ConfirmState>> =
    LazyLock::new(|| Mutex::new(ConfirmState::default()));

#[derive(Debug, Default)]
struct ConfirmState {
    target: Option<ConfirmTarget>,
}

#[derive(Debug, Clone)]
pub enum ConfirmTarget {
    Instance {
        name: String,
    },
    Account {
        username: String,
        index: usize,
    },
    ConfigProfile {
        profile: String,
    },
    Content {
        name: String,
        path: std::path::PathBuf,
        dependents: Vec<String>,
    },
    OrphanDependencies {
        paths: Vec<std::path::PathBuf>,
    },
    InstanceRuntime {
        name: String,
    },
    LauncherCache,
    AutomaticSelection {
        setting: AutomaticSetting,
        instance: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutomaticSetting {
    Java,
    Account,
}

impl AutomaticSetting {
    fn title(self) -> &'static str {
        match self {
            Self::Java => " Enable automatic Java ",
            Self::Account => " Enable automatic account ",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Java => "Use the automatically selected Java runtime",
            Self::Account => "Use the currently active account",
        }
    }
}

impl ConfirmTarget {
    fn title(&self) -> String {
        match self {
            Self::OrphanDependencies { .. } => " Remove unused dependencies ".to_owned(),
            Self::InstanceRuntime { .. } => " Change runtime ".to_owned(),
            Self::LauncherCache => " Clear caches ".to_owned(),
            Self::AutomaticSelection { setting, .. } => setting.title().to_owned(),
            _ => format!(" Delete '{}' ", self.name()),
        }
    }

    fn body(&self) -> String {
        match self {
            ConfirmTarget::Instance { .. } => {
                "This will permanently remove the instance".to_owned()
            }
            ConfirmTarget::Account { .. } => "This will permanently remove this account".to_owned(),
            ConfirmTarget::ConfigProfile { .. } => {
                "This will permanently remove this config profile".to_owned()
            }
            ConfirmTarget::Content { dependents, .. } if !dependents.is_empty() => {
                format!(
                    "Still required by installed mods:\n{}\n! Deleting this dependency may break those mods.",
                    dependents
                        .iter()
                        .map(|name| format!("• {name}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            }
            ConfirmTarget::Content { .. } => {
                "This will permanently remove the selected item".to_owned()
            }
            ConfirmTarget::OrphanDependencies { paths } => paths
                .iter()
                .map(|path| {
                    format!(
                        "• {}",
                        path.file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("dependency")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
            ConfirmTarget::InstanceRuntime { .. } => {
                "Some installed mods may be incompatible".to_owned()
            }
            ConfirmTarget::LauncherCache => {
                "Provider metadata and Java detection will be rebuilt".to_owned()
            }
            ConfirmTarget::AutomaticSelection { setting, .. } => setting.description().to_owned(),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            ConfirmTarget::Instance { name } => name,
            ConfirmTarget::Account { username, .. } => username,
            ConfirmTarget::ConfigProfile { profile } => profile,
            ConfirmTarget::Content { name, .. } => name,
            ConfirmTarget::OrphanDependencies { .. } => "unused dependencies",
            ConfirmTarget::InstanceRuntime { name, .. } => name,
            ConfirmTarget::LauncherCache => "launcher caches",
            ConfirmTarget::AutomaticSelection { instance, .. } => {
                instance.as_deref().unwrap_or("launcher")
            }
        }
    }

    fn confirm_label(&self) -> &'static str {
        match self {
            Self::Content { dependents, .. } if !dependents.is_empty() => " delete anyway",
            Self::OrphanDependencies { .. } => " remove all",
            Self::InstanceRuntime { .. } => " change",
            Self::LauncherCache => " clear",
            Self::AutomaticSelection { .. } => " enable",
            _ => " confirm",
        }
    }
}

pub fn set_pending(target: ConfirmTarget) {
    match CONFIRM_STATE.lock() {
        Ok(mut s) => {
            s.target = Some(target);
        }
        Err(e) => {
            tracing::error!("Confirm popup state lock poisoned: {}", e);
        }
    }
}

pub fn set_pending_delete(name: impl Into<String>) {
    set_pending(ConfirmTarget::Instance { name: name.into() });
}

pub fn set_pending_instance_delete(name: impl Into<String>) {
    set_pending_delete(name);
}

pub fn set_pending_content_delete(name: impl Into<String>, path: impl Into<std::path::PathBuf>) {
    set_pending_managed_content_delete(name, path, Vec::new());
}

pub fn set_pending_managed_content_delete(
    name: impl Into<String>,
    path: impl Into<std::path::PathBuf>,
    dependents: Vec<String>,
) {
    set_pending(ConfirmTarget::Content {
        name: name.into(),
        path: path.into(),
        dependents,
    });
}

pub fn set_pending_orphan_dependencies(paths: Vec<std::path::PathBuf>) {
    set_pending(ConfirmTarget::OrphanDependencies { paths });
}

pub fn pending_target() -> Option<ConfirmTarget> {
    match CONFIRM_STATE.lock() {
        Ok(s) => s.target.clone(),
        Err(_) => None,
    }
}

pub fn clear_pending() {
    match CONFIRM_STATE.lock() {
        Ok(mut s) => {
            s.target = None;
        }
        Err(e) => {
            tracing::error!("Confirm popup state lock poisoned: {}", e);
        }
    }
}

pub struct ConfirmPopup {
    title: String,
    body: String,
    confirm_label: &'static str,
}

impl ConfirmPopup {
    pub fn for_target(target: &ConfirmTarget) -> Self {
        Self {
            title: target.title(),
            body: target.body(),
            confirm_label: target.confirm_label(),
        }
    }
}

impl Widget for ConfirmPopup {
    fn render(self, area: Rect, buf: &mut Buffer) {
        use super::{base::PopupFrame, keybind_line};

        let theme = THEME.as_ref();
        let title = Line::from(vec![Span::styled(
            self.title,
            Style::default()
                .fg(theme.accent())
                .add_modifier(Modifier::BOLD),
        )]);
        let kb = keybind_line(&[("Esc", " cancel"), ("Enter", self.confirm_label)]);

        let border_color = theme.text_dim();
        let bg_color = theme.surface();
        let accent = theme.accent();
        let text = theme.text();
        let text_dim = theme.text_dim();
        let error = theme.error();
        let body = self.body;
        let styled_list = body.contains("• ");
        let popup = PopupFrame {
            title,
            border_color,
            bg: Some(bg_color),
            keybinds: Some(kb),
            search_line: None,
            content: Box::new(move |inner, buf| {
                let lines = body
                    .lines()
                    .map(|line| {
                        if let Some(value) = line.strip_prefix("• ") {
                            Line::from(vec![
                                Span::styled("• ", Style::default().fg(accent)),
                                Span::styled(value.to_owned(), Style::default().fg(text)),
                            ])
                        } else if line.starts_with('!') {
                            Line::styled(line.to_owned(), Style::default().fg(error))
                        } else {
                            Line::styled(
                                line.to_owned(),
                                Style::default().fg(if styled_list { text_dim } else { text }),
                            )
                        }
                    })
                    .collect::<Vec<_>>();
                Paragraph::new(lines)
                    .wrap(Wrap { trim: true })
                    .render(inner, buf);
            }),
        };

        popup.render(area, buf);
    }
}

pub fn confirm_popup_area(frame_area: Rect, target: &ConfirmTarget) -> Rect {
    use super::word_wrap_size;
    use ratatui::layout::Constraint;
    const MAX_W: usize = 48;
    let body = target.body();
    let title_w = Span::raw(target.name()).width() + 12;
    let footer_w = Span::raw(format!("[Esc] cancel  [Enter]{}", target.confirm_label())).width();
    let (body_w, _) = word_wrap_size(&body, MAX_W);
    let inner_w = title_w.max(body_w).max(footer_w).min(MAX_W);
    let (_, lines) = word_wrap_size(&body, inner_w);
    let popup_w = ((inner_w + 2) as u16).min(frame_area.width.saturating_sub(4));
    let popup_h = ((lines + 2) as u16).min(frame_area.height.saturating_sub(4));
    frame_area.centered(Constraint::Length(popup_w), Constraint::Length(popup_h))
}
