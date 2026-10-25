// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

mod files;

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};
use ratatui_textarea::TextArea;
use tokio::{sync::oneshot, task::JoinHandle};

use super::base::{PopupFrame, summary_heading, summary_row};
use super::settings_controls::{bundled_label, handle_text_area_input, settings_text_area};
use crate::config::theme::{BORDER_STYLE, THEME};
use crate::instance::export::{
    self, ExportOptions, ExportPlan, ExportReferences, ExportScan, PackFormat,
};
use crate::instance::{InstanceConfig, InstanceManager};
use crate::tui::widgets::status_badge;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Form,
    Files,
    Confirm,
    Overwrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Format,
    Name,
    Version,
    Author,
    Output,
    Include,
}

impl Field {
    const ALL: [Self; 6] = [
        Self::Format,
        Self::Name,
        Self::Version,
        Self::Author,
        Self::Output,
        Self::Include,
    ];

    fn supported(self, format: PackFormat) -> bool {
        match self {
            Self::Version => format != PackFormat::Mmc,
            Self::Author => format == PackFormat::CurseForge,
            _ => true,
        }
    }

    fn text_index(self) -> Option<usize> {
        match self {
            Self::Name => Some(0),
            Self::Version => Some(1),
            Self::Author => Some(2),
            Self::Output => Some(3),
            _ => None,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Format => "Format",
            Self::Name => "Pack name",
            Self::Version => "Pack version",
            Self::Author => "Author",
            Self::Output => "Output file",
            Self::Include => "Included files",
        }
    }
}

struct Resolved {
    references: ExportReferences,
    paths: BTreeSet<PathBuf>,
}

struct Resolving {
    receiver: oneshot::Receiver<Result<ExportReferences, String>>,
    task: JoinHandle<()>,
}

pub struct State {
    instance: InstanceConfig,
    format: PackFormat,
    fields: [String; 4],
    selected: Field,
    editing: Option<TextArea<'static>>,
    phase: Phase,
    scan: Option<Arc<ExportScan>>,
    selected_files: BTreeSet<PathBuf>,
    browser: files::Browser,
    resolved: HashMap<PackFormat, Result<Resolved, String>>,
    resolving: HashMap<PackFormat, Resolving>,
    bundle_unmatched: bool,
    plan: Option<ExportPlan>,
    pending: Option<oneshot::Receiver<Result<Arc<ExportScan>, String>>>,
    task: Option<JoinHandle<()>>,
}

impl Drop for State {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        for pending in self.resolving.values() {
            pending.task.abort();
        }
    }
}

impl State {
    pub fn new(instance: &InstanceConfig) -> Self {
        let directory = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_default();
        let output = directory.join(format!("{}-1.0.0.zip", instance.name));
        Self {
            instance: instance.clone(),
            format: PackFormat::CurseForge,
            fields: [
                instance.name.clone(),
                "1.0.0".to_owned(),
                std::env::var(if cfg!(windows) { "USERNAME" } else { "USER" }).unwrap_or_default(),
                output.display().to_string(),
            ],
            selected: Field::Format,
            editing: None,
            phase: Phase::Form,
            scan: None,
            selected_files: BTreeSet::new(),
            browser: files::Browser::default(),
            resolved: HashMap::new(),
            resolving: HashMap::new(),
            bundle_unmatched: false,
            plan: None,
            pending: None,
            task: None,
        }
    }

    pub fn start_scan(&mut self, manager: &InstanceManager) {
        let manager = InstanceManager::new(manager.instances_dir.clone(), manager.meta_dir.clone());
        let name = self.instance.name.clone();
        let (sender, receiver) = oneshot::channel();
        self.pending = Some(receiver);
        self.task = Some(tokio::spawn(async move {
            let result = export::scan_instance_export(&manager, &name)
                .await
                .map(Arc::new)
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
            crate::feedback::request_redraw();
        }));
    }

