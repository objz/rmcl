// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// rendering for the modpack import wizard. same pattern as new_instance:
// snapshot the state, pick the right step renderer, done.

use super::super::LoadState;
use super::super::base::PopupFrame;
use super::state::{DISCOVERY_STATE, IMPORT_STATE, ImportStep, ImportWizardState};
use crate::config::theme::{BORDER_STYLE, THEME};
use crate::tui::app::FocusedArea;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, StatefulWidget, Widget, Wrap,
    },
};

#[cfg(test)]
pub fn render(frame: &mut Frame, area: Rect, focused: FocusedArea) {
    render_inner(frame, area, focused, None);
}

pub fn render_with_picker(
    frame: &mut Frame,
    area: Rect,
    focused: FocusedArea,
    picker: &ratatui_image::picker::Picker,
) {
    render_inner(frame, area, focused, Some(picker));
}

fn render_inner(
    frame: &mut Frame,
    area: Rect,
    _focused: FocusedArea,
    picker: Option<&ratatui_image::picker::Picker>,
) {
    let snapshot = match IMPORT_STATE.lock() {
        Ok(state) => state.clone(),
        Err(e) => {
            tracing::error!("Import state lock poisoned: {}", e);
            ImportWizardState::default()
        }
    };
    if snapshot.step == ImportStep::Discover {
        if let Some(picker) = picker {
            render_discovery(frame, area, picker);
        }
        return;
    }

    let keybinds = step_keybinds(&snapshot);

    let search_line = if snapshot.step == ImportStep::Version {
        snapshot.version_search.title_line()
    } else {
        None
    };

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
                ImportStep::Discover => {}
                ImportStep::Input => render_input_step(&snapshot, chunks[0], buf),
                ImportStep::Fetching => render_fetching_step(chunks[0], buf),
                ImportStep::Version => render_version_step(&snapshot, chunks[0], buf),
                ImportStep::Confirm => render_confirm_step(&snapshot, chunks[0], buf),
            }
        }),
    };

    frame.render_widget(popup, area);
}

pub fn popup_rect(frame_area: Rect) -> Rect {
    let w = Constraint::Percentage(50);
    let step = match IMPORT_STATE.lock() {
        Ok(s) => s.step.clone(),
        Err(_) => ImportStep::Input,
    };

    match step {
        ImportStep::Discover => {
            frame_area.centered(Constraint::Percentage(80), Constraint::Percentage(80))
        }
        ImportStep::Input => {
            let h = 8u16.min(frame_area.height.saturating_sub(4));
            frame_area.centered(w, Constraint::Length(h))
        }
        ImportStep::Fetching => {
            let h = 5u16.min(frame_area.height.saturating_sub(4));
            frame_area.centered(w, Constraint::Length(h))
        }
        ImportStep::Version => {
            let h = (frame_area.height * 2 / 3)
                .max(10)
                .min(frame_area.height.saturating_sub(4));
            frame_area.centered(w, Constraint::Length(h))
        }
        ImportStep::Confirm => {
            let h = 8u16.min(frame_area.height.saturating_sub(4));
            frame_area.centered(w, Constraint::Length(h))
        }
    }
}

fn render_discovery(frame: &mut Frame, area: Rect, picker: &ratatui_image::picker::Picker) {
    let theme = THEME.as_ref();
    Clear.render(area, frame.buffer_mut());
    let Ok(mut state) = DISCOVERY_STATE.lock() else {
        return;
    };
    let keybinds = discovery_keybinds(
        state.project_page_open(),
        state.sort_panel_focused,
        state.filter_version_picker_open,
    );
    let mut title = crate::tui::widgets::styled_title("Browse Modpacks", false);
    let filter_count = state.active_filter_count();
    if filter_count > 0 {
        let label = if filter_count == 1 {
            "1 filter".to_owned()
        } else {
            format!("{filter_count} filters")
        };
        title.spans.extend([
            Span::raw(" "),
            crate::tui::widgets::status_badge(label, theme.warning()),
        ]);
    }
    let mut block = Block::default()
        .title(title)
        .title_bottom(
            super::super::keybind_line(keybinds).alignment(ratatui::layout::Alignment::Right),
        )
        .borders(Borders::ALL)
        .border_type(BORDER_STYLE.to_border_type())
        .border_style(Style::default().fg(theme.accent()))
        .style(Style::default().bg(theme.surface()));
    if let Some(search_line) = state.search.title_line() {
        block = block.title_top(search_line);
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);
    crate::tui::widgets::content::tabs::render_discovery_popup(frame, inner, &mut state, picker);
}

