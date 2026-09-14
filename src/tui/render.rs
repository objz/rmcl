// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// layout and rendering. the main frame is split into:
//   left 20%: instance sidebar
//   right 80%: title bar + content area + bottom bar (account / details / status)
// popups and error toasts render on top of everything.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
};
use tachyonfx::{EffectRenderer, Interpolation, Motion, fx};

use super::app::{App, ErrorEffectState, FocusedArea};
use super::widgets::{
    self, popups::confirm as confirm_popup, popups::import_modpack, popups::new_instance,
};
use crate::feedback::errors as error_buffer;
use crate::tui::widgets::popups::confirm::{ConfirmPopup, confirm_popup_area};
use crate::tui::widgets::popups::error::{ErrorPopup, popup_area};

impl App {
    pub(super) fn render_frame(&mut self, frame: &mut Frame) {
        self.render_frame_with_content(frame, true);
    }

    pub(super) fn render_migration_frame(&mut self, frame: &mut Frame) {
        self.render_frame_with_content(frame, false);
    }

    fn render_frame_with_content(&mut self, frame: &mut Frame, load_content: bool) {
        use crate::config::theme::THEME;
        use ratatui::style::Style;
        use ratatui::widgets::Block;

        let theme = THEME.as_ref();
        frame.render_widget(
            Block::default().style(Style::default().bg(theme.background())),
            frame.area(),
        );

        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(20), Constraint::Percentage(80)])
            .split(frame.area());

        widgets::instances::render(frame, chunks[0], self.focused, &mut self.instances_state);

        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(10),
                Constraint::Length(5),
            ])
            .split(chunks[1]);

        widgets::content::title(
            frame,
            main_chunks[0],
            self.focused,
            self.instances_state.selected_instance(),
            &mut self.throbber_state,
        );
        let world_quick_play_supported = self.selected_instance_supports_quick_play();
        if let Some((_, world)) = self.open_world_datapacks.as_ref() {
            let valid = self
                .instances_state
                .selected_instance()
                .is_some_and(|instance| {
                    world.starts_with(
                        crate::storage::InstancePaths::new(
                            self.instance_manager.instances_dir.join(&instance.name),
                        )
                        .minecraft()
                        .join("saves"),
                    )
                });
            if !valid {
                self.open_world_datapacks = None;
            }
        }
        if load_content {
            let has_modpack_update = self.instances_state.selected_modpack_update().is_some();
            widgets::content::render(
                frame,
                main_chunks[1],
                self.focused,
                self.content_tab,
                self.content_mode,
                self.instances_state.selected_instance(),
                has_modpack_update,
                &mut self.mods_state,
                &mut self.mods_discovery_state,
                &mut self.resource_packs_state,
                &mut self.resource_packs_discovery_state,
                &mut self.shaders_state,
                &mut self.shaders_discovery_state,
                &mut self.datapacks_discovery_state,
                &mut self.worlds_state,
                &mut self.world_datapacks_state,
                self.open_world_datapacks.as_ref(),
                &mut self.screenshots_state,
                &mut self.logs_state,
                &self.instance_manager.instances_dir,
                &self.picker,
                world_quick_play_supported,
            );
        } else {
            frame.render_widget(
                Block::bordered()
                    .border_type(crate::config::theme::BORDER_STYLE.to_border_type())
                    .border_style(Style::default().fg(theme.border())),
                main_chunks[1],
            );
        }

        let bottom_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(24),
                Constraint::Percentage(46),
                Constraint::Percentage(30),
            ])
            .split(main_chunks[2]);

        widgets::account::render(
            frame,
            bottom_chunks[0],
            self.focused,
            &mut self.account_state,
        );
        widgets::settings::render(
            frame,
            bottom_chunks[1],
            self.focused,
            &mut self.settings_state,
            self.instances_state.selected_instance(),
        );
        widgets::status::render(
            frame,
            bottom_chunks[2],
            self.focused,
            &mut self.throbber_state,
        );

        if self.focused == FocusedArea::OverviewExpanded {
            self.render_log_overlay(frame);
        }

        if self.focused == FocusedArea::InstanceSettings
            && let Some(state) = self.instance_settings.as_mut()
        {
            let area = widgets::popups::instance_settings::popup_rect(frame.area(), state);
            widgets::popups::instance_settings::render(frame, area, state);
        }

        if self.focused == FocusedArea::GlobalSettings
            && let Some(state) = self.global_settings.as_mut()
        {
            let area = widgets::popups::global_settings::popup_rect(frame.area(), state);
            widgets::popups::global_settings::render(frame, area, state);
        }

        // error toasts stack from the top, each one below the previous
        let all_errors = error_buffer::peek_all_errors();
        self.sync_error_effects(&all_errors);
        let mut next_y: u16 = 1;
        for event in all_errors {
            let elapsed_ms = event.pushed_at.elapsed().as_millis();
            if let Some(area) = popup_area(frame.area(), &event.message, next_y, elapsed_ms) {
                next_y = next_y.saturating_add(area.height + 1);
                frame.render_widget(ErrorPopup::new(event.clone()), area);
                self.render_error_effect(frame, area, &event, elapsed_ms);
            }
        }

        if self.instances_state.show_popup {
            let area = new_instance::popup_rect(frame.area());
            new_instance::render(frame, area, self.focused);
        }

        if self.instances_state.show_import_popup {
            let area = import_modpack::popup_rect(frame.area());
            import_modpack::render_with_picker(frame, area, self.focused, &self.picker);
        }

        if self.focused == FocusedArea::ConfirmDelete
            && let Some(target) = confirm_popup::pending_target()
        {
            let area = confirm_popup_area(frame.area(), &target);
            frame.render_widget(ConfirmPopup::for_target(&target), area);
        }

        if let Some(conflict) = &self.provider_conflict {
            render_provider_conflict(frame, conflict);
        }

        if let Some(update) = self.content_update_popup.as_mut() {
            widgets::content::update::render(frame, update, &self.picker);
        }

        if let Some(state) = self.modpack_versions_state.as_mut() {
            widgets::content::tabs::render_version_popup(
                frame,
                main_chunks[1],
                state,
                &self.picker,
            );
        }

        if let Some(update) = &self.modpack_update_popup {
            widgets::popups::modpack_update::render(frame, update);
        }
    }

    // full-screen log viewer with search highlighting and auto-scroll.
    // auto-sticks to the bottom unless the user scrolled up manually
    fn render_log_overlay(&mut self, frame: &mut Frame) {
        use crate::config::theme::{BORDER_STYLE, THEME};
        use crate::tui::logging::get_app_logs;
        use ratatui::{
            layout::{Alignment, Margin},
            style::{Modifier, Style},
            text::Line,
            widgets::{Block, Clear, Paragraph, Scrollbar, ScrollbarOrientation},
        };

        let theme = THEME.as_ref();
        let area = frame.area();
        let overlay = area.inner(Margin::new(1, 1));

        frame.render_widget(Clear, overlay);

        let all_lines = get_app_logs();
        let filtered: Vec<&String> = all_lines
            .iter()
            .filter(|l| self.log_overlay_search.matches(l))
            .collect();

        let visible_height = overlay.height.saturating_sub(2) as usize;
        let was_at_bottom =
            self.log_overlay_scroll >= self.log_overlay_max_scroll.saturating_sub(1);
        self.log_overlay_max_scroll = filtered.len().saturating_sub(visible_height);
        if was_at_bottom || self.log_overlay_scroll > self.log_overlay_max_scroll {
            self.log_overlay_scroll = self.log_overlay_max_scroll;
        }
        self.log_overlay_scrollbar =
            ratatui::widgets::ScrollbarState::new(self.log_overlay_max_scroll)
                .position(self.log_overlay_scroll);

        let mut block = Block::bordered()
            .title_top(
                Line::from(" Logs ").style(
                    Style::default()
                        .fg(theme.text())
                        .add_modifier(Modifier::BOLD),
                ),
            )
            .title_bottom(
                crate::tui::widgets::popups::keybind_line(&[("O", " close"), ("/", " search")])
                    .alignment(Alignment::Right),
            )
            .border_type(BORDER_STYLE.to_border_type())
            .border_style(Style::default().fg(theme.accent()))
            .style(Style::default().bg(theme.background()));

        if let Some(sl) = self.log_overlay_search.title_line() {
            block = block.title_top(sl);
        }

        let inner = block.inner(overlay);
        frame.render_widget(block, overlay);

        let search = &self.log_overlay_search;
        let styled: Vec<Line> = filtered
            .iter()
            .skip(self.log_overlay_scroll)
            .take(visible_height)
            .map(|line| {
                let style = if line.contains("ERROR") || line.contains("FATAL") {
                    Style::default().fg(theme.error())
                } else if line.contains("WARN") {
                    Style::default().fg(theme.warning())
                } else if line.contains("DEBUG") || line.contains("TRACE") {
                    Style::default().fg(theme.text_dim())
                } else {
                    Style::default().fg(theme.text())
                };
                search.highlight_line(line, style)
            })
            .collect();

        frame.render_widget(Paragraph::new(styled), inner);

        let scrollbar_area = ratatui::layout::Rect {
            x: overlay.x + overlay.width.saturating_sub(1),
            y: overlay.y + 1,
            width: 1,
            height: overlay.height.saturating_sub(2),
        };
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
            &mut self.log_overlay_scrollbar,
        );
    }

    // keeps the effect map in sync with current errors: removes effects for
    // dismissed errors and creates slide-in effects for new ones
    fn sync_error_effects(&mut self, events: &[error_buffer::ErrorEvent]) {
        use crate::config::theme::THEME;
        let theme = THEME.as_ref();
        let bg = theme.background();
        let active_ids: std::collections::HashSet<u64> =
            events.iter().map(|event| event.id).collect();
        self.error_effects.retain(|id, _| active_ids.contains(id));

        for event in events {
            self.error_effects.entry(event.id).or_insert_with(|| {
                ErrorEffectState::SlidingIn(
                    fx::slide_in(
                        Motion::RightToLeft,
                        4,
                        0,
                        bg,
                        (250, Interpolation::CubicOut),
                    ),
                    std::time::Instant::now(),
                )
            });
        }
    }

    // drives the slide-in / idle / slide-out state machine for each error toast.
    // transitions to FadingOut at the configured slide-start time
    fn render_error_effect(
        &mut self,
        frame: &mut Frame,
        area: ratatui::layout::Rect,
        event: &error_buffer::ErrorEvent,
        elapsed_ms: u128,
    ) {
        use crate::config::SETTINGS;
        use crate::config::theme::THEME;
        let theme = THEME.as_ref();
        let bg = theme.background();
        let (fly_out_ms, fly_start_ms) = {
            let settings = SETTINGS.read();
            (
                settings.ui.error_fly_out_ms as u128,
                settings.ui.error_slide_start_ms as u128,
            )
        };

        if elapsed_ms >= fly_start_ms {
            let entry = self
                .error_effects
                .entry(event.id)
                .or_insert(ErrorEffectState::Idle);
            if !matches!(entry, ErrorEffectState::FadingOut(..)) {
                *entry = ErrorEffectState::FadingOut(
                    fx::slide_out(
                        Motion::LeftToRight,
                        4,
                        0,
                        bg,
                        (fly_out_ms as u32, Interpolation::CubicIn),
                    ),
                    std::time::Instant::now(),
                );
            }
        }

        if let Some(effect_state) = self.error_effects.get_mut(&event.id) {
            match effect_state {
                ErrorEffectState::SlidingIn(effect, started) => {
                    let dt = started.elapsed().as_millis() as u32;
                    if effect.running() {
                        frame.render_effect(
                            effect,
                            area,
                            tachyonfx::Duration::from_millis(dt.min(32)),
                        );
                        *started = std::time::Instant::now();
                    } else {
                        *effect_state = ErrorEffectState::Idle;
                    }
                }
                ErrorEffectState::Idle => {}
                ErrorEffectState::FadingOut(effect, started) => {
                    let dt = started.elapsed().as_millis() as u32;
                    if effect.running() {
                        frame.render_effect(
                            effect,
                            area,
                            tachyonfx::Duration::from_millis(dt.min(32)),
                        );
                        *started = std::time::Instant::now();
                    }
                }
            }
        }
    }
}

