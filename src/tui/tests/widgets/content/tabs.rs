// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn mode_labels_have_the_same_rendered_width() {
    assert_eq!(
        mode_label(ContentMode::Installed).chars().count(),
        mode_label(ContentMode::Discover).chars().count()
    );
}

#[test]
fn discovery_navigation_includes_datapacks() {
    assert_eq!(
        ContentTab::Shaders.next_for_mode(ContentMode::Discover),
        ContentTab::DataPacks
    );
    assert_eq!(
        ContentTab::DataPacks.next_for_mode(ContentMode::Discover),
        ContentTab::Mods
    );
    assert_eq!(
        ContentTab::Mods.previous_for_mode(ContentMode::Discover),
        ContentTab::DataPacks
    );
}

#[test]
fn discovery_navigation_recovers_from_hidden_local_tab() {
    assert_eq!(
        ContentTab::Logs.next_for_mode(ContentMode::Discover),
        ContentTab::ResourcePacks
    );
}

#[test]
fn discovery_version_rows_only_show_the_version_number() {
    let version = crate::net::modrinth::VersionInfo {
        id: "version-id".to_owned(),
        project_id: "project-id".to_owned(),
        name: "A descriptive release title".to_owned(),
        version_number: "3.2.4-fabric-26.1".to_owned(),
        game_versions: vec![],
        loaders: vec![],
        version_type: crate::net::modrinth::VersionType::Release,
        dependencies: Vec::new(),
        date_published: String::new(),
        files: vec![],
    };

    assert_eq!(discovery_version_label(&version), "3.2.4-fabric-26.1");
}

#[test]
fn discovery_confirmation_popup_fits_its_summary() {
    assert_eq!(
        version_popup_height(false, None, false, false, false),
        VERSION_POPUP_HEIGHT
    );
    assert_eq!(version_popup_height(true, None, false, false, false), 6);
    assert_eq!(version_popup_height(true, None, false, true, false), 7);
}

#[test]
fn install_summary_checks_selected_mod_release_not_picker_version() {
    use crate::instance::ContentKind;
    use crate::net::modrinth::{DiscoveryProject, VersionInfo, VersionType};
    use ratatui::{Terminal, backend::TestBackend};

    let mut state = DiscoveryState::new(ContentKind::Mod);
    state
        .list
        .entries
        .push(crate::tui::widgets::content::discovery::project_entry(
            DiscoveryProject {
                id: "project".to_owned(),
                slug: "project".to_owned(),
                title: "Project".to_owned(),
                description: String::new(),
                downloads: 0,
                icon_url: None,
                icon_bytes: None,
            },
            None,
        ));
    state.list.list_state.selected = Some(0);
    state.begin_versions();
    let popup = state.version_popup.as_mut().unwrap();
    popup.loading = false;
    popup.confirming = true;
    popup.versions = vec![VersionInfo {
        id: "version".to_owned(),
        project_id: "project".to_owned(),
        name: "1.0".to_owned(),
        version_number: "1.0".to_owned(),
        game_versions: vec!["26.1".to_owned(), "26.3".to_owned()],
        loaders: vec!["forge".to_owned()],
        version_type: VersionType::Release,
        dependencies: Vec::new(),
        date_published: String::new(),
        files: Vec::new(),
    }];
    popup.selected_minecraft_version = Some("26.1".to_owned());
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    let picker = ratatui_image::picker::Picker::halfblocks();
    terminal
        .draw(|frame| {
            render_version_popup(frame, frame.area(), &mut state, &picker, Some("26.3"));
        })
        .unwrap();
    assert!(!format!("{}", terminal.backend()).contains("may be incompatible"));

    state
        .version_popup
        .as_mut()
        .unwrap()
        .selected_minecraft_version = Some("26.3".to_owned());
    terminal
        .draw(|frame| {
            render_version_popup(frame, frame.area(), &mut state, &picker, Some("26.3"));
        })
        .unwrap();
    assert!(!format!("{}", terminal.backend()).contains("may be incompatible"));

    let popup = state.version_popup.as_mut().unwrap();
    popup.selected_minecraft_version = None;
    popup.versions[0].game_versions = vec!["26.1".to_owned()];
    terminal
        .draw(|frame| {
            render_version_popup(frame, frame.area(), &mut state, &picker, Some("26.3"));
        })
        .unwrap();
    assert!(
        format!("{}", terminal.backend())
            .contains("This mod version may be incompatible with 26.3")
    );
    assert!(!format!("{}", terminal.backend()).contains("Warning:"));
}

