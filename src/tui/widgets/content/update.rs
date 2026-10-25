// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use ratatui::{
    Frame,
    layout::{Constraint, Margin},
    style::Style,
    widgets::{Paragraph, Widget},
};

use crate::instance::ContentKind;
use crate::instance::content::entry::ContentEntry;
use crate::instance::content::updates::{BulkUpdatePlan, UpdateSnapshot};
use crate::tui::widgets::popups::base::PopupFrame;

pub enum PendingResult {
    Prepared(UpdateSnapshot, BulkUpdatePlan),
    Applied(Result<(), String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Checking,
    Conflicts,
    Review,
    Applying,
}

pub struct State {
    pub instance_name: String,
    pub phase: Phase,
    pub plan: Option<BulkUpdatePlan>,
    pub snapshot: Option<UpdateSnapshot>,
    pub pending: Arc<Mutex<Vec<PendingResult>>>,
    pub kind: ContentKind,
    pub target_world: Option<(String, PathBuf)>,
    pub completed: bool,
    pub applied: bool,
    pub abort_handle: Option<tokio::task::AbortHandle>,
    pub list: super::list::ContentListState,
    source_entries: HashMap<PathBuf, ContentEntry>,
}

impl State {
    pub fn checking(
        instance_name: String,
        kind: ContentKind,
        target_world: Option<(String, PathBuf)>,
        entries: Vec<ContentEntry>,
    ) -> Self {
        Self {
            instance_name,
            phase: Phase::Checking,
            plan: None,
            snapshot: None,
            pending: Arc::new(Mutex::new(Vec::new())),
            kind,
            target_world,
            completed: false,
            applied: false,
            abort_handle: None,
            list: super::list::ContentListState::default(),
            source_entries: entries
                .into_iter()
                .map(|entry| (entry.path.clone(), entry))
                .collect(),
        }
    }

    pub fn cancel(&self) {
        if let Some(handle) = &self.abort_handle {
            handle.abort();
        }
    }

    pub fn drain(&mut self) -> bool {
        let pending = match self.pending.lock() {
            Ok(mut pending) => pending.drain(..).collect::<Vec<_>>(),
            Err(_) => return false,
        };
        let mut changed = false;
        for result in pending {
            changed = true;
            match result {
                PendingResult::Prepared(snapshot, plan) => {
                    self.phase = if !plan.conflicts.is_empty() {
                        Phase::Conflicts
                    } else if !plan.roots.is_empty() {
                        Phase::Review
                    } else {
                        self.completed = true;
                        Phase::Review
                    };
                    self.snapshot = Some(snapshot);
                    self.plan = Some(plan);
                    self.rebuild_list();
                }
                PendingResult::Applied(Ok(())) => {
                    self.completed = true;
                    self.applied = true;
                }
                PendingResult::Applied(Err(error)) => {
                    crate::feedback::errors::push_error(crate::feedback::errors::ErrorEvent {
                        id: 0,
                        level: tracing::Level::ERROR,
                        message: format!("Failed to update content: {error}"),
                        pushed_at: Instant::now(),
                    });
                    self.completed = true;
                }
            }
        }
        changed
    }

    pub fn push(&self, result: PendingResult) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.push(result);
            crate::feedback::request_redraw();
        }
    }

    pub fn visible(&self) -> bool {
        matches!(
            self.phase,
            Phase::Checking | Phase::Conflicts | Phase::Review
        ) && !self.completed
    }

    pub fn has_updates(&self) -> bool {
        self.plan
            .as_ref()
            .is_some_and(|plan| !plan.roots.is_empty())
    }

    pub fn show_review(&mut self) {
        self.phase = Phase::Review;
        self.rebuild_list();
    }

    pub fn show_conflicts(&mut self) {
        if self
            .plan
            .as_ref()
            .is_some_and(|plan| !plan.conflicts.is_empty())
        {
            self.phase = Phase::Conflicts;
            self.rebuild_list();
        }
    }

    fn rebuild_list(&mut self) {
        let entries = match self.phase {
            Phase::Conflicts => self
                .plan
                .as_ref()
                .into_iter()
                .flat_map(|plan| plan.conflicts.iter())
                .map(|conflict| {
                    let mut entry = self.entry_for(&conflict.installed_path, &conflict.title);
                    entry.description = user_conflict_reason(&conflict.reason);
                    entry.title_suffix = Some("Skipped".to_owned());
                    entry.footer_label = None;
                    entry.footer_change = None;
                    entry
                })
                .collect(),
            Phase::Review => self
                .plan
                .as_ref()
                .into_iter()
                .flat_map(|plan| plan.roots.iter())
                .map(|root| {
                    let mut entry = self.entry_for(&root.installed_path, &root.title);
                    // Every row here is an update; a badge would repeat the popup title.
                    entry.title_suffix = None;
                    entry.footer_label = None;
                    entry.footer_change = Some((
                        root.current_version.clone(),
                        root.target.version_number.clone(),
                    ));
                    entry
                })
                .collect(),
            Phase::Checking | Phase::Applying => Vec::new(),
        };
        self.list.warning_descriptions = matches!(self.phase, Phase::Conflicts);
        self.list.set_entries(entries);
    }

    fn entry_for(&self, path: &Path, title: &str) -> ContentEntry {
        self.source_entries
            .get(path)
            .cloned()
            .unwrap_or_else(|| ContentEntry {
                file_stem: path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or(title)
                    .to_owned(),
                name: title.to_owned(),
                source_slug: None,
                installed_path: Some(path.to_owned()),
                provider_project: None,
                world_details: None,
                title_suffix: None,
                footer_label: None,
                footer_change: None,
                description: String::new(),
                enabled: true,
                icon_bytes: None,
                provider_icon: false,
                provider_description: false,
                path: path.to_owned(),
                icon_lines: Some(crate::instance::content::fallback_icon()),
            })
    }
}