    fn identify(&mut self) {
        if self.resolved.contains_key(&self.format) || self.resolving.contains_key(&self.format) {
            return;
        }
        let Some(scan) = self.scan.clone() else {
            return;
        };
        if self.format == PackFormat::Mmc {
            self.resolved.insert(
                self.format,
                Ok(Resolved {
                    references: ExportReferences::Bundled,
                    paths: BTreeSet::new(),
                }),
            );
            return;
        }
        let format = self.format;
        let (sender, receiver) = oneshot::channel();
        let task = tokio::spawn(async move {
            let result = export::resolve_export_files(&scan, format)
                .await
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
            crate::feedback::request_redraw();
        });
        self.resolving.insert(format, Resolving { receiver, task });
    }

    pub fn text_input_active(&self) -> bool {
        self.editing.is_some()
    }

    pub fn drain(&mut self) {
        let result = match self.pending.as_mut().map(oneshot::Receiver::try_recv) {
            Some(Ok(result)) => Some(result),
            Some(Err(oneshot::error::TryRecvError::Closed)) => {
                Some(Err("Export task stopped unexpectedly".to_owned()))
            }
            _ => None,
        };
        if let Some(result) = result {
            self.pending = None;
            self.task = None;
            match result {
                Ok(scan) => {
                    self.selected_files = scan.defaults.clone();
                    self.scan = Some(scan);
                    self.identify();
                }
                Err(error) => {
                    crate::feedback::errors::push_message(tracing::Level::ERROR, error);
                }
            }
        }
        for format in PackFormat::ALL {
            let result = match self
                .resolving
                .get_mut(&format)
                .map(|pending| pending.receiver.try_recv())
            {
                Some(Ok(result)) => Some(result),
                Some(Err(oneshot::error::TryRecvError::Closed)) => Some(Err(
                    "Content identification task stopped unexpectedly".to_owned(),
                )),
                _ => None,
            };
            if let Some(result) = result {
                self.resolving.remove(&format);
                let resolved = result.and_then(|references| {
                    let scan = self.scan.as_ref().ok_or("Export scan is unavailable")?;
                    let mut options = ExportOptions::for_instance(&self.instance);
                    options.format = format;
                    let all = scan.paths().cloned().collect();
                    let plan = scan
                        .plan(&options, &all, &references)
                        .map_err(|error| error.to_string())?;
                    Ok(Resolved {
                        references,
                        paths: plan.referenced_paths().clone(),
                    })
                });
                if let Err(error) = &resolved {
                    crate::feedback::errors::push_message(
                        tracing::Level::ERROR,
                        format!("{}: {error}", format.label()),
                    );
                }
                self.resolved.insert(format, resolved);
            }
        }
    }

    fn referenced_paths(&self) -> Option<&BTreeSet<PathBuf>> {
        self.resolved
            .get(&self.format)
            .and_then(|result| result.as_ref().ok())
            .map(|resolved| &resolved.paths)
    }

    fn unmatched_files(&self, selected: &BTreeSet<PathBuf>) -> BTreeSet<PathBuf> {
        let empty = BTreeSet::new();
        self.scan.as_ref().map_or_else(BTreeSet::new, |scan| {
            if scan.needs_references(self.format, selected) && self.referenced_paths().is_none() {
                return BTreeSet::new();
            }
            scan.unmatched(
                self.format,
                selected,
                self.referenced_paths().unwrap_or(&empty),
            )
            .into_iter()
            .collect()
        })
    }

    fn options(&self) -> ExportOptions {
        let mut options = ExportOptions::for_instance(&self.instance);
        options.format = self.format;
        options.name = self.fields[0].clone();
        options.version = self.fields[1].clone();
        options.author = self.fields[2].clone();
        options.bundle_unmatched = self.bundle_unmatched;
        options
    }

    fn output(&self) -> PathBuf {
        crate::config::settings::resolve_path(&self.fields[3])
    }