#[test]
fn skipped_dependencies_render_as_skipped_in_confirmation() {
    use crate::instance::ContentKind;
    use crate::instance::content::dependencies::{DependencyPlan, PlannedInstall};
    use crate::net::modrinth::{DiscoveryProject, VersionInfo, VersionType};
    use ratatui::{Terminal, backend::TestBackend};

    let project = DiscoveryProject {
        id: "project".to_owned(),
        slug: "project".to_owned(),
        title: "Project".to_owned(),
        description: String::new(),
        downloads: 0,
        icon_url: None,
        icon_bytes: None,
    };
    let mut state = DiscoveryState::new(ContentKind::Mod);
    state
        .list
        .entries
        .push(crate::tui::widgets::content::discovery::project_entry(
            project, None,
        ));
    state.list.list_state.selected = Some(0);
    state.begin_versions();
    let root_version = VersionInfo {
        id: "version".to_owned(),
        project_id: "project".to_owned(),
        name: "1.0".to_owned(),
        version_number: "1.0".to_owned(),
        game_versions: vec!["26.3".to_owned()],
        loaders: vec!["fabric".to_owned()],
        version_type: VersionType::Release,
        dependencies: Vec::new(),
        date_published: String::new(),
        files: Vec::new(),
    };
    let mut dep_version = root_version.clone();
    dep_version.project_id = "dependency".to_owned();
    dep_version.version_number = "2.0".to_owned();
    let planned = |title: &str, version: VersionInfo| PlannedInstall {
        provider: "modrinth".to_owned(),
        project_id: title.to_owned(),
        title: title.to_owned(),
        version,
        installed_path: None,
        kind: ContentKind::Mod,
        destination: std::path::PathBuf::from("mods"),
        provider_aliases: Vec::new(),
        required_dependencies: Vec::new(),
        automatic_dependency: false,
        cleanup_eligible: false,
        replacement: true,
    };
    let popup = state.version_popup.as_mut().unwrap();
    popup.loading = false;
    popup.versions = vec![root_version.clone()];
    popup.confirming = true;
    popup.dependency_plan = Some(DependencyPlan {
        items: vec![
            planned("Project", root_version),
            planned("Dependency", dep_version),
        ],
        root_count: 1,
        optional_dependencies: 0,
    });

    let picker = ratatui_image::picker::Picker::halfblocks();
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    terminal
        .draw(|frame| {
            render_version_popup(frame, frame.area(), &mut state, &picker, Some("26.3"));
        })
        .unwrap();
    let screen = format!("{}", terminal.backend());
    assert!(screen.contains("Also changes"));
    assert!(screen.contains("skip deps"));

    state.version_popup.as_mut().unwrap().skip_dependencies = true;
    terminal
        .draw(|frame| {
            render_version_popup(frame, frame.area(), &mut state, &picker, Some("26.3"));
        })
        .unwrap();
    let screen = format!("{}", terminal.backend());
    assert!(screen.contains("Skipped"));
    assert!(!screen.contains("Also changes"));
    assert!(screen.contains("include deps"));
}

#[test]
fn discovery_version_popup_renders_over_a_project_page() {
    use crate::instance::ContentKind;
    use crate::net::modrinth::DiscoveryProject;
    use ratatui::{Terminal, backend::TestBackend};

    let project = DiscoveryProject {
        id: "project".to_owned(),
        slug: "project".to_owned(),
        title: "Project".to_owned(),
        description: String::new(),
        downloads: 0,
        icon_url: None,
        icon_bytes: None,
    };
    let mut state = DiscoveryState::new(ContentKind::Mod);
    state
        .list
        .entries
        .push(crate::tui::widgets::content::discovery::project_entry(
            project, None,
        ));
    state.list.list_state.selected = Some(0);
    state.begin_project_page();
    state.begin_versions();

    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    let picker = ratatui_image::picker::Picker::halfblocks();
    terminal
        .draw(|frame| render_discovery_popup(frame, frame.area(), &mut state, &picker))
        .unwrap();

    assert!(format!("{}", terminal.backend()).contains("Install Project"));
}

#[test]
fn discovery_sort_panel_renders_beside_results() {
    use crate::instance::ContentKind;
    use ratatui::{Terminal, backend::TestBackend};

    let mut state = DiscoveryState::new(ContentKind::Mod);
    state.sort_panel_open = true;
    state.sort_panel_page = crate::tui::widgets::content::discovery::DiscoveryPanelPage::Sort;
    let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
    let picker = ratatui_image::picker::Picker::halfblocks();
    terminal
        .draw(|frame| {
            render_discovery_popup(frame, Rect::new(0, 1, 80, 19), &mut state, &picker);
        })
        .unwrap();

    let rendered = format!("{}", terminal.backend());
    assert!(rendered.contains("Sort"));
    assert!(rendered.contains("Sort by"));
    assert!(!rendered.contains("Active"));
    assert!(rendered.contains("▲ Best match"));
    assert!(rendered.contains("Downloads"));
    state.sort_reversed = true;
    terminal
        .draw(|frame| {
            render_discovery_popup(frame, Rect::new(0, 1, 80, 19), &mut state, &picker);
        })
        .unwrap();
    assert!(format!("{}", terminal.backend()).contains("▼ Best match"));
}

