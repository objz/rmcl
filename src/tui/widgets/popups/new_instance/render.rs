// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// rendering for the new instance wizard. each step gets its own render fn
// and the popup resizes itself based on which step is active.

use super::super::LoadState;
use super::state::{
    WIZARD_STATE, WizardState, WizardStep, clamp_loader_version_index, clamp_version_index,
    ensure_loader_versions_loaded, ensure_versions_loaded, visible_versions,
};
use crate::config::theme::THEME;
use crate::instance::models::ModLoader;
use crate::tui::app::FocusedArea;
use crate::tui::widgets::popups::base::{PopupFrame, render_summary};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{ListItem, Paragraph, Widget, Wrap},
};
use tui_prompts::State as PromptState;

pub fn render(frame: &mut Frame, area: Rect, _focused: FocusedArea) {
    // grab the lock, kick off any lazy-loading, then clone and release.
    // data fetching happens here (in render) because the wizard is purely
    // reactive: version lists only load when you navigate to that step.
    let snapshot = match WIZARD_STATE.lock() {
        Ok(mut state) => {
            if state.step == WizardStep::Version {
                ensure_versions_loaded(&mut state);
                clamp_version_index(&mut state);
            }

            // vanilla has no loader version, so skip straight to confirm
            if state.step == WizardStep::LoaderVersion {
                if state.selected_loader() == ModLoader::Vanilla {
                    state.step = WizardStep::Confirm;
                } else {
                    clamp_loader_version_index(&mut state);
                    let game_version = state.selected_version().map(|v| v.id.clone());
                    let loader = state.selected_loader();
                    if let Some(game_version) = game_version {
                        ensure_loader_versions_loaded(&mut state, loader, game_version);
                    }
                }
            }

            state.clone()
        }
        Err(e) => {
            tracing::error!("Wizard state lock poisoned: {}", e);
            WizardState::default()
        }
    };

    let keybinds = step_keybinds(&snapshot);

    let search_line = snapshot.version_search.title_line();

    let theme = THEME.as_ref();
    let popup = PopupFrame {
        title: wizard_title(&snapshot),
        border_color: theme.text_dim(),
        bg: Some(theme.surface()),
        keybinds: Some(keybinds),
        search_line,
        content: Box::new(move |popup_area, buf| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1)])
                .split(popup_area);

            match snapshot.step {
                WizardStep::Name => render_name_step(&snapshot, chunks[0], buf),
                WizardStep::Version => render_version_step(&snapshot, chunks[0], buf),
                WizardStep::Loader => render_loader_step(&snapshot, chunks[0], buf),
                WizardStep::LoaderVersion => render_loader_version_step(&snapshot, chunks[0], buf),
                WizardStep::Confirm => render_confirm_step(&snapshot, chunks[0], buf),
            }
        }),
    };

    frame.render_widget(popup, area);
}

pub fn popup_rect(frame_area: Rect) -> Rect {
    use ratatui::layout::Constraint;

    let step = match WIZARD_STATE.lock() {
        Ok(s) => s.step.clone(),
        Err(_) => WizardStep::Name,
    };

    let w = Constraint::Percentage(50);

    match step {
        WizardStep::Name => {
            let h = 6u16.min(frame_area.height.saturating_sub(4));
            frame_area.centered(w, Constraint::Length(h))
        }
        WizardStep::Version | WizardStep::LoaderVersion => {
            let h = (frame_area.height * 2 / 3)
                .max(10)
                .min(frame_area.height.saturating_sub(4));
            frame_area.centered(w, Constraint::Length(h))
        }
        WizardStep::Loader => {
            let h = 9u16.min(frame_area.height.saturating_sub(4));
            frame_area.centered(w, Constraint::Length(h))
        }
        WizardStep::Confirm => {
            let h = 6u16.min(frame_area.height.saturating_sub(4));
            frame_area.centered(w, Constraint::Length(h))
        }
    }
}

fn wizard_title(_state: &WizardState) -> Line<'static> {
    use crate::tui::widgets::styled_title;
    styled_title("New Instance", false)
}

fn step_keybinds(state: &WizardState) -> ratatui::text::Line<'static> {
    use crate::tui::widgets::popups::keybind_line;
    match state.step {
        WizardStep::Name => keybind_line(&[("Enter", " continue")]),
        WizardStep::Loader => keybind_line(&[("h", " back"), ("Enter", " select")]),
        WizardStep::Version => keybind_line(&[
            ("/", " search"),
            ("s", " snap"),
            ("h", " back"),
            ("Enter", " select"),
        ]),
        WizardStep::LoaderVersion => keybind_line(&[("h", " back"), ("Enter", " select")]),
        WizardStep::Confirm => keybind_line(&[("h", " back"), ("Enter", " create")]),
    }
}