    fn confirmation_lines(&self) -> Vec<Line<'static>> {
        let Some(plan) = &self.plan else {
            return Vec::new();
        };
        let theme = THEME.as_ref();
        let mut format = vec![
            Span::raw(self.format.label()),
            Span::raw(" "),
            status_badge(format!(".{}", self.format.extension()), theme.text_dim()),
        ];
        if self.format == PackFormat::Mmc {
            format.extend([Span::raw(" "), bundled_label()]);
        }
        let mut rows = vec![
            summary_heading("Pack"),
            summary_row("Format", Line::from(format)),
            summary_row("Pack name", plan.options.name.clone()),
        ];
        if Field::Version.supported(self.format) {
            rows.push(summary_row("Pack version", plan.options.version.clone()));
        }
        rows.extend([
            Line::default(),
            summary_heading("Runtime"),
            summary_row("Minecraft", plan.options.game_version.clone()),
            summary_row(
                "Loader",
                format!(
                    "{} {}",
                    plan.options.loader,
                    plan.options.loader_version.as_deref().unwrap_or("")
                )
                .trim_end()
                .to_owned(),
            ),
            Line::default(),
            summary_heading("Files"),
            summary_row("Output file", self.output().display().to_string()),
            summary_row("Included files", format!("{} selected", plan.file_count())),
        ]);
        let copies = plan.file_count() - plan.referenced_count();
        let mut contents = copies.to_string();
        if !plan.unmatched.is_empty() {
            contents.push_str(&format!(" ({} unmatched)", plan.unmatched.len()));
        }
        if self.format != PackFormat::Mmc {
            rows.push(summary_row(
                "Download links",
                plan.referenced_count().to_string(),
            ));
        }
        rows.push(summary_row("File copies", contents));
        rows
    }

    fn confirm(&mut self, manager: &InstanceManager) {
        let result = (|| -> export::ExportResult<ExportPlan> {
            let scan = self.scan.as_ref().ok_or("Files are still being scanned")?;
            let root =
                crate::storage::InstancePaths::new(manager.instances_dir.join(&self.instance.name))
                    .minecraft();
            export::validate_output_path(&root, &self.output())?;
            let empty = ExportReferences::empty(self.format);
            let references = match self.resolved.get(&self.format) {
                Some(Ok(resolved)) => &resolved.references,
                _ if !scan.needs_references(self.format, &self.selected_files) => &empty,
                Some(Err(error)) => return Err(error.clone().into()),
                _ => return Err("Content is still being identified; wait before continuing".into()),
            };
            let plan = scan.plan(&self.options(), &self.selected_files, references)?;
            if !plan.unmatched.is_empty() && !self.bundle_unmatched {
                return Err("Approve copying unmatched files with c, or deselect them".into());
            }
            Ok(plan)
        })();
        match result {
            Ok(plan) => {
                self.plan = Some(plan);
                self.phase = Phase::Confirm;
            }
            Err(error) => {
                crate::feedback::errors::push_message(tracing::Level::ERROR, error.to_string())
            }
        }
    }

    fn write(&mut self, overwrite: bool) -> Option<JoinHandle<()>> {
        let plan = self.plan.take()?;
        let output = self.output();
        let progress = crate::feedback::progress::ProgressTask::start("Exporting modpack");
        Some(tokio::spawn(async move {
            let _progress = progress;
            if let Err(error) = export::write_export_with_overwrite(plan, &output, overwrite).await
            {
                crate::feedback::errors::push_message(tracing::Level::ERROR, error.to_string());
            }
            crate::feedback::request_redraw();
        }))
    }

    fn rotate_format(&mut self, forward: bool) {
        let index = PackFormat::ALL
            .iter()
            .position(|format| *format == self.format)
            .unwrap();
        self.format =
            PackFormat::ALL[(index + if forward { 1 } else { 2 }) % PackFormat::ALL.len()];
        self.fields[3] = PathBuf::from(&self.fields[3])
            .with_extension(self.format.extension())
            .display()
            .to_string();
        self.bundle_unmatched = false;
        self.identify();
    }

    fn begin_edit(&mut self, manager: &InstanceManager) {
        if !self.selected.supported(self.format) {
            return;
        }
        match self.selected {
            Field::Format => self.rotate_format(true),
            Field::Include => {
                if self.scan.is_none() {
                    if self.pending.is_none() {
                        self.start_scan(manager);
                    }
                } else {
                    self.phase = Phase::Files;
                }
            }
            field => {
                self.editing = field
                    .text_index()
                    .map(|index| settings_text_area(vec![self.fields[index].clone()]))
            }
        }
    }

    fn commit_edit(&mut self) {
        let Some(editor) = self.editing.as_ref() else {
            return;
        };
        let value = editor.lines().join("");
        if let Some(index) = self.selected.text_index() {
            if self.selected != Field::Author && value.trim().is_empty() {
                crate::feedback::errors::push_message(
                    tracing::Level::ERROR,
                    format!("{} must not be empty", self.selected.label()),
                );
                return;
            }
            self.fields[index] = value;
        }
        self.editing = None;
    }

    fn move_selection(&mut self, forward: bool) {
        let fields = Field::ALL
            .into_iter()
            .filter(|field| field.supported(self.format))
            .collect::<Vec<_>>();
        let index = fields
            .iter()
            .position(|field| *field == self.selected)
            .unwrap_or(0);
        self.selected = fields[(index + if forward { 1 } else { fields.len() - 1 }) % fields.len()];
    }

    pub fn handle_key(&mut self, key: &KeyEvent, manager: &InstanceManager) -> bool {
        self.drain();
        match self.phase {
            Phase::Overwrite => {
                match key.code {
                    KeyCode::Esc => self.phase = Phase::Confirm,
                    KeyCode::Enter => return self.write(true).is_some(),
                    _ => {}
                }
                return false;
            }
            Phase::Confirm => {
                match key.code {
                    KeyCode::Esc => {
                        self.plan = None;
                        self.phase = Phase::Files;
                    }
                    KeyCode::Enter => match self.output().try_exists() {
                        Ok(true) => self.phase = Phase::Overwrite,
                        Ok(false) => return self.write(false).is_some(),
                        Err(error) => crate::feedback::errors::push_message(
                            tracing::Level::ERROR,
                            error.to_string(),
                        ),
                    },
                    _ => {}
                }
                return false;
            }
            Phase::Files => {
                match key.code {
                    KeyCode::Esc => {
                        self.phase = Phase::Form;
                    }
                    KeyCode::Char('h') | KeyCode::Left => {
                        if self.browser.directory.pop() {
                            self.browser.selected = 0;
                        }
                    }
                    KeyCode::Char('l') | KeyCode::Right => self.enter_directory(),
                    KeyCode::Char('j') | KeyCode::Down => {
                        self.browser.selected = (self.browser.selected + 1)
                            .min(self.file_entries().len().saturating_sub(1))
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        self.browser.selected = self.browser.selected.saturating_sub(1)
                    }
                    KeyCode::Char(' ') => self.toggle_file(),
                    KeyCode::Char('c')
                        if key.modifiers.is_empty()
                            && !self.unmatched_files(&self.selected_files).is_empty() =>
                    {
                        self.bundle_unmatched = !self.bundle_unmatched;
                    }
                    KeyCode::Enter => self.confirm(manager),
                    _ => {}
                }
                return false;
            }
            Phase::Form => {}
        }
        if let Some(editor) = self.editing.as_mut() {
            match key.code {
                KeyCode::Enter => self.commit_edit(),
                KeyCode::Esc => {
                    self.editing = None;
                }
                _ => handle_text_area_input(editor, key),
            }
            return false;
        }
        match key.code {
            KeyCode::Esc => return true,
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => self.move_selection(true),
            KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => self.move_selection(false),
            KeyCode::Left | KeyCode::Char('h') if self.selected == Field::Format => {
                self.rotate_format(false)
            }
            KeyCode::Right | KeyCode::Char('l') if self.selected == Field::Format => {
                self.rotate_format(true)
            }
            KeyCode::Enter | KeyCode::Right => self.begin_edit(manager),
            _ => {}
        }
        false
    }
}