#[test]
fn installed_sort_panel_only_shows_file_fields() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut state = DiscoveryState::new(crate::instance::ContentKind::Mod);
    state.set_local_mode(true);
    state.sort_panel_open = true;
    state.sort_panel_page = crate::tui::widgets::content::discovery::DiscoveryPanelPage::Sort;
    state.local_sort_index = 6;
    state.local_sort_descending = true;
    let mut terminal = Terminal::new(TestBackend::new(100, 25)).unwrap();
    let picker = ratatui_image::picker::Picker::halfblocks();
    terminal
        .draw(|frame| render_discovery_popup(frame, Rect::new(0, 1, 100, 24), &mut state, &picker))
        .unwrap();
    let rendered = format!("{}", terminal.backend());
    let positions = ["Name", "File size", "Date modified"].map(|label| {
        rendered
            .find(label)
            .unwrap_or_else(|| panic!("missing {label}: {rendered}"))
    });
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(rendered.contains("Sort by"));
    assert!(!rendered.contains("Best match"));
    assert!(!rendered.contains("Project ranking"));
    assert!(rendered.contains("▼ File size"));
    assert!(!rendered.contains("Largest"));
    assert!(!rendered.contains("Oldest"));

    state.sort_panel_selected = 1;
    state.local_sort_descending = false;
    terminal
        .draw(|frame| render_discovery_popup(frame, Rect::new(0, 1, 100, 24), &mut state, &picker))
        .unwrap();
    let rendered = format!("{}", terminal.backend());
    assert!(rendered.contains("▲ File size"));
    assert!(!rendered.contains("Smallest"));
    assert!(!rendered.contains("Downloads"));
    assert!(rendered.contains("Sort by"));
}

#[test]
fn curseforge_panel_uses_curseforge_categories_and_sorts() {
    use ratatui::{Terminal, backend::TestBackend};
    crate::net::curseforge::seed_discovery_categories_for_test();
    let mut state = DiscoveryState::new(crate::instance::ContentKind::Mod);
    state.set_local_mode(false);
    state.category_provider = "curseforge".to_owned();
    state.sort = crate::instance::content::provider::DiscoverySort::Popular;
    state.sort_panel_page = crate::tui::widgets::content::discovery::DiscoveryPanelPage::Filters;
    let mut terminal = Terminal::new(TestBackend::new(60, 30)).unwrap();
    terminal
        .draw(|frame| render_sort_panel(frame, Rect::new(0, 1, 60, 29), &mut state))
        .unwrap();
    let rendered = format!("{}", terminal.backend());
    assert!(rendered.contains("API and Library"));
    assert!(!rendered.contains("Environment"));
    state.sort_panel_page = crate::tui::widgets::content::discovery::DiscoveryPanelPage::Sort;
    terminal
        .draw(|frame| render_sort_panel(frame, Rect::new(0, 1, 60, 29), &mut state))
        .unwrap();
    let rendered = format!("{}", terminal.backend());
    assert!(rendered.contains("Release date"));
    assert!(rendered.contains("Best match"));
}

#[test]
fn discovery_filter_panel_renders_compatibility_and_categories() {
    use crate::instance::ContentKind;
    use ratatui::{Terminal, backend::TestBackend};

    let mut state = DiscoveryState::new(ContentKind::Shader);
    state.sort_panel_open = true;
    state.sort_panel_page = crate::tui::widgets::content::discovery::DiscoveryPanelPage::Filters;
    state.filters.categories.insert(
        "cartoon".to_owned(),
        crate::tui::widgets::content::discovery::CategoryFilter::Include,
    );
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let picker = ratatui_image::picker::Picker::halfblocks();
    terminal
        .draw(|frame| {
            render_discovery_popup(frame, Rect::new(0, 1, 100, 29), &mut state, &picker);
        })
        .unwrap();

    let rendered = format!("{}", terminal.backend());
    assert!(rendered.contains("MC version"));
    assert!(rendered.contains("Categories"));
    assert!(rendered.contains("Vanilla-like"));
    assert!(rendered.contains("+ Cartoon"));
    assert!(!rendered.contains("Include"));
}