fn render_provider_conflict(frame: &mut Frame, conflict: &super::app::ProviderConflictState) {
    use crate::config::theme::{BORDER_STYLE, THEME};
    use ratatui::{
        layout::{Constraint, Flex, Layout},
        style::{Modifier, Style},
        text::Line,
        widgets::{Block, Borders, Clear, List, ListItem, ListState},
    };

    let theme = THEME.as_ref();
    let [area] = Layout::vertical([Constraint::Length(
        (conflict.candidates.len() as u16 + 4).min(frame.area().height.saturating_sub(2)),
    )])
    .flex(Flex::Center)
    .areas(frame.area());
    let [area] = Layout::horizontal([Constraint::Percentage(58)])
        .flex(Flex::Center)
        .areas(area);
    frame.render_widget(Clear, area);

    let title = conflict
        .relative_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("content file");
    let items = conflict.candidates.iter().map(|candidate| {
        ListItem::new(Line::from(format!(
            "{}  {}  {}",
            candidate.provider, candidate.project_id, candidate.version_id
        )))
    });
    let mut state = ListState::default().with_selected(Some(conflict.selected));
    let mut block = Block::default()
        .title(Line::from(format!(" Choose provider for {title} ")))
        .borders(Borders::ALL)
        .border_type(BORDER_STYLE.to_border_type())
        .border_style(Style::default().fg(theme.accent()));
    if crate::tui::widgets::popups::shortcut_hints_visible(
        crate::config::settings::ShortcutHintScope::Popups,
    ) {
        block = block.title_bottom(Line::from(" [j/k] select  [Enter] use  [Esc] later "));
    }
    let list = List::new(items)
        .block(block)
        .style(Style::default().fg(theme.text()).bg(theme.surface()))
        .highlight_style(
            Style::default()
                .fg(theme.accent())
                .bg(theme.stripe())
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▌");
    frame.render_stateful_widget(list, area, &mut state);
}