fn discovery_keybinds(
    project_page_open: bool,
    sort_panel_focused: bool,
    filter_version_picker_open: bool,
) -> &'static [(&'static str, &'static str)] {
    if project_page_open {
        &[
            ("j/k", " scroll"),
            ("g/G", " top/bottom"),
            ("v", " versions"),
            ("h", " back"),
        ]
    } else if filter_version_picker_open {
        &[
            ("j/k", " navigate"),
            ("/", " search"),
            ("s", " snapshots"),
            ("r", " reset"),
            ("h", " back"),
            ("Enter", " select"),
        ]
    } else if sort_panel_focused {
        &[
            ("j/k", " navigate"),
            ("Enter", " select"),
            ("r", " reset"),
            ("h/l", " tabs"),
            ("f", " filters"),
        ]
    } else {
        &[
            ("j/k", " navigate"),
            (" [/] ", " pages"),
            ("Enter", " view"),
            ("v", " versions"),
            ("/", " search"),
            ("f", " filters"),
            ("i", " import"),
            ("Esc", " close"),
        ]
    }
}

fn render_input_step(state: &ImportWizardState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let theme = THEME.as_ref();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area);

    let input_line = if state.input.is_empty() {
        Line::from(vec![
            Span::styled(
                "URL, slug, or pack file path...",
                Style::default().fg(theme.text_dim()),
            ),
            Span::styled(
                "\u{2588}",
                Style::default()
                    .fg(theme.text_dim())
                    .add_modifier(Modifier::SLOW_BLINK),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(state.input.clone(), Style::default().fg(theme.text())),
            Span::styled(
                "\u{2588}",
                Style::default()
                    .fg(theme.text_dim())
                    .add_modifier(Modifier::SLOW_BLINK),
            ),
        ])
    };
    Paragraph::new(input_line).render(chunks[0], buf);
}

fn render_fetching_step(area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let theme = THEME.as_ref();
    Paragraph::new("Fetching modpack info...")
        .style(Style::default().fg(theme.text_dim()))
        .render(area, buf);
}

fn render_version_step(state: &ImportWizardState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
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
            let items: Vec<ListItem> = super::state::visible_versions(state)
                .into_iter()
                .map(|version| {
                    let game_ver = version.game_versions.first().cloned().unwrap_or_default();
                    let loader = version.loaders.first().cloned().unwrap_or_default();
                    ListItem::new(state.version_search.highlight_line(
                        &format!("{}  {}  {}", version.version_number, game_ver, loader),
                        Style::default().fg(theme.text()),
                    ))
                })
                .collect();

            let list = List::new(items)
                .highlight_style(
                    Style::default()
                        .fg(theme.accent())
                        .add_modifier(Modifier::BOLD),
                )
                .highlight_symbol("\u{25b6} ");

            let mut list_state = ListState::default().with_selected(Some(state.version_idx));
            StatefulWidget::render(list, area, buf, &mut list_state);
        }
    }
}

fn render_confirm_step(state: &ImportWizardState, area: Rect, buf: &mut ratatui::buffer::Buffer) {
    let theme = THEME.as_ref();
    let summary = match &state.summary {
        Some(s) => s,
        None => {
            Paragraph::new("No summary available")
                .style(Style::default().fg(theme.text_dim()))
                .render(area, buf);
            return;
        }
    };

    let label_style = Style::default().fg(theme.text_dim());

    let loader_display = if let Some(ref lv) = summary.loader_version {
        format!("{} {}", summary.loader, lv)
    } else {
        summary.loader.to_string()
    };

    Paragraph::new(vec![
        Line::from(vec![
            Span::styled("Name: ", label_style),
            Span::raw(summary.name.clone()),
        ]),
        Line::from(vec![
            Span::styled("Pack Version: ", label_style),
            Span::raw(summary.pack_version.clone()),
        ]),
        Line::from(vec![
            Span::styled("MC Version: ", label_style),
            Span::raw(summary.game_version.clone()),
        ]),
        Line::from(vec![
            Span::styled("Loader: ", label_style),
            Span::raw(loader_display),
        ]),
        Line::from(vec![
            Span::styled("Mods: ", label_style),
            Span::raw(format!("{} files", summary.mod_count)),
        ]),
        Line::from(vec![
            Span::styled("Overrides: ", label_style),
            Span::raw(format!("{} files", summary.override_count)),
        ]),
    ])
    .style(Style::default().fg(theme.text()))
    .wrap(Wrap { trim: true })
    .render(area, buf);
}

fn wizard_title(state: &ImportWizardState) -> Line<'static> {
    use crate::tui::widgets::styled_title;
    styled_title(
        if state.step == ImportStep::Confirm {
            "Install Modpack"
        } else {
            "Import Modpack"
        },
        false,
    )
}

fn step_keybinds(state: &ImportWizardState) -> Line<'static> {
    use super::super::keybind_line;
    match state.step {
        ImportStep::Discover => Line::default(),
        ImportStep::Input => keybind_line(&[("Enter", " fetch"), ("Esc", " back")]),
        ImportStep::Fetching => keybind_line(&[("Esc", " cancel")]),
        ImportStep::Version => {
            keybind_line(&[("/", " search"), ("h", " back"), ("Enter", " select")])
        }
        ImportStep::Confirm => keybind_line(&[("h", " back"), ("Enter", " install")]),
    }
}

#[cfg(test)]
#[path = "../../../tests/widgets/popups/import_modpack/render.rs"]
mod tests;
