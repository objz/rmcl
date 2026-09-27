// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// the "overview" panel showing download/install progress.
// shows a gauge when the total is known, or a spinner when it's not.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, Borders, Gauge, Paragraph},
};
use throbber_widgets_tui::{Throbber, ThrobberState};

use crate::config::theme::{BORDER_STYLE, THEME};
use crate::feedback::progress::PROGRESS;
use crate::tui::app::FocusedArea;

use super::styled_title;

pub fn render(
    frame: &mut Frame,
    area: Rect,
    focused: FocusedArea,
    throbber_state: &mut ThrobberState,
    discovery_activity: Option<&str>,
) {
    let theme = THEME.as_ref();
    let border_color = if focused == FocusedArea::Overview {
        theme.accent()
    } else {
        theme.border()
    };

    let block = Block::default()
        .title(styled_title("Overview", true))
        .borders(Borders::ALL)
        .border_type(BORDER_STYLE.to_border_type())
        .border_style(Style::default().fg(border_color));

    let state = match PROGRESS.lock() {
        Ok(s) => s.clone(),
        Err(_) => {
            frame.render_widget(
                Paragraph::new(Span::styled("Ready", Style::default().fg(theme.text_dim())))
                    .block(block),
                area,
            );
            return;
        }
    };

    if state.current_action.is_none() && discovery_activity.is_none() {
        frame.render_widget(
            Paragraph::new(Span::styled("Ready", Style::default().fg(theme.text_dim())))
                .block(block),
            area,
        );
        return;
    }

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let action_text = state
        .current_action
        .as_deref()
        .or(discovery_activity)
        .unwrap_or("");
    let sub_text = state.sub_action.as_deref().unwrap_or("");

    match state.progress {
        Some((current, total)) if total > 0 => {
            let chunks = Layout::vertical([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);

            let ratio = (current as f64 / total as f64).min(1.0);
            let gauge = Gauge::default()
                .gauge_style(
                    Style::default()
                        .fg(theme.success())
                        .bg(theme.surface())
                        .add_modifier(Modifier::BOLD),
                )
                .percent((ratio * 100.0) as u16);
            frame.render_widget(gauge, chunks[0]);
            frame.render_widget(
                Paragraph::new(action_text).style(Style::default().fg(theme.text())),
                chunks[1],
            );
            if !sub_text.is_empty() {
                frame.render_widget(
                    Paragraph::new(sub_text).style(Style::default().fg(theme.text_dim())),
                    chunks[2],
                );
            }
        }
        _ => {
            let chunks =
                Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(inner);
            let throbber = Throbber::default()
                .label(action_text)
                .style(Style::default().fg(theme.text()))
                .throbber_style(
                    Style::default()
                        .fg(theme.text_dim())
                        .add_modifier(Modifier::BOLD),
                );
            frame.render_stateful_widget(throbber, chunks[0], throbber_state);
            if !sub_text.is_empty() {
                frame.render_widget(
                    Paragraph::new(sub_text).style(Style::default().fg(theme.text_dim())),
                    chunks[1],
                );
            }
        }
    }
}
