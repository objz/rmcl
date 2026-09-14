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
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
};

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
    let [area] = Layout::vertical([Constraint::Length((count as u16 + 5).clamp(8, 20))])
        .flex(Flex::Center)
        .areas(frame.area());
    let [area] = Layout::horizontal([Constraint::Percentage(62)])
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
                    .highlight_symbol("▌")
                    .highlight_style(Style::default().bg(theme.stripe())),
                area,
                &mut selected,
            );
        }
        Phase::Review => {
            let mut lines = state.plan.as_ref().map_or_else(Vec::new, |plan| {
                vec![
                    Line::from(vec![
                        Span::styled(
                            plan.summary.name.clone(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(format!(
                            "  {} → {}",
                            plan.current_version, plan.target_version
                        )),
                    ]),
                    Line::from(format!("Minecraft: {}", plan.summary.game_version)),
                    Line::from(format!(
                        "Loader: {} {}",
                        plan.summary.loader,
                        plan.summary.loader_version.as_deref().unwrap_or("")
                    )),
                    Line::from(format!("Pack files: {}", plan.summary.mod_count)),
                    Line::from(format!("Overrides: {}", plan.summary.override_count)),
                ]
            });
            if let Some(error) = &state.error {
                lines.push(Line::from(error.clone()).style(Style::default().fg(theme.error())));
            }
            frame.render_widget(Paragraph::new(lines).block(block), area);
        }
    }
}
