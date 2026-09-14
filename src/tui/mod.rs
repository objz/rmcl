// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// tui entrypoint: sets up the terminal, runs the app, cleans up on exit.

pub mod app;
mod event;
mod input;
pub mod logging;
mod render;
pub mod widgets;

use crate::feedback::request_redraw;

#[cfg(test)]
pub(crate) mod tests;

pub type Tui = ratatui::DefaultTerminal;

pub async fn show() -> color_eyre::Result<()> {
    // restore the terminal before printing a panic. without this, a panic
    // leaves raw mode + alternate screen active and looks like a freeze
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::DisableMouseCapture,
            crossterm::event::PopKeyboardEnhancementFlags
        );
        ratatui::restore();
        default_hook(info);
    }));

    let mut terminal = ratatui::init();

    // opt into enhanced keyboard protocol to distinguish key press vs release
    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::EnableMouseCapture,
        crossterm::event::PushKeyboardEnhancementFlags(
            crossterm::event::KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                | crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
        )
    );

    let result = async {
        // figure out the terminal's font cell size for rendering images.
        // falls back to halfblock characters if the terminal doesn't respond
        let mut picker = ratatui_image::picker::Picker::from_query_stdio()
            .unwrap_or_else(|_| ratatui_image::picker::Picker::halfblocks());
        let detected_protocol = picker.protocol_type();
        let requested_protocol = match crate::config::SETTINGS.read().ui.image_protocol {
            crate::config::settings::ImageProtocol::Auto => detected_protocol,
            crate::config::settings::ImageProtocol::Halfblocks
            | crate::config::settings::ImageProtocol::Quadrants => {
                ratatui_image::picker::ProtocolType::Halfblocks
            }
            crate::config::settings::ImageProtocol::Kitty
                if detected_protocol == ratatui_image::picker::ProtocolType::Kitty =>
            {
                ratatui_image::picker::ProtocolType::Kitty
            }
            crate::config::settings::ImageProtocol::Iterm2
                if detected_protocol == ratatui_image::picker::ProtocolType::Iterm2 =>
            {
                ratatui_image::picker::ProtocolType::Iterm2
            }
            _ => ratatui_image::picker::ProtocolType::Halfblocks,
        };
        picker.set_protocol_type(requested_protocol);

        let mut app = app::App::new(picker, detected_protocol);
        match run_layout_migration_screen(&mut terminal, &mut app).await? {
            MigrationScreenOutcome::NotNeeded => {}
            MigrationScreenOutcome::Migrated => {
                // the modal uses pre-migration state as its background. rebuild the
                // app after confirmation so no instance or profile data stays stale
                let picker = app.into_picker();
                app = app::App::new(picker, detected_protocol);
            }
            MigrationScreenOutcome::Quit => return Ok(()),
        }
        app.run(&mut terminal).await
    }
    .await;

    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::event::DisableMouseCapture,
        crossterm::event::PopKeyboardEnhancementFlags
    );

    ratatui::restore();
    result
}

enum MigrationScreenOutcome {
    NotNeeded,
    Migrated,
    Quit,
}

