// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ratatui::{
    Frame,
    layout::{Constraint, Flex, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use super::base::{summary_heading, summary_row};
use crate::config::theme::{BORDER_STYLE, THEME};
use crate::instance::import::refresh::RefreshPlan;

pub enum PendingResult {
    Prepared(Box<Result<RefreshPlan, String>>),
    Applied(Box<Result<crate::instance::InstanceConfig, String>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Preparing,
    Conflicts,
    Review,
    Applying,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Update,
    Change,
    Reinstall,
}

pub struct State {
    pub phase: Phase,
    pub action: Action,
    pub plan: Option<RefreshPlan>,
    pub selected: usize,
    pub replace: Vec<bool>,
    pub error: Option<String>,
    pub pending: Arc<Mutex<Vec<PendingResult>>>,
    pub completed: Option<crate::instance::InstanceConfig>,
}

impl State {
    pub fn preparing(action: Action) -> Self {
        Self {
            phase: Phase::Preparing,
            action,
            plan: None,
            selected: 0,
            replace: Vec::new(),
            error: None,
            pending: Arc::new(Mutex::new(Vec::new())),
            completed: None,
        }
    }

    pub fn drain(&mut self) {
        let pending = match self.pending.lock() {
            Ok(mut pending) => pending.drain(..).collect::<Vec<_>>(),
            Err(_) => return,
        };
        for result in pending {
            match result {
                PendingResult::Prepared(result) => match *result {
                    Ok(plan) => {
                        self.replace = vec![false; plan.conflicts.len()];
                        self.phase = if plan.conflicts.is_empty() {
                            Phase::Review
                        } else {
                            Phase::Conflicts
                        };
                        self.plan = Some(plan);
                        self.error = None;
                    }
                    Err(error) => {
                        self.phase = Phase::Review;
                        self.error = Some(error);
                    }
                },
                PendingResult::Applied(result) => match *result {
                    Ok(instance) => self.completed = Some(instance),
                    Err(error) => {
                        self.phase = Phase::Review;
                        self.error = Some(error);
                    }
                },
            }
        }
    }

    pub fn replacements(&self) -> HashSet<PathBuf> {
        self.plan
            .as_ref()
            .into_iter()
            .flat_map(|plan| plan.conflicts.iter())
            .enumerate()
            .filter(|(index, _)| self.replace.get(*index) == Some(&true))
            .map(|(_, path)| path.clone())
            .collect()
    }
}

pub fn render(frame: &mut Frame, state: &State) {
    let theme = THEME.as_ref();
    let count = state
        .plan
        .as_ref()
        .map_or(1, |plan| plan.conflicts.len().max(5));
    let mut lines = state.plan.as_ref().map_or_else(Vec::new, |plan| {
        review_lines(&plan.summary, &plan.current_version, &plan.target_version)
    });
    if let Some(error) = &state.error {
        lines.push(Line::from(error.clone()).style(Style::default().fg(theme.error())));
    }
    let summary = Paragraph::new(lines).wrap(Wrap { trim: false });
    let width = frame.area().width * if state.phase == Phase::Review { 68 } else { 62 } / 100;
    let height = if state.phase == Phase::Review {
        summary.line_count(width.saturating_sub(2)) as u16 + 2
    } else {
        (count as u16 + 5).clamp(8, 20)
    };
    let [area] = Layout::vertical([Constraint::Length(
        height.min(frame.area().height.saturating_sub(4)),
    )])
    .flex(Flex::Center)
    .areas(frame.area());
    let [area] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    frame.render_widget(Clear, area);
    let action = match state.action {
        Action::Update => "update",
        Action::Change => "change",
        Action::Reinstall => "reinstall",
    };
    let (title, footer) = match state.phase {
        Phase::Preparing => match state.action {
            Action::Update => (" Prepare modpack update ", " [Esc] cancel "),
            Action::Change => (" Prepare modpack change ", " [Esc] cancel "),
            Action::Reinstall => (" Prepare modpack reinstall ", " [Esc] cancel "),
        },
        Phase::Conflicts => (
            " Resolve modpack conflicts ",
            " [j/k] select  [Space] keep/replace  [Enter] continue  [Esc] cancel ",
        ),
        Phase::Review => match state.action {
            Action::Update => (" Update modpack ", " [Enter] update  [Esc] cancel "),
            Action::Change => (" Change modpack ", " [Enter] change  [Esc] cancel "),
            Action::Reinstall => (" Reinstall modpack ", " [Enter] reinstall  [Esc] cancel "),
        },
        Phase::Applying => match state.action {
            Action::Update => (" Updating modpack ", " Please wait "),
            Action::Change => (" Changing modpack ", " Please wait "),
            Action::Reinstall => (" Reinstalling modpack ", " Please wait "),
        },
    };
    let mut block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BORDER_STYLE.to_border_type())
        .border_style(Style::default().fg(theme.accent()))
        .style(Style::default().fg(theme.text()).bg(theme.surface()));
    if state.phase == Phase::Applying
        || super::shortcut_hints_visible(crate::config::settings::ShortcutHintScope::Popups)
    {
        block = block.title_bottom(Line::from(footer).centered());
    }
    match state.phase {
        Phase::Preparing => frame.render_widget(
            Paragraph::new(format!("Preparing the selected modpack {action}…")).block(block),
            area,
        ),
        Phase::Applying => frame.render_widget(
            Paragraph::new("Preserving user files and activating the staged pack…").block(block),
            area,
        ),
        Phase::Conflicts => {
            let items = state
                .plan
                .as_ref()
                .into_iter()
                .flat_map(|plan| plan.conflicts.iter())
                .enumerate()
                .map(|(index, path)| {
                    let action = if state.replace.get(index) == Some(&true) {
                        "Replace"
                    } else {
                        "Keep"
                    };
                    ListItem::new(Line::from(vec![
                        Span::raw(format!("• {}  ", path.display())),
                        Span::styled(action, Style::default().fg(theme.accent())),
                    ]))
                });
            let mut selected = ListState::default().with_selected(Some(state.selected));
            frame.render_stateful_widget(
                List::new(items)
                    .block(block)
                    .highlight_symbol(Span::styled("▌", Style::default().fg(theme.accent())))
                    .highlight_style(
                        Style::default()
                            .fg(theme.text())
                            .bg(theme.stripe())
                            .add_modifier(Modifier::BOLD),
                    ),
                area,
                &mut selected,
            );
        }
        Phase::Review => {
            frame.render_widget(summary.block(block), area);
        }
    }
}

fn review_lines(
    summary: &crate::instance::import::ImportSummary,
    current: &str,
    target: &str,
) -> Vec<Line<'static>> {
    vec![
        summary_heading("Pack"),
        summary_row("Pack name", summary.name.clone()),
        summary_row("Pack version", format!("{current} → {target}")),
        Line::default(),
        summary_heading("Runtime"),
        summary_row("Minecraft", summary.game_version.clone()),
        summary_row(
            "Loader",
            format!(
                "{} {}",
                summary.loader,
                summary.loader_version.as_deref().unwrap_or("")
            )
            .trim_end()
            .to_owned(),
        ),
        Line::default(),
        summary_heading("Files"),
        summary_row("Pack files", summary.mod_count.to_string()),
        summary_row("Overrides", summary.override_count.to_string()),
    ]
}

#[cfg(test)]
#[path = "../../tests/widgets/popups/modpack_update.rs"]
mod tests;