fn keybinds(state: &State) -> Vec<(&'static str, &'static str)> {
    if state.editing.is_some() {
        return vec![("Enter", " apply"), ("Esc", " cancel")];
    }
    match state.phase {
        Phase::Form if state.selected == Field::Format => {
            vec![("h/l", " adjust"), ("Enter", " next"), ("Esc", " close")]
        }
        Phase::Form
            if state.selected == Field::Include
                && state.scan.is_none()
                && state.pending.is_some() =>
        {
            vec![("j/k", ""), ("Esc", " close")]
        }
        Phase::Form => vec![
            ("j/k", ""),
            (
                "Enter",
                if state.selected == Field::Include {
                    " open files"
                } else {
                    " edit"
                },
            ),
            ("Esc", " close"),
        ],
        Phase::Files => {
            let mut keys = vec![("Space", " toggle"), ("Enter", " continue")];
            if !state.unmatched_files(&state.selected_files).is_empty() {
                keys.push(("c", " copy"));
            }
            keys.extend([("Esc", " back"), ("h/l", " folders")]);
            keys
        }
        Phase::Confirm => vec![("Enter", " export"), ("Esc", " cancel")],
        Phase::Overwrite => vec![("Esc", " back"), ("Enter", " overwrite")],
    }
}

pub fn render(frame: &mut Frame, state: &mut State) {
    if state.phase == Phase::Overwrite {
        let theme = THEME.as_ref();
        let message = Paragraph::new(vec![
            Line::styled(
                "This will overwrite the existing file",
                Style::default().fg(theme.warning()),
            ),
            Line::from(state.output().display().to_string()),
        ])
        .style(Style::default().fg(theme.text()))
        .wrap(Wrap { trim: true });
        let width = 64.min(frame.area().width.saturating_sub(4));
        let height = (message.line_count(width.saturating_sub(2)) as u16 + 2)
            .min(frame.area().height.saturating_sub(4));
        let area = frame
            .area()
            .centered(Constraint::Length(width), Constraint::Length(height));
        frame.render_widget(
            PopupFrame {
                title: crate::tui::widgets::styled_title("Overwrite file?", false),
                border_color: theme.text_dim(),
                bg: Some(theme.surface()),
                keybinds: Some(super::keybind_line(&keybinds(state))),
                search_line: None,
                content: Box::new(|area, buffer| message.clone().render(area, buffer)),
            },
            area,
        );
        return;
    }
    if state.phase == Phase::Confirm {
        let theme = THEME.as_ref();
        let lines = state.confirmation_lines();
        let summary = Paragraph::new(lines).wrap(Wrap { trim: false });
        let width = frame.area().width * 68 / 100;
        let height = (summary.line_count(width.saturating_sub(2)) as u16 + 2)
            .min(frame.area().height.saturating_sub(4));
        let area = frame
            .area()
            .centered(Constraint::Length(width), Constraint::Length(height));
        frame.render_widget(
            PopupFrame {
                title: crate::tui::widgets::styled_title("Export Modpack", false),
                border_color: theme.text_dim(),
                bg: Some(theme.surface()),
                keybinds: Some(super::keybind_line(&[
                    ("Esc", " back"),
                    ("Enter", " export"),
                ])),
                search_line: None,
                content: Box::new(|area, buffer| summary.clone().render(area, buffer)),
            },
            area,
        );
        return;
    }
    let theme = THEME.as_ref();
    let height = match state.phase {
        Phase::Form => 11,
        Phase::Files => (frame.area().height * 2 / 3).clamp(12, 24),
        _ => unreachable!(),
    };
    let area = frame.area().centered(
        Constraint::Percentage(if state.phase == Phase::Files { 78 } else { 68 }),
        Constraint::Length(height.min(frame.area().height.saturating_sub(4))),
    );
    let title = match state.phase {
        Phase::Files => "Included Files",
        _ => "Export Modpack",
    };
    frame.render_widget(
        PopupFrame {
            title: Line::from(format!(" {title} · {} ", state.instance.name)),
            border_color: theme.text_dim(),
            bg: Some(theme.surface()),
            keybinds: Some(super::keybind_line_fitted(
                crate::config::settings::ShortcutHintScope::Popups,
                &keybinds(state),
                area.width.saturating_sub(2),
            )),
            search_line: None,
            content: Box::new(|area, buffer| match state.phase {
                Phase::Form => render_form(state, area, buffer),
                Phase::Files => render_files(state, area, buffer),
                _ => unreachable!(),
            }),
        },
        area,
    );
}