pub fn render(frame: &mut Frame, state: &mut State, picker: &ratatui_image::picker::Picker) {
    if !state.visible() {
        return;
    }
    let theme = crate::config::theme::THEME.as_ref();
    if state.phase == Phase::Checking {
        let area = frame.area().centered(
            Constraint::Percentage(40),
            Constraint::Length(5.min(frame.area().height.saturating_sub(4))),
        );
        let popup = PopupFrame {
            title: crate::tui::widgets::styled_title("Preparing updates", false),
            border_color: theme.accent(),
            bg: Some(theme.surface()),
            keybinds: Some(crate::tui::widgets::popups::keybind_line(&[(
                "Esc", " cancel",
            )])),
            search_line: None,
            content: Box::new(|area, buffer| {
                Paragraph::new("Resolving dependencies…")
                    .style(Style::default().fg(crate::config::theme::THEME.as_ref().text_dim()))
                    .render(area, buffer);
            }),
        };
        frame.render_widget(popup, area);
        return;
    }
    let count = state.list.entries.len() as u16;
    let height = count.saturating_mul(3).saturating_add(2).clamp(5, 20);
    let area = frame.area().centered(
        Constraint::Percentage(60),
        Constraint::Length(height.min(frame.area().height.saturating_sub(4))),
    );
    let (title, keybinds) = match state.phase {
        Phase::Conflicts => (
            "Updates needing attention",
            if state.has_updates() {
                crate::tui::widgets::popups::keybind_line(&[
                    ("j/k", " navigate"),
                    ("Enter", " review updates"),
                    ("h/Esc", " close"),
                ])
            } else {
                crate::tui::widgets::popups::keybind_line(&[
                    ("j/k", " navigate"),
                    ("h/Esc", " close"),
                ])
            },
        ),
        Phase::Review => (
            "Update installed content",
            if state
                .plan
                .as_ref()
                .is_some_and(|plan| !plan.conflicts.is_empty())
            {
                crate::tui::widgets::popups::keybind_line(&[
                    ("j/k", " navigate"),
                    ("h", " back"),
                    ("Enter", " update all"),
                    ("Esc", " close"),
                ])
            } else {
                crate::tui::widgets::popups::keybind_line(&[
                    ("j/k", " navigate"),
                    ("Enter", " update all"),
                    ("Esc", " close"),
                ])
            },
        ),
        Phase::Checking | Phase::Applying => return,
    };
    let popup = PopupFrame {
        title: crate::tui::widgets::styled_title(title, false),
        border_color: theme.accent(),
        bg: Some(theme.surface()),
        keybinds: Some(keybinds),
        search_line: None,
        content: Box::new(|_, _| {}),
    };
    frame.render_widget(popup, area);
    super::list::render(
        frame,
        area.inner(Margin::new(1, 1)),
        &mut state.list,
        true,
        "",
        "",
        picker,
        false,
        true,
    );
}

fn user_conflict_reason(reason: &str) -> String {
    let reason = reason.strip_prefix("Parse error: ").unwrap_or(reason);
    if let Some(details) = reason.strip_prefix("Conflicting selected versions for '")
        && let Some((dependency, _)) = details.split_once("': '")
    {
        return format!("Other selected updates require different versions of {dependency}");
    }
    if let Some(details) = reason.strip_prefix("Conflicting required versions for '")
        && let Some((dependency, _)) = details.split_once("': '")
    {
        return format!("Incompatible required versions of {dependency}");
    }
    reason.to_owned()
}

#[cfg(test)]
#[path = "../../tests/widgets/content/update.rs"]
mod tests;
