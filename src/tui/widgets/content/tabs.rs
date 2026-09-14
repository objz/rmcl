// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// the outer frame for the content area: tab bar, keybind footer,
// and dispatching render calls to the active tab's widget.
// also renders the instance name/version header with run state indicators.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, ListItem, Paragraph, Widget, Wrap},
};
use throbber_widgets_tui::{Throbber, ThrobberState};

use crate::config::theme::{BORDER_STYLE, THEME};
use crate::tui::app::FocusedArea;
use crate::tui::widgets::content::{ContentMode, DiscoveryState};

use crate::tui::widgets::styled_title;

type ContentScanner =
    fn(&std::path::Path, &str, bool) -> crate::instance::content::entry::ContentEntry;

const VERSION_POPUP_HEIGHT: u16 = 18;

#[derive(Clone, Copy)]
struct DownloadableTab {
    directory: &'static str,
    extension: &'static str,
    scanner: ContentScanner,
    loading_text: &'static str,
    empty_text: &'static str,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ContentTab {
    #[default]
    Mods,
    ResourcePacks,
    Shaders,
    DataPacks,
    Screenshots,
    Worlds,
    Logs,
}

impl ContentTab {
    const ALL: &'static [ContentTab] = &[
        ContentTab::Mods,
        ContentTab::ResourcePacks,
        ContentTab::Shaders,
        ContentTab::Screenshots,
        ContentTab::Worlds,
        ContentTab::Logs,
    ];

    const DISCOVERY: &'static [ContentTab] = &[
        ContentTab::Mods,
        ContentTab::ResourcePacks,
        ContentTab::Shaders,
        ContentTab::DataPacks,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ContentTab::Mods => "Mods",
            ContentTab::ResourcePacks => "Resource Packs",
            ContentTab::Shaders => "Shaders",
            ContentTab::DataPacks => "Datapacks",
            ContentTab::Screenshots => "Screenshots",
            ContentTab::Worlds => "Worlds",
            ContentTab::Logs => "Logs",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&t| t == self).unwrap_or(0)
    }

    pub fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    pub fn previous(self) -> Self {
        let idx = self.index();
        Self::ALL[if idx == 0 {
            Self::ALL.len() - 1
        } else {
            idx - 1
        }]
    }

    pub fn next_for_mode(self, mode: ContentMode) -> Self {
        cycle_tab(self, visible_tabs(mode), true)
    }

    pub fn previous_for_mode(self, mode: ContentMode) -> Self {
        cycle_tab(self, visible_tabs(mode), false)
    }

    fn downloadable(self) -> Option<DownloadableTab> {
        match self {
            Self::Mods => Some(DownloadableTab {
                directory: "mods",
                extension: ".jar",
                scanner: crate::instance::scan_one_mod,
                loading_text: "Loading mods...",
                empty_text: "No mods installed.",
            }),
            Self::ResourcePacks => Some(DownloadableTab {
                directory: "resourcepacks",
                extension: ".zip",
                scanner: crate::instance::scan_one_resource_pack,
                loading_text: "Loading resource packs...",
                empty_text: "No resource packs installed.",
            }),
            Self::Shaders => Some(DownloadableTab {
                directory: "shaderpacks",
                extension: ".zip",
                scanner: crate::instance::scan_one_shader,
                loading_text: "Loading shaders...",
                empty_text: "No shaders installed.",
            }),
            _ => None,
        }
    }
}

fn visible_tabs(mode: ContentMode) -> &'static [ContentTab] {
    match mode {
        ContentMode::Installed => ContentTab::ALL,
        ContentMode::Discover => ContentTab::DISCOVERY,
    }
}

fn mode_label(mode: ContentMode) -> String {
    format!(" {} ", mode.label())
}