fn render_form(state: &State, area: Rect, buffer: &mut Buffer) {
    let theme = THEME.as_ref();
    let form = area;
    let mut rows = Vec::new();
    for field in Field::ALL {
        if matches!(field, Field::Format | Field::Output) {
            if field == Field::Output {
                rows.push((None, Line::default()));
            }
            rows.push((
                None,
                Line::from(Span::styled(
                    if field == Field::Format {
                        "  Pack"
                    } else {
                        "  Files"
                    },
                    Style::default()
                        .fg(theme.text())
                        .add_modifier(Modifier::BOLD),
                )),
            ));
        }
        let selected = state.selected == field;
        let supported = field.supported(state.format);
        let scanning = field == Field::Include && state.scan.is_none() && state.pending.is_some();
        let value = if selected && state.editing.is_some() {
            String::new()
        } else {
            match field {
                _ if !supported => "—".to_owned(),
                Field::Format => state.format.label().to_owned(),
                Field::Include if state.scan.is_none() => if state.pending.is_some() {
                    "Scanning…"
                } else {
                    "Not scanned"
                }
                .to_owned(),
                Field::Include => format!("{} selected ›", state.selected_files.len()),
                Field::Author if state.fields[2].is_empty() => "not set".to_owned(),
                _ => field
                    .text_index()
                    .map_or_else(String::new, |index| state.fields[index].clone()),
            }
        };
        let mut spans = vec![
            Span::styled(
                if selected { "▌ " } else { "  " },
                Style::default().fg(theme.accent()),
            ),
            Span::styled(
                format!("{:<18}", field.label()),
                Style::default().fg(theme.text_dim()),
            ),
            Span::styled(
                value,
                Style::default()
                    .fg(if !supported || scanning {
                        theme.text_dim()
                    } else if selected {
                        theme.accent()
                    } else {
                        theme.text()
                    })
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ),
        ];
        if field == Field::Format {
            spans.extend([
                Span::raw(" "),
                status_badge(format!(".{}", state.format.extension()), theme.text_dim()),
            ]);
            if state.format == PackFormat::Mmc {
                spans.extend([Span::raw(" "), bundled_label()]);
            }
        }
        rows.push((Some(field), Line::from(spans)));
    }
    let selected = rows
        .iter()
        .position(|(field, _)| *field == Some(state.selected))
        .unwrap_or(0);
    let scroll = (selected + 1).saturating_sub(form.height as usize);
    for (row, (field, line)) in rows
        .into_iter()
        .skip(scroll)
        .take(form.height as usize)
        .enumerate()
    {
        let row_area = Rect {
            y: form.y + row as u16,
            height: 1,
            ..form
        };
        Paragraph::new(line)
            .style(Style::default().bg(if field == Some(state.selected) {
                theme.stripe()
            } else {
                theme.surface()
            }))
            .render(row_area, buffer);
        if field == Some(state.selected)
            && let Some(editor) = &state.editing
        {
            editor.render(
                Rect {
                    x: form.x.saturating_add(20).min(form.right()),
                    width: form.width.saturating_sub(20),
                    ..row_area
                },
                buffer,
            );
        }
    }
}