fn render_name_step(state: &WizardState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let theme = THEME.as_ref();
    let value = state.name_state.value();
    // \u{2588} is the full block char used as a fake blinking cursor
    let line = if value.is_empty() {
        Line::from(vec![
            Span::styled("Instance name...", Style::default().fg(theme.text_dim())),
            Span::styled(
                "\u{2588}",
                Style::default()
                    .fg(theme.text_dim())
                    .add_modifier(Modifier::SLOW_BLINK),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(value, Style::default().fg(theme.text())),
            Span::styled(
                "\u{2588}",
                Style::default()
                    .fg(theme.text_dim())
                    .add_modifier(Modifier::SLOW_BLINK),
            ),
        ])
    };

    Paragraph::new(line).render(area, buf);
}

fn render_version_step(state: &WizardState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let theme = THEME.as_ref();
    match &state.versions {
        LoadState::Idle | LoadState::Loading => {
            Paragraph::new("Loading versions...")
                .style(Style::default().fg(theme.text_dim()))
                .render(area, buf);
        }
        LoadState::Error(message) => {
            Paragraph::new(message.as_str())
                .wrap(Wrap { trim: true })
                .style(Style::default().fg(theme.error()))
                .render(area, buf);
        }
        LoadState::Loaded(_) => {
            let items: Vec<ListItem> = visible_versions(state)
                .into_iter()
                .enumerate()
                .map(|(index, version)| {
                    let suffix = if version.stable {
                        String::new()
                    } else {
                        " (snapshot)".to_string()
                    };
                    ListItem::new(state.version_search.highlight_line(
                        &format!("{}{}", version.id, suffix),
                        Style::default().fg(if index == state.version_idx {
                            theme.accent()
                        } else {
                            theme.text()
                        }),
                    ))
                })
                .collect();

            super::super::select_list::render(items, state.version_idx, area, buf);
        }
    }
}

fn render_loader_step(state: &WizardState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let theme = THEME.as_ref();
    let items: Vec<ListItem> = super::super::select_list::MOD_LOADERS
        .into_iter()
        .map(|loader| {
            ListItem::new(Line::from(Span::styled(
                loader.to_string(),
                Style::default().fg(theme.text()),
            )))
        })
        .collect();

    super::super::select_list::render(items, state.loader_idx, area, buf);
}

fn render_loader_version_step(state: &WizardState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let theme = THEME.as_ref();
    if state.selected_loader() == ModLoader::Vanilla {
        Paragraph::new("Vanilla has no loader version.")
            .style(Style::default().fg(theme.text_dim()))
            .render(area, buf);
        return;
    }

    match &state.loader_versions {
        LoadState::Idle | LoadState::Loading => {
            Paragraph::new(format!("Loading {} versions...", state.selected_loader()))
                .style(Style::default().fg(theme.text_dim()))
                .render(area, buf);
        }
        LoadState::Error(message) => {
            Paragraph::new(message.as_str())
                .wrap(Wrap { trim: true })
                .style(Style::default().fg(theme.error()))
                .render(area, buf);
        }
        LoadState::Loaded(versions) => {
            let items: Vec<ListItem> = versions
                .iter()
                .map(|version| {
                    ListItem::new(Line::from(Span::styled(
                        version.clone(),
                        Style::default().fg(theme.text()),
                    )))
                })
                .collect();

            super::super::select_list::render(items, state.loader_version_idx, area, buf);
        }
    }
}

fn render_confirm_step(state: &WizardState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let game_version = state
        .selected_version()
        .map(|version| version.id.as_str())
        .unwrap_or("<not selected>");
    let loader = state.selected_loader();
    let loader_version = if loader == ModLoader::Vanilla {
        "n/a".to_string()
    } else {
        state
            .selected_loader_version()
            .unwrap_or_else(|| "<not selected>".to_string())
    };
    let loader = loader.to_string();
    render_summary(
        &[
            ("Name", state.name_state.value()),
            ("MC", game_version),
            ("Loader", &loader),
            ("Loader version", &loader_version),
        ],
        area,
        buf,
    );
}

#[cfg(test)]
#[path = "../../../tests/widgets/popups/new_instance/render.rs"]
mod tests;