async fn run_layout_migration_screen(
    terminal: &mut Tui,
    app: &mut app::App,
) -> color_eyre::Result<MigrationScreenOutcome> {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    let instances_dir = crate::config::SETTINGS.read().paths.resolve_instances_dir();
    let meta_dir = crate::config::SETTINGS.read().paths.resolve_meta_dir();
    let config = crate::config::get_config_path().join("config.toml");
    if !crate::layout_migration::is_needed(&instances_dir, &meta_dir) {
        crate::layout_migration::initialize_new_layout(&meta_dir)?;
        if let Err(error) = crate::config::upgrade_config_file(&config) {
            tracing::warn!("Could not upgrade launcher config: {error}");
            crate::feedback::errors::push_message(
                tracing::Level::ERROR,
                format!("Could not upgrade config.toml: {error}"),
            );
        }
        return Ok(MigrationScreenOutcome::NotNeeded);
    }

    loop {
        let progress = Arc::new(Mutex::new(crate::layout_migration::MigrationProgress {
            phase: "Preparing migration".to_owned(),
            item: "Inventorying launcher data".to_owned(),
            current: 0,
            total: 1,
            item_current: None,
            item_total: None,
            backup_dir: None,
        }));
        let task_progress = progress.clone();
        let task_instances = instances_dir.clone();
        let task_meta = meta_dir.clone();
        let task_config = config.clone();
        let task = tokio::task::spawn_blocking(move || {
            crate::layout_migration::run(&task_instances, &task_meta, &task_config, |update| {
                if let Ok(mut current) = task_progress.lock() {
                    *current = update;
                }
                request_redraw();
            })
        });

        while !task.is_finished() {
            let current = progress.lock().ok().map(|state| state.clone());
            terminal.draw(|frame| {
                app.render_migration_frame(frame);
                if let Some(current) = &current {
                    let item_fraction = match (current.item_current, current.item_total) {
                        (Some(value), Some(total)) if total > 0 => {
                            value.min(total) as f64 / total as f64
                        }
                        _ => 0.0,
                    };
                    let ratio = if current.total == 0 {
                        0.0
                    } else {
                        ((current.current as f64 + item_fraction) / current.total as f64)
                            .clamp(0.0, 1.0)
                    };
                    render_migration_progress_popup(
                        frame,
                        ratio,
                        current.phase.clone(),
                        current.item.clone(),
                    );
                }
            })?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        match task.await? {
            Ok(backup) => {
                tracing::info!("Layout migration complete; backup at {}", backup.display());
                if crate::layout_migration::cache_rebuild_pending(&meta_dir) {
                    if let Err(error) =
                        rebuild_runtime_cache_screen(terminal, app, &instances_dir, &meta_dir).await
                    {
                        if migration_retry_requested(terminal, app, &error).await? {
                            continue;
                        }
                        return Ok(MigrationScreenOutcome::Quit);
                    }
                    crate::layout_migration::finish_cache_rebuild(&meta_dir)?;
                }
                migration_completion_confirmation(terminal, app, &backup).await?;
                return Ok(MigrationScreenOutcome::Migrated);
            }
            Err(error) => {
                if !migration_retry_requested(terminal, app, &error.to_string()).await? {
                    return Ok(MigrationScreenOutcome::Quit);
                }
            }
        }
    }
}

async fn rebuild_runtime_cache_screen(
    terminal: &mut Tui,
    app: &mut app::App,
    instances_dir: &std::path::Path,
    meta_dir: &std::path::Path,
) -> Result<(), String> {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    let manager = crate::instance::InstanceManager::new(instances_dir, meta_dir);
    let instances = manager.load_all();
    let instance_total = instances.len() as u64;
    let instance_progress = Arc::new(Mutex::new((0_u64, String::new())));
    let task_progress = instance_progress.clone();
    let task = tokio::spawn(async move {
        for (index, instance) in instances.iter().enumerate() {
            if let Ok(mut state) = task_progress.lock() {
                state.0 = index as u64;
                state.1 = instance.name.clone();
            }
            request_redraw();
            manager
                .repair_runtime_cache(instance)
                .await
                .map_err(|error| error.to_string())?;
        }
        if let Ok(mut state) = task_progress.lock() {
            state.0 = instance_total;
            state.1.clear();
        }
        Ok::<(), String>(())
    });

    while !task.is_finished() {
        let progress = crate::feedback::progress::PROGRESS
            .lock()
            .ok()
            .map(|progress| progress.clone())
            .unwrap_or_default();
        let (instance_current, instance_name) = instance_progress
            .lock()
            .ok()
            .map(|state| state.clone())
            .unwrap_or_default();
        terminal
            .draw(|frame| {
                app.render_migration_frame(frame);
                let action = progress
                    .current_action
                    .as_deref()
                    .unwrap_or("Checking cached runtime files");
                let detail = progress.sub_action.as_deref().unwrap_or("");
                let (current, total) = progress.progress.unwrap_or((0, 0));
                let instance_ratio = if instance_total == 0 {
                    1.0
                } else {
                    instance_current.min(instance_total) as f64 / instance_total as f64
                };
                let ratio = if total == 0 {
                    instance_ratio
                } else {
                    current.min(total) as f64 / total as f64
                };
                let action = if instance_name.is_empty() {
                    action.to_owned()
                } else {
                    format!("{action} — {instance_name}")
                };
                render_migration_progress_popup(frame, ratio, action, detail.to_owned());
            })
            .map_err(|error| error.to_string())?;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    task.await.map_err(|error| error.to_string())?
}

async fn migration_retry_requested(
    terminal: &mut Tui,
    app: &mut app::App,
    error: &str,
) -> color_eyre::Result<bool> {
    use crate::config::theme::{BORDER_STYLE, THEME};
    use crossterm::event::{Event, KeyCode};
    use ratatui::layout::{Constraint, Direction, Layout};
    use ratatui::style::Style;
    use ratatui::widgets::{Block, Paragraph, Wrap};
    use std::time::Duration;

    loop {
        terminal.draw(|frame| {
            app.render_migration_frame(frame);
            let theme = THEME.as_ref();
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Fill(1),
                    Constraint::Length(9),
                    Constraint::Fill(1),
                ])
                .split(frame.area());
            let columns = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(12),
                    Constraint::Percentage(76),
                    Constraint::Percentage(12),
                ])
                .split(rows[1]);
            frame.render_widget(
                Paragraph::new(format!(
                    "Migration stopped safely:\n\n{error}\n\n[r] retry    [q] quit"
                ))
                .block(
                    Block::bordered()
                        .title(" Migration needs attention ")
                        .border_type(BORDER_STYLE.to_border_type())
                        .border_style(Style::default().fg(theme.error()))
                        .style(Style::default().fg(theme.text()).bg(theme.surface())),
                )
                .wrap(Wrap { trim: false }),
                columns[1],
            );
        })?;
        if crossterm::event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = crossterm::event::read()?
        {
            match key.code {
                KeyCode::Char('r') => return Ok(true),
                KeyCode::Char('q') | KeyCode::Esc => return Ok(false),
                _ => {}
            }
        }
    }
}