#[test]
fn discovery_filter_panel_renders_version_picker_inline() {
    use crate::instance::ContentKind;
    use ratatui::{Terminal, backend::TestBackend};

    let mut state = DiscoveryState::new(ContentKind::Mod);
    state.sort_panel_open = true;
    state.filter_version_picker_open = true;
    state.filters.game_version =
        crate::tui::widgets::content::discovery::GameVersionFilter::Specific(
            std::collections::BTreeMap::from([
                (
                    "1.21.1".to_owned(),
                    crate::tui::widgets::content::discovery::CategoryFilter::Include,
                ),
                (
                    "1.20.1".to_owned(),
                    crate::tui::widgets::content::discovery::CategoryFilter::Exclude,
                ),
            ]),
        );
    *state.filter_game_versions.lock().unwrap() =
        crate::tui::widgets::popups::LoadState::Loaded(vec![
            crate::instance::loader::GameVersion {
                id: "1.21.1".to_owned(),
                stable: true,
            },
            crate::instance::loader::GameVersion {
                id: "1.20.1".to_owned(),
                stable: true,
            },
        ]);
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let picker = ratatui_image::picker::Picker::halfblocks();
    terminal
        .draw(|frame| {
            render_discovery_popup(frame, Rect::new(0, 1, 100, 29), &mut state, &picker);
        })
        .unwrap();

    let rendered = format!("{}", terminal.backend());
    assert!(rendered.contains("Minecraft version"));
    assert!(rendered.contains("Current"));
    assert!(rendered.contains("Any"));
    assert!(!rendered.contains("Scope"));
    assert!(rendered.contains("+ 1.21.1"));
    assert!(rendered.contains("− 1.20.1"));
    let lines: Vec<_> = rendered.lines().collect();
    let any = lines
        .iter()
        .position(|line| line.contains("· Any"))
        .unwrap();
    let first_version = lines
        .iter()
        .position(|line| line.contains("+ 1.21.1"))
        .unwrap();
    assert_eq!(first_version, any + 2);

    state.filters.game_version =
        crate::tui::widgets::content::discovery::GameVersionFilter::Current;
    terminal
        .draw(|frame| render_discovery_popup(frame, Rect::new(0, 1, 100, 29), &mut state, &picker))
        .unwrap();
    assert!(format!("{}", terminal.backend()).contains("● Current"));
    state.filters.game_version = crate::tui::widgets::content::discovery::GameVersionFilter::Any;
    terminal
        .draw(|frame| render_discovery_popup(frame, Rect::new(0, 1, 100, 29), &mut state, &picker))
        .unwrap();
    assert!(format!("{}", terminal.backend()).contains("● Any"));
}

#[test]
fn modpack_filter_panel_uses_popup_surface() {
    use ratatui::{Terminal, backend::TestBackend};

    let mut state = DiscoveryState::new_modpacks();
    state.sort_panel_open = true;
    state.sort_panel_focused = true;
    state.filter_version_picker_open = true;
    *state.filter_game_versions.lock().unwrap() =
        crate::tui::widgets::popups::LoadState::Loaded(vec![
            crate::instance::loader::GameVersion {
                id: "1.21.1".to_owned(),
                stable: true,
            },
        ]);
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let picker = ratatui_image::picker::Picker::halfblocks();
    terminal
        .draw(|frame| {
            render_discovery_popup(frame, Rect::new(0, 1, 100, 29), &mut state, &picker);
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let theme = THEME.as_ref();
    assert_eq!(buffer.cell((80, 2)).unwrap().bg, theme.surface());
    assert_eq!(buffer.cell((80, 3)).unwrap().bg, theme.stripe());
    assert_eq!(buffer.cell((80, 15)).unwrap().bg, theme.surface());

    state.filter_version_picker_open = false;
    terminal
        .draw(|frame| {
            render_discovery_popup(frame, Rect::new(0, 1, 100, 29), &mut state, &picker);
        })
        .unwrap();
    assert_eq!(
        terminal.backend().buffer().cell((80, 2)).unwrap().bg,
        theme.stripe()
    );
    assert_eq!(
        terminal.backend().buffer().cell((80, 15)).unwrap().bg,
        theme.surface()
    );

    state.sort_panel_page = crate::tui::widgets::content::discovery::DiscoveryPanelPage::Sort;
    terminal
        .draw(|frame| {
            render_discovery_popup(frame, Rect::new(0, 1, 100, 29), &mut state, &picker);
        })
        .unwrap();
    assert_eq!(
        terminal.backend().buffer().cell((80, 15)).unwrap().bg,
        theme.surface()
    );
}

#[test]
fn confirmation_metadata_is_human_readable() {
    assert_eq!(
        confirmation_loaders(&["fabric".to_owned(), "neoforge".to_owned()]),
        "Fabric, NeoForge"
    );
    assert_eq!(
        confirmation_release_date("2026-07-26T14:30:00Z"),
        "2026-07-26"
    );
    assert_eq!(confirmation_values(&[]), "Unknown");
}

#[test]
fn empty_world_picker_only_offers_exit_actions() {
    let keybinds = world_picker_keybinds(false);
    assert!(!keybinds.iter().any(|(key, _)| *key == "Enter"));
    assert!(!keybinds.iter().any(|(key, _)| *key == "j/k"));
}