fn cycle_tab(current: ContentTab, tabs: &[ContentTab], forward: bool) -> ContentTab {
    let index = tabs.iter().position(|tab| *tab == current).unwrap_or(0);
    if forward {
        tabs[(index + 1) % tabs.len()]
    } else {
        tabs[if index == 0 {
            tabs.len() - 1
        } else {
            index - 1
        }]
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render(
    frame: &mut Frame,
    area: Rect,
    focused: FocusedArea,
    tab: ContentTab,
    mode: ContentMode,
    instance: Option<&crate::instance::InstanceConfig>,
    has_modpack_update: bool,
    mods_state: &mut super::list::ContentListState,
    mods_discovery_state: &mut DiscoveryState,
    resource_packs_state: &mut super::list::ContentListState,
    resource_packs_discovery_state: &mut DiscoveryState,
    shaders_state: &mut super::list::ContentListState,
    shaders_discovery_state: &mut DiscoveryState,
    datapacks_discovery_state: &mut DiscoveryState,
    worlds_state: &mut super::list::ContentListState,
    world_datapacks_state: &mut super::list::ContentListState,
    open_world_datapacks: Option<&(String, std::path::PathBuf)>,
    screenshots_state: &mut crate::tui::widgets::screenshots_grid::ScreenshotsState,
    logs_state: &mut crate::tui::widgets::logs_viewer::LogsState,
    instances_dir: &std::path::Path,
    picker: &ratatui_image::picker::Picker,
    world_quick_play_supported: bool,
) {
    let theme = THEME.as_ref();
    let is_focused = focused == FocusedArea::Content;

    let border_color = if is_focused {
        theme.accent()
    } else {
        theme.border()
    };

    let tabs = visible_tabs(mode);
    let tab_titles: Vec<Span> = tabs
        .iter()
        .enumerate()
        .flat_map(|(i, t)| {
            let mut spans = Vec::new();
            if i > 0 {
                spans.push(Span::styled(
                    "\u{2022}",
                    Style::default().fg(theme.text_dim()),
                ));
            }
            if tabs.get(i) == Some(&tab) {
                let style = Style::default()
                    .fg(theme.accent())
                    .add_modifier(Modifier::BOLD);
                spans.push(Span::styled(format!(" {} ", t.label()), style));
            } else {
                spans.push(Span::styled(
                    format!(" {} ", t.label()),
                    Style::default().fg(theme.text()),
                ));
            }
            spans
        })
        .collect();

    let search_line = match tab {
        ContentTab::Mods if mode == ContentMode::Discover => {
            mods_discovery_state.search.title_line()
        }
        ContentTab::Mods => mods_state.search.title_line(),
        ContentTab::ResourcePacks if mode == ContentMode::Discover => {
            resource_packs_discovery_state.search.title_line()
        }
        ContentTab::ResourcePacks => resource_packs_state.search.title_line(),
        ContentTab::Shaders if mode == ContentMode::Discover => {
            shaders_discovery_state.search.title_line()
        }
        ContentTab::Shaders => shaders_state.search.title_line(),
        ContentTab::DataPacks => datapacks_discovery_state.search.title_line(),
        ContentTab::Worlds if open_world_datapacks.is_some() => {
            world_datapacks_state.search.title_line()
        }
        ContentTab::Worlds => worlds_state.search.title_line(),
        ContentTab::Screenshots => screenshots_state.search.title_line(),
        ContentTab::Logs => {
            if logs_state.viewer_focused {
                logs_state.viewer_search.title_line()
            } else {
                logs_state.search.title_line()
            }
        }
    };

    let mode_background = match mode {
        ContentMode::Installed => theme.success(),
        ContentMode::Discover => theme.info(),
    };
    let mut content_titles = vec![
        Span::styled(
            mode_label(mode),
            crate::tui::widgets::status_badge_style(mode_background),
        ),
        Span::raw(" "),
    ];
    content_titles.extend(tab_titles);

    let mut block = Block::default()
        .title_top(Line::from(content_titles))
        .borders(Borders::ALL)
        .border_type(BORDER_STYLE.to_border_type())
        .border_style(Style::default().fg(border_color));

    if let Some(sl) = search_line {
        block = block.title_top(sl);
    }

    let discovery_can_delete = match tab {
        ContentTab::Mods => mods_discovery_state.selected_is_installed(),
        ContentTab::ResourcePacks => resource_packs_discovery_state.selected_is_installed(),
        ContentTab::Shaders => shaders_discovery_state.selected_is_installed(),
        ContentTab::DataPacks => false,
        _ => false,
    };
    let discovery_page_open = match tab {
        ContentTab::Mods => mods_discovery_state.project_page_open(),
        ContentTab::ResourcePacks => resource_packs_discovery_state.project_page_open(),
        ContentTab::Shaders => shaders_discovery_state.project_page_open(),
        ContentTab::DataPacks => datapacks_discovery_state.project_page_open(),
        _ => false,
    };
    let discovery_unavailable = mode == ContentMode::Discover
        && instance.is_some_and(|instance| match tab {
            ContentTab::Mods => mods_discovery_state.unavailable_message(instance).is_some(),
            ContentTab::ResourcePacks => resource_packs_discovery_state
                .unavailable_message(instance)
                .is_some(),
            ContentTab::Shaders => shaders_discovery_state
                .unavailable_message(instance)
                .is_some(),
            ContentTab::DataPacks => datapacks_discovery_state
                .unavailable_message(instance)
                .is_some(),
            _ => false,
        });
    let has_updates = match tab {
        ContentTab::Mods => mods_state.entries.iter(),
        ContentTab::ResourcePacks => resource_packs_state.entries.iter(),
        ContentTab::Shaders => shaders_state.entries.iter(),
        ContentTab::Worlds if open_world_datapacks.is_some() => {
            world_datapacks_state.entries.iter()
        }
        _ => worlds_state.entries.iter(),
    }
    .any(|entry| entry.title_suffix.as_deref() == Some("Update"));
    let can_change_version = match tab {
        ContentTab::Mods => mods_state.selected_has_provider_project(),
        ContentTab::ResourcePacks => resource_packs_state.selected_has_provider_project(),
        ContentTab::Shaders => shaders_state.selected_has_provider_project(),
        ContentTab::Worlds if open_world_datapacks.is_some() => {
            world_datapacks_state.selected_has_provider_project()
        }
        _ => false,
    };

    // keybinds change depending on which tab is active and whether
    // the content panel or instances panel has focus
    let kb: Option<&[(&str, &str)]> = if is_focused {
        Some(match (mode, tab) {
            (ContentMode::Discover, _) if discovery_unavailable => {
                &[("h/l", " tabs"), ("Tab", " installed")]
            }
            (ContentMode::Discover, _) if discovery_page_open => &[
                ("j/k", " scroll"),
                ("g/G", " top/bottom"),
                ("v", " versions"),
                ("h", " back"),
            ],
            (ContentMode::Discover, _) if discovery_can_delete => &[
                ("j/k", " navigate"),
                (" [/] ", " pages"),
                ("Enter", " view"),
                ("v", " versions"),
                ("d", " delete"),
                ("h/l", " tabs"),
                ("/", " search"),
                ("Tab", " installed"),
            ],
            (ContentMode::Discover, _) => &[
                ("j/k", " navigate"),
                (" [/] ", " pages"),
                ("Enter", " view"),
                ("v", " versions"),
                ("h/l", " tabs"),
                ("/", " search"),
                ("Tab", " installed"),
            ],
            (ContentMode::Installed, ContentTab::Mods)
            | (ContentMode::Installed, ContentTab::ResourcePacks)
            | (ContentMode::Installed, ContentTab::Shaders) => &[
                ("j/k", " navigate"),
                ("⏎", " toggle"),
                ("v", " versions"),
                ("u", " update all"),
                ("d", " delete"),
                ("Shift+⏎", " open dir"),
                ("h/l", " tabs"),
                ("/", " search"),
                ("Tab", " discovery"),
            ],
            (ContentMode::Installed, ContentTab::Worlds) if open_world_datapacks.is_some() => &[
                ("j/k", " navigate"),
                ("v", " versions"),
                ("u", " update all"),
                ("d", " delete"),
                ("Shift+Enter", " open dir"),
                ("h/Esc", " back"),
                ("/", " search"),
            ],
            (ContentMode::Installed, ContentTab::Worlds) if world_quick_play_supported => &[
                ("j/k", " navigate"),
                ("Enter", " datapacks"),
                ("q", " quick launch"),
                ("d", " delete"),
                ("Shift+⏎", " open dir"),
                ("h/l", " tabs"),
                ("/", " search"),
                ("Tab", " discovery"),
            ],
            (ContentMode::Installed, ContentTab::Worlds) => &[
                ("j/k", " navigate"),
                ("Enter", " datapacks"),
                ("d", " delete"),
                ("Shift+⏎", " open dir"),
                ("h/l", " tabs"),
                ("/", " search"),
                ("Tab", " discovery"),
            ],
            (ContentMode::Installed, ContentTab::DataPacks) => &[],
            (ContentMode::Installed, ContentTab::Screenshots) => &[
                ("Shift+HJKL", " grid"),
                ("⏎", " open"),
                ("d", " delete"),
                ("Shift+⏎", " open dir"),
                ("h/l", " tabs"),
                ("/", " search"),
                ("Tab", " discovery"),
            ],
            (ContentMode::Installed, ContentTab::Logs) => {
                if logs_state.viewer_focused {
                    &[
                        ("j/k", " scroll"),
                        ("g/G", " top/bottom"),
                        ("d", " delete"),
                        ("Esc", " back"),
                        ("/", " search"),
                        ("Tab", " discovery"),
                    ]
                } else {
                    &[
                        ("j/k", " navigate"),
                        ("⏎", " view"),
                        ("d", " delete"),
                        ("h/l", " tabs"),
                        ("/", " search"),
                        ("Tab", " discovery"),
                    ]
                }
            }
        })
    } else if focused == FocusedArea::Instances {
        Some(&[
            ("l", " launch"),
            ("⏎", " content"),
            ("v", " versions"),
            ("Shift+⏎", " open dir"),
            ("Esc", " kill"),
            ("a", " add"),
            ("m", " modpacks"),
            ("u", " update"),
            ("d", " delete"),
            ("r", " rename"),
            ("/", " search"),
        ])
    } else {
        None
    };

    let mut keybinds = kb.map_or_else(Vec::new, <[_]>::to_vec);
    if is_focused && mode == ContentMode::Installed && !has_updates {
        keybinds.retain(|(key, _)| *key != "u");
    }
    if is_focused && mode == ContentMode::Installed && !can_change_version {
        keybinds.retain(|(key, _)| *key != "v");
    }
    if focused == FocusedArea::Instances
        && instance.is_none_or(|instance| instance.modpack_source.is_none())
    {
        keybinds.retain(|(key, _)| *key != "v");
    }
    if focused == FocusedArea::Instances && !has_modpack_update {
        keybinds.retain(|(key, _)| *key != "u");
    }
    if is_focused && !keybinds.iter().any(|(key, _)| key.contains("Esc")) {
        keybinds.push(("Esc", " back"));
    }
    // The main panel footer stays on its border; lower-priority hints are omitted when narrow.
    block = block.title_bottom(crate::tui::widgets::popups::keybind_line_fitted(
        if focused == FocusedArea::Instances {
            crate::config::settings::ShortcutHintScope::Instances
        } else {
            crate::config::settings::ShortcutHintScope::Content
        },
        &keybinds,
        area.width.saturating_sub(2),
    ));

    let content_area = block.inner(area);
    frame.render_widget(block, area);

    let downloadable_state = match tab {
        ContentTab::Mods => Some((mods_state, mods_discovery_state)),
        ContentTab::ResourcePacks => Some((resource_packs_state, resource_packs_discovery_state)),
        ContentTab::Shaders => Some((shaders_state, shaders_discovery_state)),
        _ => None,
    };
    if let Some((state, discovery_state)) = downloadable_state {
        render_downloadable(
            frame,
            content_area,
            instance,
            state,
            discovery_state,
            tab.downloadable().expect("downloadable tab config"),
            mode,
            is_focused,
            instances_dir,
            picker,
        );
        return;
    }

    if tab == ContentTab::DataPacks {
        if let Some(instance) = instance {
            if worlds_state.loaded_for.as_deref() != Some(instance.name.as_str()) {
                let saves = instances_dir
                    .join(&instance.name)
                    .join(crate::storage::MINECRAFT_DIR_NAME)
                    .join("saves");
                worlds_state.start_load(
                    &saves,
                    &instance.name,
                    crate::instance::scan_one_world,
                    "",
                );
                worlds_state.watch_dir(saves);
            }
            let loading_text = format!(
                "Searching {}...",
                crate::config::SETTINGS
                    .read()
                    .content
                    .discovery_provider_label()
            );
            render_discovery(
                frame,
                content_area,
                datapacks_discovery_state,
                instance,
                is_focused,
                &loading_text,
                picker,
            );
        } else {
            frame.render_widget(
                Paragraph::new("No instance selected.")
                    .style(Style::default().fg(theme.text_dim())),
                content_area,
            );
        }
        return;
    }

    // lazy-load: only scan when switching to an instance that hasn't been loaded yet
    match tab {
        ContentTab::Mods
        | ContentTab::ResourcePacks
        | ContentTab::Shaders
        | ContentTab::DataPacks => unreachable!(),
        ContentTab::Logs => {
            if let Some(instance) = instance {
                if logs_state.loaded_for.as_deref() != Some(instance.name.as_str()) {
                    logs_state.start_load(instances_dir, &instance.name);
                }
                crate::tui::widgets::logs_viewer::render(
                    frame,
                    content_area,
                    logs_state,
                    is_focused,
                );
            } else {
                frame.render_widget(
                    Paragraph::new("No instance selected.")
                        .style(Style::default().fg(theme.text_dim())),
                    content_area,
                );
            }
        }
        ContentTab::Screenshots => {
            if let Some(instance) = instance {
                if screenshots_state.loaded_for.as_deref() != Some(instance.name.as_str()) {
                    screenshots_state.start_load(instances_dir, &instance.name);
                }
                crate::tui::widgets::screenshots_grid::render(
                    frame,
                    content_area,
                    screenshots_state,
                    is_focused,
                );
            } else {
                frame.render_widget(
                    Paragraph::new("No instance selected.")
                        .style(Style::default().fg(theme.text_dim())),
                    content_area,
                );
            }
        }
        ContentTab::Worlds => {
            if let Some(instance) = instance {
                if let Some((world_name, world_path)) = open_world_datapacks {
                    let cache_key = format!("{}:{world_name}", instance.name);
                    let content_dir = world_path.join("datapacks");
                    if world_datapacks_state.loaded_for.as_deref() != Some(cache_key.as_str()) {
                        world_datapacks_state.start_load(
                            &content_dir,
                            &cache_key,
                            crate::instance::scan_one_datapack,
                            ".zip",
                        );
                        world_datapacks_state.watch_dir(content_dir);
                    }
                    super::list::render(
                        frame,
                        content_area,
                        world_datapacks_state,
                        is_focused,
                        "Loading datapacks...",
                        "No datapacks installed.",
                        picker,
                        false,
                        false,
                    );
                    if datapacks_discovery_state.version_popup.is_some() {
                        render_version_popup(
                            frame,
                            content_area,
                            datapacks_discovery_state,
                            picker,
                        );
                    }
                    return;
                }
                if worlds_state.loaded_for.as_deref() != Some(instance.name.as_str()) {
                    let content_dir = instances_dir
                        .join(&instance.name)
                        .join(crate::storage::MINECRAFT_DIR_NAME)
                        .join("saves");
                    worlds_state.start_load(
                        &content_dir,
                        &instance.name,
                        crate::instance::scan_one_world,
                        "",
                    );
                    worlds_state.watch_dir(content_dir);
                }
                super::list::render(
                    frame,
                    content_area,
                    worlds_state,
                    is_focused,
                    "Loading worlds...",
                    "No worlds saved.",
                    picker,
                    false,
                    true,
                );
            } else {
                frame.render_widget(
                    Paragraph::new("No instance selected.")
                        .style(Style::default().fg(theme.text_dim())),
                    content_area,
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_downloadable(
    frame: &mut Frame,
    area: Rect,
    instance: Option<&crate::instance::InstanceConfig>,
    state: &mut super::list::ContentListState,
    discovery_state: &mut DiscoveryState,
    tab: DownloadableTab,
    mode: ContentMode,
    is_focused: bool,
    instances_dir: &std::path::Path,
    picker: &ratatui_image::picker::Picker,
) {
    let Some(instance) = instance else {
        frame.render_widget(
            Paragraph::new("No instance selected.")
                .style(Style::default().fg(THEME.as_ref().text_dim())),
            area,
        );
        return;
    };
    if state.loaded_for.as_deref() != Some(instance.name.as_str()) {
        let content_dir = instances_dir
            .join(&instance.name)
            .join(crate::storage::MINECRAFT_DIR_NAME)
            .join(tab.directory);
        state.start_load(&content_dir, &instance.name, tab.scanner, tab.extension);
        state.watch_dir(content_dir);
    }
    if mode == ContentMode::Discover {
        let loading_text = format!(
            "Searching {}...",
            crate::config::SETTINGS
                .read()
                .content
                .discovery_provider_label()
        );
        render_discovery(
            frame,
            area,
            discovery_state,
            instance,
            is_focused,
            &loading_text,
            picker,
        );
    } else {
        super::list::render(
            frame,
            area,
            state,
            is_focused,
            tab.loading_text,
            tab.empty_text,
            picker,
            false,
            false,
        );
        if discovery_state.version_popup.is_some() {
            render_version_popup(frame, area, discovery_state, picker);
        }
    }
}

fn render_discovery(
    frame: &mut Frame,
    area: Rect,
    state: &mut DiscoveryState,
    instance: &crate::instance::InstanceConfig,
    is_focused: bool,
    loading_text: &str,
    picker: &ratatui_image::picker::Picker,
) {
    state.set_viewport_rows(area.height);
    if let Some(message) = state.unavailable_message(instance) {
        frame.render_widget(
            Paragraph::new(message).style(Style::default().fg(THEME.as_ref().text_dim())),
            area,
        );
        return;
    }
    render_discovery_body(frame, area, state, is_focused, loading_text, picker);
}

pub(crate) fn render_discovery_popup(
    frame: &mut Frame,
    area: Rect,
    state: &mut DiscoveryState,
    picker: &ratatui_image::picker::Picker,
) {
    state.set_viewport_rows(area.height);
    render_discovery_body(frame, area, state, true, "Searching modpacks...", picker);
}

fn render_discovery_body(
    frame: &mut Frame,
    area: Rect,
    state: &mut DiscoveryState,
    is_focused: bool,
    loading_text: &str,
    picker: &ratatui_image::picker::Picker,
) {
    if let Some(page) = state.project_page.as_mut() {
        if let Some(error) = page.error.as_deref() {
            frame.render_widget(
                Paragraph::new(error)
                    .style(Style::default().fg(THEME.as_ref().error()))
                    .wrap(Wrap { trim: true }),
                area,
            );
        } else if let Some(document) = page.document.as_mut() {
            page.max_scroll = crate::tui::widgets::markdown::render(
                frame,
                area,
                document,
                &mut page.scroll,
                picker,
            );
        } else {
            frame.render_widget(
                Paragraph::new(format!("Loading {}...", page.title))
                    .style(Style::default().fg(THEME.as_ref().text_dim())),
                area,
            );
        }
    } else {
        let empty_text = state.empty_text().to_string();
        let paginate = !state.search.active && state.version_popup.is_none();
        super::list::render(
            frame,
            area,
            &mut state.list,
            is_focused,
            loading_text,
            &empty_text,
            picker,
            paginate,
            false,
        );
    }
    if state.version_popup.is_some() {
        render_version_popup(frame, area, state, picker);
    }
}

pub(crate) fn render_version_popup(
    frame: &mut Frame,
    area: Rect,
    state: &mut DiscoveryState,
    picker: &ratatui_image::picker::Picker,
) {
    let Some(popup) = state.version_popup.as_mut() else {
        return;
    };
    if popup.selecting_world {
        render_world_picker(frame, area, popup, picker);
        return;
    }
    let popup_area = area.centered(
        Constraint::Percentage(50),
        Constraint::Length(
            version_popup_height(
                popup.confirming,
                popup.dependency_plan.as_ref(),
                popup.target_world.is_some(),
            )
            .min(area.height.saturating_sub(2)),
        ),
    );
    let theme = THEME.as_ref();
    let title = popup.title();
    let loading = popup.loading;
    let resolving_dependencies = popup.resolving_dependencies;
    let installing = popup.installing;
    let confirming = popup.confirming;
    let error = popup.error.clone();
    let selected = popup.selected;
    let selecting_minecraft_version = popup.selecting_minecraft_version;
    let selected_version = popup
        .selected_version()
        .map(|version| version.version_number.clone())
        .unwrap_or_default();
    let minecraft_versions = popup
        .selected_version()
        .map(|version| confirmation_values(&version.game_versions))
        .unwrap_or_else(|| "Unknown".to_owned());
    let loaders = popup
        .selected_version()
        .map(|version| confirmation_loaders(&version.loaders))
        .unwrap_or_else(|| "Unknown".to_owned());
    let release_date = popup
        .selected_version()
        .map(|version| confirmation_release_date(&version.date_published))
        .unwrap_or_else(|| "Unknown".to_owned());
    let replacing = popup.installed_path.is_some();
    let reinstalling = popup.current_version_id.as_deref()
        == popup.selected_version().map(|version| version.id.as_str());
    let provider_label = popup.provider_label().to_owned();
    let target_world = popup.target_world.as_ref().map(|(name, _)| name.clone());
    let dependency_installs = popup
        .dependency_plan
        .as_ref()
        .map(|plan| {
            plan.dependency_installs()
                .map(|item| item.title.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let dependency_replacements = popup
        .dependency_plan
        .as_ref()
        .map(|plan| {
            plan.dependency_replacements()
                .map(|item| format!("{} -> {}", item.title, item.version.version_number))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let optional_dependencies = popup
        .dependency_plan
        .as_ref()
        .map(|plan| plan.optional_dependencies)
        .unwrap_or_default();
    let can_switch_provider = popup.sources.len() > 1;
    let current_version_id = popup.current_version_id.clone();
    let items = if selecting_minecraft_version {
        popup
            .minecraft_versions
            .iter()
            .map(|version| ListItem::new(version.clone()).style(Style::default().fg(theme.text())))
            .collect::<Vec<_>>()
    } else {
        popup
            .visible_versions()
            .enumerate()
            .map(|(index, version)| {
                let mut spans = vec![Span::styled(
                    discovery_version_label(version),
                    Style::default()
                        .fg(if index == selected {
                            theme.accent()
                        } else {
                            theme.text()
                        })
                        .add_modifier(if index == selected {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                )];
                if current_version_id.as_deref() == Some(version.id.as_str()) {
                    spans.extend([
                        Span::raw("  "),
                        Span::styled(
                            " Installed ",
                            Style::default()
                                .fg(theme.background())
                                .bg(theme.success())
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]);
                }
                ListItem::new(Line::from(spans))
            })
            .collect::<Vec<_>>()
    };
    let keybinds = if confirming {
        crate::tui::widgets::popups::keybind_line(&[
            ("h", " back"),
            (
                "Enter",
                if reinstalling {
                    " reinstall"
                } else if replacing {
                    " change"
                } else {
                    " install"
                },
            ),
        ])
    } else {
        let mut keybinds = vec![("j/k", " navigate")];
        if can_switch_provider {
            keybinds.push(("Tab", " source"));
        }
        if popup.selected_minecraft_version.is_some() {
            keybinds.push(("h", " back"));
        }
        keybinds.extend([("Enter", " continue"), ("Esc", " close")]);
        crate::tui::widgets::popups::keybind_line(&keybinds)
    };

    let popup_frame = crate::tui::widgets::popups::base::PopupFrame {
        title: styled_title(&title, false),
        border_color: theme.accent(),
        bg: Some(theme.surface()),
        keybinds: Some(keybinds),
        search_line: Some(
            Line::from(Span::styled(
                format!(" {provider_label} "),
                Style::default()
                    .fg(theme.accent())
                    .add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Right),
        ),
        content: Box::new(move |area, buffer| {
            if installing {
                Paragraph::new("Installing...")
                    .style(Style::default().fg(THEME.as_ref().text_dim()))
                    .render(area, buffer);
            } else if loading {
                Paragraph::new(if resolving_dependencies {
                    "Resolving required dependencies..."
                } else {
                    "Loading compatible versions..."
                })
                .style(Style::default().fg(THEME.as_ref().text_dim()))
                .render(area, buffer);
            } else if let Some(error) = &error {
                Paragraph::new(error.as_str())
                    .style(Style::default().fg(THEME.as_ref().error()))
                    .wrap(Wrap { trim: true })
                    .render(area, buffer);
            } else if confirming {
                let mut rows = vec![
                    ("Version", selected_version.as_str()),
                    ("Minecraft", minecraft_versions.as_str()),
                    ("Loader", loaders.as_str()),
                    ("Released", release_date.as_str()),
                ];
                if let Some(world) = target_world.as_deref() {
                    rows.push(("World", world));
                }
                if !dependency_installs.is_empty() {
                    rows.push(("Also installs", dependency_installs.as_str()));
                }
                if !dependency_replacements.is_empty() {
                    rows.push(("Also changes", dependency_replacements.as_str()));
                }
                let optional_text;
                if optional_dependencies > 0 {
                    optional_text = optional_dependencies.to_string();
                    rows.push(("Optional not installed", optional_text.as_str()));
                }
                crate::tui::widgets::popups::base::render_summary(&rows, area, buffer);
            } else if items.is_empty() {
                Paragraph::new(if selecting_minecraft_version {
                    "No compatible Minecraft versions found."
                } else {
                    "No compatible versions found."
                })
                .style(Style::default().fg(THEME.as_ref().text_dim()))
                .render(area, buffer);
            } else {
                crate::tui::widgets::popups::select_list::render(
                    items.clone(),
                    selected,
                    area,
                    buffer,
                );
            }
        }),
    };
    frame.render_widget(popup_frame, popup_area);
}

fn render_world_picker(
    frame: &mut Frame,
    area: Rect,
    popup: &mut super::discovery::VersionPopupState,
    picker: &ratatui_image::picker::Picker,
) {
    let popup_area = area.centered(
        Constraint::Percentage(50),
        Constraint::Length(VERSION_POPUP_HEIGHT.min(area.height.saturating_sub(2))),
    );
    let theme = THEME.as_ref();
    let title = popup.title();
    let provider_label = popup.provider_label().to_owned();
    let keybinds = world_picker_keybinds(!popup.worlds.entries.is_empty());
    let frame_widget = crate::tui::widgets::popups::base::PopupFrame {
        title: styled_title(&title, false),
        border_color: theme.accent(),
        bg: Some(theme.surface()),
        keybinds: Some(crate::tui::widgets::popups::keybind_line(keybinds)),
        search_line: Some(
            Line::from(Span::styled(
                format!(" {provider_label} "),
                Style::default()
                    .fg(theme.accent())
                    .add_modifier(Modifier::BOLD),
            ))
            .alignment(Alignment::Right),
        ),
        content: Box::new(|_, _| {}),
    };
    frame.render_widget(frame_widget, popup_area);
    super::list::render(
        frame,
        popup_area.inner(Margin::new(1, 1)),
        &mut popup.worlds,
        true,
        "Loading worlds...",
        "No worlds available.",
        picker,
        false,
        false,
    );
}

fn world_picker_keybinds(has_worlds: bool) -> &'static [(&'static str, &'static str)] {
    if has_worlds {
        &[
            ("j/k", " navigate"),
            ("Enter", " continue"),
            ("h", " back"),
            ("Esc", " close"),
        ]
    } else {
        &[("h", " back"), ("Esc", " close")]
    }
}

fn version_popup_height(
    confirming: bool,
    plan: Option<&crate::instance::content::dependencies::DependencyPlan>,
    has_world: bool,
) -> u16 {
    if !confirming {
        return VERSION_POPUP_HEIGHT;
    }
    let Some(plan) = plan else {
        return 6 + u16::from(has_world);
    };
    6 + u16::from(has_world)
        + u16::from(plan.dependency_installs().next().is_some())
        + u16::from(plan.dependency_replacements().next().is_some())
        + u16::from(plan.optional_dependencies > 0)
}

fn confirmation_values(values: &[String]) -> String {
    if values.is_empty() {
        "Unknown".to_owned()
    } else {
        values.join(", ")
    }
}

fn confirmation_loaders(loaders: &[String]) -> String {
    let loaders = loaders
        .iter()
        .map(|loader| match loader.as_str() {
            "fabric" => "Fabric",
            "forge" => "Forge",
            "neoforge" => "NeoForge",
            "quilt" => "Quilt",
            "minecraft" => "Minecraft",
            "datapack" => "Datapack",
            other => other,
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    confirmation_values(&loaders)
}

fn confirmation_release_date(value: &str) -> String {
    if value.is_empty() {
        return "Unknown".to_owned();
    }
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|date| date.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|_| value.to_owned())
}

fn discovery_version_label(version: &crate::net::modrinth::VersionInfo) -> String {
    version.version_number.clone()
}

// the header bar above the content tabs, showing instance name, loader info,
// and a spinner/error indicator when the instance is running or crashed
pub fn title(
    frame: &mut Frame,
    area: Rect,
    focused: FocusedArea,
    instance: Option<&crate::instance::InstanceConfig>,
    throbber_state: &mut ThrobberState,
) {
    let theme = THEME.as_ref();
    let color = if focused == FocusedArea::Content {
        theme.accent()
    } else {
        theme.border()
    };

    let block = Block::default()
        .title(styled_title("Content", true))
        .borders(Borders::ALL)
        .border_type(BORDER_STYLE.to_border_type())
        .border_style(Style::default().fg(color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    match instance {
        None => {
            frame.render_widget(
                Paragraph::new("No instance selected").style(Style::default().fg(theme.text_dim())),
                inner,
            );
        }
        Some(inst) => {
            let [left_area, right_area] =
                Layout::horizontal([Constraint::Min(0), Constraint::Length(32)]).areas(inner);

            use crate::instance::runtime::RunState;
            let run_state = crate::instance::runtime::get(&inst.name);

            match run_state {
                Some(RunState::Authenticating)
                | Some(RunState::Running)
                | Some(RunState::Starting) => {
                    let throbber = Throbber::default()
                        .label(inst.name.as_str())
                        .style(
                            Style::default()
                                .fg(theme.text())
                                .add_modifier(Modifier::BOLD),
                        )
                        .throbber_style(
                            Style::default()
                                .fg(theme.success())
                                .add_modifier(Modifier::BOLD),
                        )
                        .throbber_set(throbber_widgets_tui::BRAILLE_EIGHT_DOUBLE)
                        .use_type(throbber_widgets_tui::WhichUse::Spin);
                    frame.render_stateful_widget(throbber, left_area, throbber_state);
                }
                Some(RunState::Crashed(_)) => {
                    frame.render_widget(
                        Paragraph::new(Line::from(vec![
                            Span::styled(
                                "\u{2717} ",
                                Style::default()
                                    .fg(theme.error())
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                inst.name.as_str(),
                                Style::default()
                                    .fg(theme.text())
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ])),
                        left_area,
                    );
                }
                None => {
                    frame.render_widget(
                        Paragraph::new(Span::styled(
                            inst.name.as_str(),
                            Style::default()
                                .fg(theme.text())
                                .add_modifier(Modifier::BOLD),
                        )),
                        left_area,
                    );
                }
            }

            let loader_str = match &inst.loader_version {
                Some(lv) => format!("{} \u{00b7} {} {}", inst.game_version, inst.loader, lv),
                None => format!("{} \u{00b7} {}", inst.game_version, inst.loader),
            };
            frame.render_widget(
                Paragraph::new(loader_str)
                    .style(Style::default().fg(theme.text_dim()))
                    .alignment(Alignment::Right),
                right_area,
            );
        }
    }
}

#[cfg(test)]
#[path = "../../tests/widgets/content/tabs.rs"]
mod tests;