async fn migration_completion_confirmation(
    terminal: &mut Tui,
    app: &mut app::App,
    backup: &std::path::Path,
) -> color_eyre::Result<()> {
    use crate::config::theme::THEME;
    use crate::tui::widgets::popups::{base::PopupFrame, keybind_line};
    use crossterm::event::{Event, KeyCode};
    use ratatui::style::{Modifier, Style};
    use ratatui::text::Line;
    use ratatui::widgets::{Paragraph, Widget, Wrap};
    use std::time::Duration;

    loop {
        terminal.draw(|frame| {
            app.render_migration_frame(frame);
            let theme = THEME.as_ref();
            let backup = backup.display().to_string();
            frame.render_widget(
                PopupFrame {
                    title: Line::from(" Migration finished ").style(
                        Style::default()
                            .fg(theme.success())
                            .add_modifier(Modifier::BOLD),
                    ),
                    border_color: theme.success(),
                    bg: Some(theme.surface()),
                    keybinds: Some(keybind_line(&[("Enter", " continue")])),
                    search_line: None,
                    content: Box::new(move |area, buffer| {
                        Paragraph::new(format!("Backup:\n{backup}"))
                            .style(Style::default().fg(theme.text()))
                            .wrap(Wrap { trim: false })
                            .render(area, buffer);
                    }),
                },
                migration_popup_area(frame.area(), 6),
            );
        })?;
        if crossterm::event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = crossterm::event::read()?
            && key.code == KeyCode::Enter
        {
            return Ok(());
        }
    }
}

fn render_migration_progress_popup(
    frame: &mut ratatui::Frame,
    ratio: f64,
    action: String,
    detail: String,
) {
    use crate::config::theme::THEME;
    use crate::tui::widgets::popups::base::PopupFrame;
    use ratatui::layout::{Constraint, Layout};
    use ratatui::style::{Modifier, Style};
    use ratatui::text::Line;
    use ratatui::widgets::{Gauge, Paragraph, Widget};

    let theme = THEME.as_ref();
    frame.render_widget(
        PopupFrame {
            title: Line::from(" Migration ").style(
                Style::default()
                    .fg(theme.text_dim())
                    .add_modifier(Modifier::BOLD),
            ),
            border_color: theme.accent(),
            bg: Some(theme.surface()),
            keybinds: None,
            search_line: None,
            content: Box::new(move |area, buffer| {
                let rows = Layout::vertical([
                    Constraint::Length(1),
                    Constraint::Length(1),
                    Constraint::Length(1),
                ])
                .split(area);
                Gauge::default()
                    .gauge_style(
                        Style::default()
                            .fg(theme.success())
                            .bg(theme.surface())
                            .add_modifier(Modifier::BOLD),
                    )
                    .percent((ratio.clamp(0.0, 1.0) * 100.0) as u16)
                    .render(rows[0], buffer);
                Paragraph::new(action.as_str())
                    .style(Style::default().fg(theme.text()))
                    .render(rows[1], buffer);
                Paragraph::new(detail.as_str())
                    .style(Style::default().fg(theme.text_dim()))
                    .render(rows[2], buffer);
            }),
        },
        migration_popup_area(frame.area(), 5),
    );
}

fn migration_popup_area(area: ratatui::layout::Rect, height: u16) -> ratatui::layout::Rect {
    use ratatui::layout::Constraint;

    area.centered(
        Constraint::Length(area.width.saturating_sub(4).min(72)),
        Constraint::Length(area.height.saturating_sub(2).min(height)),
    )
}