fn render_files(state: &State, area: Rect, buffer: &mut Buffer) {
    let theme = THEME.as_ref();
    let [heading, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    let path = if state.browser.directory.as_os_str().is_empty() {
        "minecraft/".to_owned()
    } else {
        format!("minecraft/{}/", state.browser.directory.display())
    };
    Paragraph::new(path)
        .style(
            Style::default()
                .fg(theme.text())
                .add_modifier(Modifier::BOLD),
        )
        .render(heading, buffer);
    let compact = body.width < 76;
    let summary = file_summary(state, compact);
    let (list, panel) = if compact {
        let height = summary
            .iter()
            .map(|line| super::word_wrap_size(&line.to_string(), body.width as usize).1 as u16)
            .sum::<u16>();
        let [panel, list] = Layout::vertical([
            Constraint::Length(height.min(body.height.saturating_sub(1))),
            Constraint::Min(1),
        ])
        .areas(body);
        (list, panel)
    } else {
        let [list, panel] =
            Layout::horizontal([Constraint::Min(1), Constraint::Length(32)]).areas(body);
        let block = Block::default()
            .borders(Borders::LEFT)
            .border_type(BORDER_STYLE.to_border_type())
            .border_style(Style::default().fg(theme.border()));
        let panel_inner = block.inner(panel);
        block.render(panel, buffer);
        (
            Rect {
                width: list.width.saturating_sub(1),
                ..list
            },
            Rect {
                width: panel_inner.width.saturating_sub(2),
                ..panel_inner
            },
        )
    };
    files::render(state, list, buffer);
    Paragraph::new(summary)
        .wrap(Wrap { trim: false })
        .render(panel, buffer);
}

fn file_summary(state: &State, compact: bool) -> Vec<Line<'static>> {
    let theme = THEME.as_ref();
    let downloads = state
        .referenced_paths()
        .map_or(0, |paths| state.selected_files.intersection(paths).count());
    let needs_references = state
        .scan
        .as_ref()
        .is_some_and(|scan| scan.needs_references(state.format, &state.selected_files));
    let identified = !needs_references || state.referenced_paths().is_some();
    let status = if state.scan.is_none() && state.pending.is_some() {
        Some(("Scanning files…".to_owned(), theme.text_dim()))
    } else if needs_references && !state.resolved.contains_key(&state.format) {
        Some((
            format!("Identifying {} downloads…", state.format.label()),
            theme.text_dim(),
        ))
    } else {
        None
    };
    let included = state.selected_files.len();
    let excluded = state.scan.as_ref().map_or(0, |scan| scan.skipped.len());
    let unmatched = state.unmatched_files(&state.selected_files);
    let copies = included - downloads - unmatched.len();
    let count = |value: usize| {
        if identified {
            value.to_string()
        } else {
            "…".to_owned()
        }
    };
    let mut lines = if compact {
        vec![Line::styled(
            if state.format == PackFormat::Mmc {
                format!("{included} selected · {included} file copies")
            } else {
                format!(
                    "{included} selected · {} links · {} copies · {} unmatched",
                    count(downloads),
                    count(copies),
                    count(unmatched.len())
                )
            },
            Style::default().fg(theme.text_dim()),
        )]
    } else {
        let mut lines = vec![Line::styled(
            "  Selection",
            Style::default()
                .fg(theme.text())
                .add_modifier(Modifier::BOLD),
        )];
        let mut counts = vec![("Selected", included.to_string())];
        if state.format == PackFormat::Mmc {
            counts.push(("File copies", included.to_string()));
        } else {
            counts.extend([
                ("Download links", count(downloads)),
                ("Other copies", count(copies)),
                ("Unmatched", count(unmatched.len())),
            ]);
        }
        counts.push(("Excluded", excluded.to_string()));
        for (label, value) in counts {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {label:<16}"),
                    Style::default().fg(theme.text_dim()),
                ),
                Span::styled(value, Style::default().fg(theme.text())),
            ]));
        }
        lines.push(Line::default());
        lines
    };
    let indent = if compact { "" } else { "  " };
    if !unmatched.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(
                format!(
                    "{indent}[{}] ",
                    if state.bundle_unmatched { "✓" } else { " " }
                ),
                Style::default().fg(if state.bundle_unmatched {
                    theme.success()
                } else {
                    theme.warning()
                }),
            ),
            Span::styled(
                format!("Copy {} unmatched", unmatched.len()),
                Style::default().fg(theme.text()),
            ),
        ]));
    }
    if identified && (copies > 0 || !unmatched.is_empty()) {
        lines.push(Line::styled(
            format!("{indent}Check licences"),
            Style::default().fg(theme.text_dim()),
        ));
    }
    if compact && excluded > 0 {
        lines.push(Line::styled(
            format!("{excluded} excluded"),
            Style::default().fg(theme.text_dim()),
        ));
    }
    if let Some((message, color)) = status {
        lines.insert(0, Line::styled(message, Style::default().fg(color)));
    }
    lines
}

#[cfg(test)]
#[path = "../../tests/widgets/popups/export_modpack.rs"]
mod tests;
