// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use std::path::{Path, PathBuf};

use super::harness::UiHarness;
use crate::instance::content::entry::ContentEntry;
use crate::instance::{
    ContentFileRecord, ContentKind, ContentManifest, ProviderProject, Resolution,
};
use crate::net::modrinth::DiscoveryProject;
use crate::tui::{
    app::{FocusedArea, ProviderConflictState},
    widgets::{
        content::{ContentMode, ContentTab},
        popups::confirm,
    },
};

#[test]
fn remote_project_links_use_http_urls_and_encode_shell_metacharacters() {
    let url =
        crate::tui::input::project_link_url(r#"https://example.com/"&%USERNAME%?q=a b"#).unwrap();
    assert_eq!(url.as_str(), "https://example.com/%22&%USERNAME%?q=a%20b");
    for link in [
        "file:///C:/Windows/notepad.exe",
        "javascript:alert(1)",
        "shell:AppsFolder",
        r"C:\folder\file",
        "not a url",
    ] {
        assert!(crate::tui::input::project_link_url(link).is_err(), "{link}");
    }
}

#[test]
fn ctrl_arrows_navigate_adjacent_panels_without_changing_content_tabs() {
    let mut ui = UiHarness::new();
    let panels = [
        FocusedArea::Instances,
        FocusedArea::Content,
        FocusedArea::Account,
        FocusedArea::Settings,
        FocusedArea::Overview,
    ];
    let directions = [KeyCode::Up, KeyCode::Down, KeyCode::Left, KeyCode::Right];
    let destinations = [
        [panels[0], panels[0], panels[0], panels[1]],
        [panels[1], panels[2], panels[0], panels[1]],
        [panels[1], panels[2], panels[0], panels[3]],
        [panels[1], panels[3], panels[2], panels[4]],
        [panels[1], panels[4], panels[3], panels[4]],
    ];
    for (panel, expected) in panels.into_iter().zip(destinations) {
        for (direction, destination) in directions.into_iter().zip(expected) {
            for kind in [KeyEventKind::Press, KeyEventKind::Repeat] {
                ui.app.focused = panel;
                assert!(ui.key_event(KeyEvent::new_with_kind(
                    direction,
                    KeyModifiers::CONTROL,
                    kind,
                )));
                assert_eq!(ui.app.focused, destination, "{panel:?} {direction:?}");
                assert_eq!(ui.app.content_tab, ContentTab::Mods);
            }
        }
    }

    ui.app.focused = FocusedArea::Content;
    for mode in [ContentMode::Installed, ContentMode::Discover] {
        ui.app.content_mode = mode;
        ui.key(KeyCode::Right);
        assert_eq!(ui.app.content_tab, ContentTab::ResourcePacks);
        ui.key(KeyCode::Left);
        assert_eq!(ui.app.content_tab, ContentTab::Mods);
        assert_eq!(ui.app.focused, FocusedArea::Content);
    }
}

#[test]
fn ctrl_arrows_do_not_escape_modal_input() {
    let mut ui = UiHarness::new();
    for panel in [FocusedArea::ConfirmDelete, FocusedArea::OverviewExpanded] {
        ui.app.focused = panel;
        ui.key_with(KeyCode::Down, KeyModifiers::CONTROL);
        assert_eq!(ui.app.focused, panel);
    }

    ui.app.focused = FocusedArea::Instances;
    ui.key(KeyCode::Char('a'));
    ui.key_with(KeyCode::Down, KeyModifiers::CONTROL);
    assert_eq!(ui.app.focused, FocusedArea::Popup);
    ui.key(KeyCode::Esc);
    ui.key(KeyCode::Char('m'));
    ui.key_with(KeyCode::Down, KeyModifiers::CONTROL);
    assert_eq!(ui.app.focused, FocusedArea::ImportPopup);
    ui.key(KeyCode::Esc);

    ui.app.focused = FocusedArea::Account;
    ui.key(KeyCode::Char('a'));
    ui.key_with(KeyCode::Right, KeyModifiers::CONTROL);
    assert_eq!(ui.app.focused, FocusedArea::Account);

    ui.app.focused = FocusedArea::Settings;
    ui.key(KeyCode::Char('a'));
    ui.key_with(KeyCode::Right, KeyModifiers::CONTROL);
    assert_eq!(ui.app.focused, FocusedArea::Settings);

    ui.app.focused = FocusedArea::Instances;
    ui.app.instances_state.renaming = Some("Renaming".to_owned());
    ui.key_with(KeyCode::Right, KeyModifiers::CONTROL);
    assert_eq!(ui.app.focused, FocusedArea::Instances);
}

#[test]
fn global_navigation_returns_from_log_overlay() {
    let mut ui = UiHarness::new();

    ui.key(KeyCode::Char('C'));
    assert_eq!(ui.app.focused, FocusedArea::Content);

    ui.key(KeyCode::Char('O'));
    assert_eq!(ui.app.focused, FocusedArea::OverviewExpanded);

    ui.key(KeyCode::Esc);
    assert_eq!(ui.app.focused, FocusedArea::Content);

    ui.key(KeyCode::Char('q'));
    assert!(ui.app.exit);
}

#[test]
fn escape_returns_through_nested_sections() {
    let mut ui = UiHarness::new();

    ui.app.focused = FocusedArea::Content;
    ui.app.mods_state.search.activate();
    ui.key(KeyCode::Esc);
    assert_eq!(ui.app.focused, FocusedArea::Content);
    ui.key(KeyCode::Esc);
    assert_eq!(ui.app.focused, FocusedArea::Instances);

    ui.app.focused = FocusedArea::Account;
    ui.key(KeyCode::Char('a'));
    ui.key(KeyCode::Esc);
    assert_eq!(ui.app.focused, FocusedArea::Account);
    ui.key(KeyCode::Esc);
    assert_eq!(ui.app.focused, FocusedArea::Instances);

    ui.app.focused = FocusedArea::Settings;
    ui.key(KeyCode::Esc);
    assert_eq!(ui.app.focused, FocusedArea::Instances);

    ui.app.focused = FocusedArea::Content;
    ui.app.content_tab = ContentTab::Worlds;
    ui.app.open_world_datapacks = Some(("World".to_owned(), PathBuf::from("World")));
    ui.key(KeyCode::Esc);

    assert_eq!(ui.app.focused, FocusedArea::Content);
    assert!(ui.app.open_world_datapacks.is_none());
}

#[test]
fn installed_version_action_requires_selected_provider_match() {
    let mut ui = UiHarness::new();
    ui.add_instance("Unmatched");
    ui.app.focused = FocusedArea::Content;
    ui.app.content_tab = ContentTab::Mods;
    let minecraft = crate::storage::InstancePaths::new(ui.instance_path("Unmatched")).minecraft();
    let path = minecraft.join("mods/unknown.jar");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"unknown").unwrap();
    ui.app.mods_state.entries = vec![content_entry("Unknown mod", path)];
    ui.app.mods_state.list_state.selected = Some(0);
    let record = managed_mod_record(&minecraft, "mods/unknown.jar", "known", false, Vec::new());
    let project = record.resolved_project().unwrap().clone();
    ui.app.content_manifest = Some((
        "Unmatched".to_owned(),
        ContentManifest {
            files: vec![record],
            ..Default::default()
        },
    ));

    ui.key(KeyCode::Char('v'));
    assert!(ui.app.mods_discovery_state.version_popup.is_none());

    ui.app.mods_state.entries[0].provider_project = Some(project);
    ui.key(KeyCode::Char('v'));
    assert!(ui.app.mods_discovery_state.version_popup.is_some());
}

#[test]
fn installed_popup_navigation_preserves_local_filters() {
    let mut ui = UiHarness::new();
    ui.add_instance("Popup");
    ui.app.focused = FocusedArea::Content;
    ui.app.content_tab = ContentTab::Mods;
    let minecraft = crate::storage::InstancePaths::new(ui.instance_path("Popup")).minecraft();
    let path = minecraft.join("mods/known.jar");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"known").unwrap();
    ui.app.mods_state.entries = vec![content_entry("Known mod", path)];
    ui.app.mods_state.list_state.selected = Some(0);
    let record = managed_mod_record(&minecraft, "mods/known.jar", "known", false, Vec::new());
    let project = record.resolved_project().unwrap().clone();
    ui.app.content_manifest = Some((
        "Popup".to_owned(),
        ContentManifest {
            files: vec![record],
            ..Default::default()
        },
    ));
    ui.app.mods_state.entries[0].provider_project = Some(project);

    ui.key(KeyCode::Char('v'));
    assert!(ui.app.mods_discovery_state.version_popup.is_some());
    let popup = ui.app.mods_discovery_state.version_popup.as_mut().unwrap();
    popup.loading = false;
    popup.versions = vec![
        crate::net::modrinth::VersionInfo {
            id: "v1".to_owned(),
            project_id: "known".to_owned(),
            name: "V1".to_owned(),
            version_number: "1.0".to_owned(),
            game_versions: vec!["1.21.1".to_owned()],
            loaders: vec!["fabric".to_owned()],
            version_type: crate::net::modrinth::VersionType::Release,
            dependencies: Vec::new(),
            date_published: String::new(),
            files: Vec::new(),
        },
        crate::net::modrinth::VersionInfo {
            id: "v2".to_owned(),
            project_id: "known".to_owned(),
            name: "V2".to_owned(),
            version_number: "2.0".to_owned(),
            game_versions: vec!["1.21.1".to_owned()],
            loaders: vec!["fabric".to_owned()],
            version_type: crate::net::modrinth::VersionType::Release,
            dependencies: Vec::new(),
            date_published: String::new(),
            files: Vec::new(),
        },
    ];

    ui.draw();
    assert!(ui.app.mods_discovery_state.local_mode);
    let filters_before = ui.app.mods_discovery_state.filters.clone();

    ui.key_with(KeyCode::Left, KeyModifiers::CONTROL);
    assert_eq!(ui.app.focused, FocusedArea::Content);
    assert!(ui.app.mods_discovery_state.version_popup.is_some());

    // Navigating the popup versions must not flip the state into discovery
    // mode (which used to swap the filters and refresh the background list).
    ui.key(KeyCode::Char('j'));
    assert_eq!(
        ui.app
            .mods_discovery_state
            .version_popup
            .as_ref()
            .unwrap()
            .selected,
        1
    );
    assert!(ui.app.mods_discovery_state.local_mode);
    assert_eq!(ui.app.mods_discovery_state.filters, filters_before);
}

#[test]
fn installed_version_hint_requires_selected_provider_match() {
    let mut ui = UiHarness::new();
    ui.app.focused = FocusedArea::Content;
    ui.app.content_tab = ContentTab::Mods;
    ui.app.mods_state.entries = vec![content_entry(
        "Unknown mod",
        PathBuf::from("mods/unknown.jar"),
    )];
    ui.app.mods_state.list_state.selected = Some(0);

    ui.draw();
    assert!(!ui.screen().contains("[v] versions"));

    ui.app.mods_state.entries[0].provider_project = Some(ProviderProject {
        provider: "modrinth".to_owned(),
        project_id: "known".to_owned(),
        version_id: "known-version".to_owned(),
    });
    ui.draw();
    assert!(ui.screen().contains("[v] versions"));
}

#[test]
fn managed_modpack_instance_exposes_direct_version_selector() {
    let mut ui = UiHarness::new();
    ui.add_instance("Managed Pack");

    ui.draw();
    assert!(!ui.screen().contains("[v] versions"));
    ui.app.instances_state.instances[0].modpack_source = Some(ProviderProject {
        provider: "unsupported".to_owned(),
        project_id: "pack".to_owned(),
        version_id: "current".to_owned(),
    });

    ui.draw();
    assert!(ui.screen().contains("[v] versions"));
    ui.key(KeyCode::Char('v'));
    let popup = ui
        .app
        .modpack_versions_state
        .as_ref()
        .and_then(|state| state.version_popup.as_ref())
        .unwrap();
    assert!(!popup.selecting_minecraft_version);
    assert_eq!(popup.current_version_id.as_deref(), Some("current"));

    ui.key(KeyCode::Esc);
    assert!(ui.app.modpack_versions_state.is_none());

    ui.key(KeyCode::Char('v'));
    let popup = ui
        .app
        .modpack_versions_state
        .as_mut()
        .and_then(|state| state.version_popup.as_mut())
        .unwrap();
    popup.loading = false;
    popup.versions = vec![crate::net::modrinth::VersionInfo {
        id: "current".to_owned(),
        project_id: "pack".to_owned(),
        name: "Current".to_owned(),
        version_number: "1.0".to_owned(),
        game_versions: vec!["1.21.1".to_owned()],
        loaders: Vec::new(),
        version_type: crate::net::modrinth::VersionType::Release,
        dependencies: Vec::new(),
        date_published: String::new(),
        files: Vec::new(),
    }];
    ui.key(KeyCode::Enter);
    assert!(ui.app.modpack_versions_state.is_none());
    assert_eq!(
        ui.app.modpack_update_popup.as_ref().unwrap().action,
        crate::tui::widgets::popups::modpack_update::Action::Reinstall
    );

    ui.app.modpack_update_popup = None;
    ui.key(KeyCode::Char('v'));
    let popup = ui
        .app
        .modpack_versions_state
        .as_mut()
        .and_then(|state| state.version_popup.as_mut())
        .unwrap();
    popup.loading = false;
    popup.versions = vec![crate::net::modrinth::VersionInfo {
        id: "new".to_owned(),
        project_id: "pack".to_owned(),
        name: "New".to_owned(),
        version_number: "2.0".to_owned(),
        game_versions: vec!["1.21.1".to_owned()],
        loaders: Vec::new(),
        version_type: crate::net::modrinth::VersionType::Release,
        dependencies: Vec::new(),
        date_published: String::new(),
        files: Vec::new(),
    }];
    ui.key(KeyCode::Enter);
    assert_eq!(
        ui.app.modpack_update_popup.as_ref().unwrap().action,
        crate::tui::widgets::popups::modpack_update::Action::Change
    );
}

#[test]
fn world_datapack_version_action_requires_selected_provider_match() {
    let mut ui = UiHarness::new();
    ui.add_instance("Datapacks");
    ui.app.focused = FocusedArea::Content;
    ui.app.content_tab = ContentTab::Worlds;
    let minecraft = crate::storage::InstancePaths::new(ui.instance_path("Datapacks")).minecraft();
    let world = minecraft.join("saves/World");
    let path = world.join("datapacks/unknown.zip");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"pack").unwrap();
    ui.app.world_datapacks_state.entries = vec![content_entry("Unknown datapack", path)];
    ui.app.world_datapacks_state.list_state.selected = Some(0);
    ui.app.open_world_datapacks = Some(("World".to_owned(), world));
    let mut record = managed_mod_record(
        &minecraft,
        "saves/World/datapacks/unknown.zip",
        "known",
        false,
        Vec::new(),
    );
    record.kind = ContentKind::DataPack;
    let project = record.resolved_project().unwrap().clone();
    ui.app.content_manifest = Some((
        "Datapacks".to_owned(),
        ContentManifest {
            files: vec![record],
            ..Default::default()
        },
    ));

    ui.key(KeyCode::Char('v'));
    assert!(ui.app.datapacks_discovery_state.version_popup.is_none());

    ui.app.world_datapacks_state.entries[0].provider_project = Some(project);
    ui.key(KeyCode::Char('v'));
    assert!(ui.app.datapacks_discovery_state.version_popup.is_some());
}

#[test]
fn worlds_reserves_q_for_available_quick_launch() {
    let mut ui = UiHarness::new();
    ui.add_instance("Test Instance");
    ui.app.focused = FocusedArea::Content;
    ui.app.content_tab = ContentTab::Worlds;

    ui.key(KeyCode::Char('q'));
    assert!(!ui.app.exit, "unsupported Quick Play must not quit rmcl");

    let meta = serde_json::json!({
        "id": "1.21.1",
        "mainClass": "net.minecraft.client.main.Main",
        "arguments": {
            "game": [{
                "rules": [{
                    "action": "allow",
                    "features": { "is_quick_play_singleplayer": true }
                }],
                "value": ["--quickPlaySingleplayer", "${quickPlaySingleplayer}"]
            }],
            "jvm": []
        }
    });
    let meta_path = crate::storage::MetadataPaths::new(&ui.app.instance_manager.meta_dir)
        .versions()
        .join("1.21.1/meta.json");
    std::fs::create_dir_all(meta_path.parent().unwrap()).unwrap();
    std::fs::write(meta_path, serde_json::to_vec(&meta).unwrap()).unwrap();
    ui.app.world_quick_play_support = None;
    ui.draw();

    assert!(ui.screen().contains("quick launch"));
}

#[test]
fn mouse_wheel_uses_the_focused_views_scroll_navigation() {
    let mut ui = UiHarness::new();
    ui.app.focused = FocusedArea::OverviewExpanded;
    ui.app.log_overlay_max_scroll = 1;

    ui.mouse(MouseEventKind::ScrollDown);
    ui.mouse(MouseEventKind::ScrollDown);
    assert_eq!(ui.app.log_overlay_scroll, 1);

    ui.mouse(MouseEventKind::ScrollUp);
    assert_eq!(ui.app.log_overlay_scroll, 0);
}

#[test]
fn discovery_mode_recovers_from_a_hidden_tab_and_cycles_visible_tabs() {
    let mut ui = UiHarness::new();
    ui.app.focused = FocusedArea::Content;
    ui.app.content_tab = ContentTab::Logs;

    ui.key(KeyCode::Tab);
    assert_eq!(ui.app.content_mode, ContentMode::Discover);
    assert_eq!(ui.app.content_tab, ContentTab::Mods);

    ui.key(KeyCode::Right);
    assert_eq!(ui.app.content_tab, ContentTab::ResourcePacks);

    ui.key(KeyCode::Left);
    assert_eq!(ui.app.content_tab, ContentTab::Mods);
}

#[test]
fn versions_open_from_a_discovery_project_page() {
    let mut ui = UiHarness::new();
    ui.add_instance("Test Instance");
    ui.app.focused = FocusedArea::Content;
    ui.app.content_mode = ContentMode::Discover;
    ui.app.content_tab = ContentTab::Mods;
    ui.app.mods_discovery_state.list.entries.push(
        crate::tui::widgets::content::discovery::provider_project_entry(
            DiscoveryProject {
                id: "project".to_owned(),
                slug: "project".to_owned(),
                title: "Project".to_owned(),
                description: String::new(),
                downloads: 0,
                icon_url: None,
                icon_bytes: None,
            },
            "modrinth",
            "project".to_owned(),
            None,
        ),
    );
    ui.app.mods_discovery_state.list.list_state.selected = Some(0);
    ui.app.mods_discovery_state.begin_project_page();

    ui.key(KeyCode::Char('v'));

    assert!(ui.app.mods_discovery_state.version_popup.is_some());
}

#[test]
fn instance_delete_can_be_cancelled_without_touching_disk() {
    let mut ui = UiHarness::new();
    ui.add_instance("Test Instance");
    let instance_path = ui.instance_path("Test Instance");

    ui.key(KeyCode::Char('d'));
    assert_eq!(ui.app.focused, FocusedArea::ConfirmDelete);
    assert!(matches!(
        confirm::pending_target(),
        Some(confirm::ConfirmTarget::Instance { name }) if name == "Test Instance"
    ));

    ui.key(KeyCode::Esc);
    assert_eq!(ui.app.focused, FocusedArea::Instances);
    assert!(confirm::pending_target().is_none());
    assert_eq!(ui.app.instances_state.instances.len(), 1);
    assert!(instance_path.exists());
}

#[test]
fn confirmed_instance_delete_removes_state_and_disk() {
    let mut ui = UiHarness::new();
    let name = format!("rmcl-ui-test-{}", std::process::id());
    ui.add_instance(&name);
    let instance_path = ui.instance_path(&name);
    assert!(!crate::instance::desktop::exists(&name));

    ui.key(KeyCode::Char('d'));
    ui.draw();
    assert!(ui.screen().contains(&format!("Delete '{name}'")));
    assert!(
        ui.screen()
            .contains("This will permanently remove the instance")
    );
    ui.key(KeyCode::Char('y'));

    assert_eq!(ui.app.focused, FocusedArea::Instances);
    assert!(ui.app.instances_state.instances.is_empty());
    assert!(!instance_path.exists());
}

#[test]
fn confirmed_screenshot_delete_removes_state_and_file() {
    let mut ui = UiHarness::new();
    ui.add_instance("Screenshots");
    let path = ui
        .instance_path("Screenshots")
        .join(crate::storage::MINECRAFT_DIR_NAME)
        .join("screenshots")
        .join("shot.png");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"image").unwrap();
    ui.app.screenshots_state.entries = vec![crate::instance::screenshots::ScreenshotEntry {
        name: "shot.png".to_owned(),
        path: path.clone(),
        width: 1,
        height: 1,
    }];
    ui.app.focused = FocusedArea::Content;
    ui.app.content_tab = ContentTab::Screenshots;

    ui.key(KeyCode::Char('d'));
    ui.key(KeyCode::Enter);

    assert_eq!(ui.app.focused, FocusedArea::Content);
    assert!(ui.app.screenshots_state.entries.is_empty());
    assert!(!path.exists());
}

fn managed_mod_record(
    minecraft: &Path,
    path: &str,
    project_id: &str,
    automatic_dependency: bool,
    required_dependencies: Vec<ProviderProject>,
) -> ContentFileRecord {
    ContentFileRecord {
        relative_path: PathBuf::from(path),
        kind: ContentKind::Mod,
        enabled: true,
        fingerprint: crate::instance::content::manifest::fingerprint(&minecraft.join(path))
            .unwrap(),
        resolution: Resolution::Resolved {
            project: ProviderProject {
                provider: "modrinth".to_owned(),
                project_id: project_id.to_owned(),
                version_id: format!("{project_id}-version"),
            },
        },
        provider_aliases: Vec::new(),
        provider_checks: Vec::new(),
        required_dependencies,
        automatic_dependency,
        cleanup_eligible: automatic_dependency,
    }
}

fn content_entry(name: &str, path: PathBuf) -> ContentEntry {
    ContentEntry {
        file_stem: name.to_owned(),
        name: name.to_owned(),
        source_slug: None,
        installed_path: Some(path.clone()),
        provider_project: None,
        world_details: None,
        title_suffix: None,
        footer_label: None,
        footer_change: None,
        description: String::new(),
        enabled: true,
        icon_bytes: None,
        provider_icon: false,
        provider_description: false,
        path,
        icon_lines: None,
    }
}

#[test]
fn deleting_a_mod_offers_its_unused_dependency_chain() {
    let mut ui = UiHarness::new();
    ui.add_instance("Dependencies");
    let minecraft = ui
        .instance_path("Dependencies")
        .join(crate::storage::MINECRAFT_DIR_NAME);
    let root_path = minecraft.join("mods/root.jar");
    let library_path = minecraft.join("mods/library.jar");
    std::fs::create_dir_all(root_path.parent().unwrap()).unwrap();
    std::fs::write(&root_path, b"r").unwrap();
    std::fs::write(&library_path, b"l").unwrap();
    let dependency = ProviderProject {
        provider: "modrinth".to_owned(),
        project_id: "library".to_owned(),
        version_id: "library-version".to_owned(),
    };
    ContentManifest {
        version: 1,
        files: vec![
            managed_mod_record(&minecraft, "mods/root.jar", "root", false, vec![dependency]),
            managed_mod_record(&minecraft, "mods/library.jar", "library", true, Vec::new()),
        ],
    }
    .save(&crate::storage::InstancePaths::new(ui.instance_path("Dependencies")).content_manifest())
    .unwrap();
    ui.app.mods_state.entries = vec![content_entry("Root", root_path.clone())];
    ui.app.mods_state.list_state.selected = Some(0);
    ui.app.focused = FocusedArea::Content;
    ui.app.content_tab = ContentTab::Mods;

    ui.key(KeyCode::Char('d'));
    ui.key(KeyCode::Enter);

    assert!(!root_path.exists());
    assert!(library_path.exists());
    assert!(matches!(
        confirm::pending_target(),
        Some(confirm::ConfirmTarget::OrphanDependencies { paths })
            if paths == vec![library_path.clone()]
    ));
    assert_eq!(ui.app.focused, FocusedArea::ConfirmDelete);

    ui.key(KeyCode::Enter);

    assert!(!library_path.exists());
    assert_eq!(ui.app.focused, FocusedArea::Content);
    let manifest = ContentManifest::load(
        &crate::storage::InstancePaths::new(ui.instance_path("Dependencies")).content_manifest(),
    )
    .unwrap();
    assert!(manifest.files.is_empty());
}

#[test]
fn deleting_installed_content_from_either_tab_resets_discovery_version_sources() {
    for mode in [ContentMode::Discover, ContentMode::Installed] {
        let mut ui = UiHarness::new();
        ui.add_instance("Delete and install again");
        ui.app.sync_instance_content();
        let instance = ui.app.instances_state.selected_instance().unwrap().clone();
        let paths = crate::storage::InstancePaths::new(ui.instance_path(&instance.name));
        let minecraft = paths.minecraft();
        let path = minecraft.join("mods/root.jar");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"root").unwrap();
        let mut record = managed_mod_record(&minecraft, "mods/root.jar", "root", false, Vec::new());
        record.provider_aliases.push(ProviderProject {
            provider: "curseforge".to_owned(),
            project_id: "other-root".to_owned(),
            version_id: "other-version".to_owned(),
        });
        let sources = record
            .resolved_project()
            .into_iter()
            .chain(&record.provider_aliases)
            .cloned()
            .map(|source| ("root".to_owned(), source))
            .collect();
        let mut manifest = ContentManifest::default();
        manifest.upsert(record);
        manifest.save(&paths.content_manifest()).unwrap();
        ui.app.content_manifest = Some((instance.name.clone(), manifest.clone()));
        ui.app
            .mods_state
            .set_entries(vec![content_entry("Root", path.clone())]);
        let request = ui.app.mods_discovery_state.begin_search(&instance);
        request
            .stream
            .upsert(crate::tui::widgets::content::discovery::project_entry(
                DiscoveryProject {
                    id: "root".to_owned(),
                    slug: "root".to_owned(),
                    title: "Root".to_owned(),
                    description: String::new(),
                    downloads: 0,
                    icon_url: None,
                    icon_bytes: None,
                },
                Some(path.clone()),
            ));
        crate::tui::widgets::content::DiscoveryState::push_provider_result(
            &request.pending,
            request.generation,
            0,
            Ok(
                crate::tui::widgets::content::discovery::DiscoveryPageResult {
                    received: 1,
                    total_hits: 1,
                    ..Default::default()
                },
            ),
            sources,
        );
        ui.app.mods_discovery_state.drain_pending();
        ui.app.mods_discovery_state.drain_list(&ui.app.picker);
        ui.app
            .mods_discovery_state
            .refresh_installed_manifest(&manifest, &minecraft);
        ui.app.mods_discovery_state.list.list_state.selected = Some(0);
        ui.app.focused = FocusedArea::Content;
        ui.app.content_tab = ContentTab::Mods;
        ui.app.content_mode = mode;

        ui.key(KeyCode::Char('d'));
        ui.key(KeyCode::Enter);
        assert!(!path.exists());
        assert!(
            ContentManifest::load(&paths.content_manifest())
                .unwrap()
                .files
                .is_empty()
        );
        ui.app.content_mode = ContentMode::Discover;
        ui.key(KeyCode::Char('v'));
        let popup = ui.app.mods_discovery_state.version_popup.as_mut().unwrap();
        assert!(popup.installed_path.is_none());
        assert!(
            popup.current_version_id.is_none(),
            "{mode:?}: deleted version remains installed"
        );
        assert!(
            popup
                .sources
                .iter()
                .all(|source| source.version_id.is_empty())
        );
        assert_eq!(popup.title(), "Install Root");
        popup.loading = false;
        let switched = ui.app.mods_discovery_state.switch_version_source().unwrap();
        assert!(switched.current_version_id.is_none());
        assert_eq!(
            ui.app
                .mods_discovery_state
                .version_popup
                .as_ref()
                .unwrap()
                .title(),
            "Install Root"
        );
    }
}

#[test]
fn deleting_a_required_library_warns_but_can_continue() {
    let mut ui = UiHarness::new();
    ui.add_instance("Required");
    let minecraft = ui
        .instance_path("Required")
        .join(crate::storage::MINECRAFT_DIR_NAME);
    let library_path = minecraft.join("mods/library.jar");
    std::fs::create_dir_all(library_path.parent().unwrap()).unwrap();
    std::fs::write(&library_path, b"l").unwrap();
    std::fs::write(minecraft.join("mods/root.jar"), b"r").unwrap();
    ContentManifest {
        version: 1,
        files: vec![
            managed_mod_record(
                &minecraft,
                "mods/root.jar",
                "root",
                false,
                vec![ProviderProject {
                    provider: "modrinth".to_owned(),
                    project_id: "library".to_owned(),
                    version_id: "library-version".to_owned(),
                }],
            ),
            managed_mod_record(&minecraft, "mods/library.jar", "library", true, Vec::new()),
        ],
    }
    .save(&crate::storage::InstancePaths::new(ui.instance_path("Required")).content_manifest())
    .unwrap();
    ui.app.mods_state.entries = vec![content_entry("Library", library_path.clone())];
    ui.app.mods_state.list_state.selected = Some(0);
    ui.app.focused = FocusedArea::Content;
    ui.app.content_tab = ContentTab::Mods;

    ui.key(KeyCode::Char('d'));

    assert!(matches!(
        confirm::pending_target(),
        Some(confirm::ConfirmTarget::Content { dependents, .. })
            if dependents == vec!["root"]
    ));
    ui.key(KeyCode::Enter);
    assert!(!Path::new(&library_path).exists());
}

#[test]
fn content_delete_rejects_ownership_changed_after_confirmation_opened() {
    let mut ui = UiHarness::new();
    ui.add_instance("ChangedOwner");
    let paths = crate::storage::InstancePaths::new(ui.instance_path("ChangedOwner"));
    let minecraft = paths.minecraft();
    let path = minecraft.join("mods/root.jar");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"original").unwrap();
    let manifest = ContentManifest {
        version: 1,
        files: vec![managed_mod_record(
            &minecraft,
            "mods/root.jar",
            "root",
            false,
            Vec::new(),
        )],
    };
    manifest.save(&paths.content_manifest()).unwrap();
    ui.app.content_manifest = Some(("ChangedOwner".to_owned(), manifest));
    ui.app.mods_state.entries = vec![content_entry("Root", path.clone())];
    ui.app.mods_state.list_state.selected = Some(0);
    ui.app.focused = FocusedArea::Content;
    ui.app.content_tab = ContentTab::Mods;
    ui.key(KeyCode::Char('d'));

    let new_owner = ProviderProject {
        provider: "curseforge".to_owned(),
        project_id: "new-owner".to_owned(),
        version_id: "new".to_owned(),
    };
    ContentManifest::update(&paths.content_manifest(), |manifest| {
        manifest.files[0].resolution = Resolution::Resolved {
            project: new_owner.clone(),
        };
        Ok(())
    })
    .unwrap();
    ui.key(KeyCode::Enter);

    assert_eq!(std::fs::read(&path).unwrap(), b"original");
    assert_eq!(ui.app.mods_state.entries[0].path, path);
    assert_eq!(
        ContentManifest::load(&paths.content_manifest())
            .unwrap()
            .files[0]
            .resolved_project(),
        Some(&new_owner)
    );
}

#[test]
fn confirmed_account_delete_updates_the_account_panel() {
    let mut ui = UiHarness::new();
    ui.add_instance("preferred-account-test");
    ui.add_account("Player");
    ui.app.instances_state.instances[0].preferred_account = Some("Player".to_owned());
    ui.app
        .instance_manager
        .save(&ui.app.instances_state.instances[0])
        .unwrap();
    ui.app.focused = FocusedArea::Account;

    ui.key(KeyCode::Char('d'));
    ui.key(KeyCode::Char('y'));

    assert_eq!(ui.app.focused, FocusedArea::Account);
    assert!(ui.app.account_state.store.accounts.is_empty());
    assert_eq!(ui.app.account_state.list_state.selected, None);
    assert_eq!(
        ui.app
            .instances_state
            .selected_instance()
            .unwrap()
            .preferred_account,
        None
    );
    assert_eq!(
        ui.app
            .instance_manager
            .load_one("preferred-account-test")
            .unwrap()
            .preferred_account,
        None
    );
}

#[test]
fn typing_d_in_offline_account_name_does_not_open_delete_confirmation() {
    let mut ui = UiHarness::new();
    ui.add_account("MicrosoftPlayer");
    ui.app.focused = FocusedArea::Account;

    ui.key(KeyCode::Char('a'));
    ui.key(KeyCode::Char('o'));
    ui.key(KeyCode::Char('d'));

    assert_eq!(ui.app.focused, FocusedArea::Account);
    assert!(matches!(
        &ui.app.account_state.add_mode,
        crate::tui::widgets::account::AddMode::OfflineNameInput(name) if name == "d"
    ));
}

#[test]
fn switching_instances_discards_another_instances_update_review() {
    let mut ui = UiHarness::new();
    ui.add_instance("A");
    ui.add_instance("B");
    let mut update = crate::tui::widgets::content::update::State::checking(
        "A".to_owned(),
        ContentKind::Mod,
        None,
        Vec::new(),
    );
    update.phase = crate::tui::widgets::content::update::Phase::Review;
    ui.app.content_update_popup = Some(update);
    ui.app.instances_state.list_state.selected = Some(1);

    ui.key(KeyCode::Enter);

    assert!(ui.app.content_update_popup.is_none());
}

#[test]
fn checking_popup_locks_input_until_cancelled() {
    let mut ui = UiHarness::new();
    ui.add_instance("A");
    ui.app.content_update_popup = Some(crate::tui::widgets::content::update::State::checking(
        "A".to_owned(),
        ContentKind::Mod,
        None,
        Vec::new(),
    ));
    ui.app.focused = FocusedArea::Content;

    ui.key(KeyCode::Tab);
    assert!(ui.app.content_update_popup.is_some());
    assert_eq!(ui.app.content_tab, ContentTab::Mods);
    ui.draw();
    assert!(ui.screen().contains("Preparing updates"));

    ui.key(KeyCode::Esc);
    assert!(ui.app.content_update_popup.is_none());
}

#[test]
fn log_overlay_ignores_page_keys_and_stays_put_while_reading_history() {
    use crate::tui::logging::{clear_app_logs, push_app_log};
    let mut ui = UiHarness::new();
    clear_app_logs();
    for index in 0..40 {
        push_app_log(format!("12:00:{index:02}:INFO:rmcl: line {index}"));
    }
    ui.app.focused = FocusedArea::OverviewExpanded;
    ui.draw();
    // 30-row terminal: overlay 28 rows, minus border = 26 visible lines.
    assert_eq!(ui.app.log_overlay_max_scroll, 14);
    assert_eq!(ui.app.log_overlay_scroll, 14);

    ui.key(KeyCode::PageUp);
    assert_eq!(ui.app.log_overlay_scroll, 14);
    ui.key(KeyCode::PageDown);
    assert_eq!(ui.app.log_overlay_scroll, 14);
    assert!(ui.screen().contains("[g/G] top/bottom"));
    assert!(ui.screen().contains("[Esc] close"));
    assert!(!ui.screen().contains("PgUp"));
    assert!(!ui.screen().contains("[y]"));
    for _ in 0..5 {
        ui.key(KeyCode::Char('k'));
    }
    assert_eq!(ui.app.log_overlay_scroll, 9);
    for index in 40..45 {
        push_app_log(format!("12:01:{index:02}:INFO:rmcl: line {index}"));
    }
    ui.draw();
    assert_eq!(ui.app.log_overlay_max_scroll, 19);
    assert_eq!(ui.app.log_overlay_scroll, 9);

    ui.key(KeyCode::End);
    ui.draw();
    assert_eq!(ui.app.log_overlay_scroll, 19);
    push_app_log("12:02:00:INFO:rmcl: latest".to_owned());
    ui.draw();
    assert_eq!(ui.app.log_overlay_scroll, 20);
    clear_app_logs();
}

#[test]
fn log_overlay_level_filter_opens_cycles_and_closes() {
    use crate::tui::logging::{clear_app_logs, push_app_log};
    use crate::tui::widgets::content::discovery::CategoryFilter;
    let mut ui = UiHarness::new();
    clear_app_logs();
    push_app_log("12:00:00:ERROR:rmcl: failed".to_owned());
    push_app_log("12:00:00:INFO:rmcl: done".to_owned());
    ui.app.focused = FocusedArea::OverviewExpanded;

    ui.key(KeyCode::Char('f'));
    assert!(ui.app.log_filter_open);
    ui.draw();
    assert!(ui.screen().contains("Log levels"));
    assert!(ui.screen().contains("· Error"));
    ui.key(KeyCode::Enter);
    assert_eq!(ui.app.log_level_filters[0], Some(CategoryFilter::Include));
    ui.draw();
    assert!(ui.screen().contains("+ Error"));
    assert!(ui.screen().contains("1 filter(s)"));
    ui.key(KeyCode::Esc);
    ui.draw();
    assert!(ui.screen().contains("failed"));
    assert!(!ui.screen().contains("done"));
    ui.key(KeyCode::Char('f'));
    ui.key(KeyCode::Enter);
    ui.draw();
    assert!(ui.screen().contains("− Error"));
    ui.key(KeyCode::Esc);
    assert!(!ui.app.log_filter_open);
    assert_eq!(ui.app.log_level_filters[0], Some(CategoryFilter::Exclude));
    ui.draw();
    assert!(!ui.screen().contains("failed"));
    assert!(ui.screen().contains("done"));
    clear_app_logs();
}

#[test]
fn mouse_drag_selects_viewer_characters_and_click_clears_yank_hint() {
    use crate::instance::logs::files::LogFileEntry;
    use crossterm::event::KeyModifiers;
    let mut ui = UiHarness::new();
    ui.add_instance("A");
    ui.app.focused = FocusedArea::Content;
    ui.app.content_tab = ContentTab::Logs;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("a.log");
    std::fs::write(&path, "alpha\nbeta\ngamma").unwrap();
    ui.app.logs_state.entries = vec![LogFileEntry {
        name: "a.log".to_owned(),
        path,
    }];
    ui.app.logs_state.list_state.selected = Some(0);
    ui.app.logs_state.loaded_for = Some("A".to_owned());
    ui.app.logs_state.viewer_lines =
        vec!["alpha".to_owned(), "beta".to_owned(), "gamma".to_owned()];
    ui.draw();
    assert!(!ui.screen().contains("[y]"));
    let area = ui.app.logs_state.viewer_area;
    assert!(area.height >= 3);
    let click = |kind, row| MouseEvent {
        kind,
        column: area.x + 2,
        row,
        modifiers: KeyModifiers::NONE,
    };
    ui.app
        .handle_mouse_event(click(MouseEventKind::Down(MouseButton::Left), area.y));
    assert!(ui.app.logs_state.selection.range.is_none());
    ui.app
        .handle_mouse_event(click(MouseEventKind::Drag(MouseButton::Left), area.y + 1));
    ui.app
        .handle_mouse_event(click(MouseEventKind::Up(MouseButton::Left), area.y + 1));
    assert_eq!(ui.app.logs_state.selection.range, Some(((0, 2), (1, 2))));
    assert_eq!(
        ui.app
            .logs_state
            .selection
            .text(&["alpha", "beta", "gamma"])
            .as_deref(),
        Some("pha\nbe")
    );
    assert!(ui.app.logs_state.viewer_focused);
    ui.draw();
    assert!(ui.screen().contains("[y] yank"));
    assert!(ui.screen().contains("[g/G] top/bottom"));
    assert!(!ui.screen().contains("PgUp"));
    ui.app
        .handle_mouse_event(click(MouseEventKind::Down(MouseButton::Left), area.y));
    ui.app
        .handle_mouse_event(click(MouseEventKind::Up(MouseButton::Left), area.y));
    ui.draw();
    assert!(ui.app.logs_state.selection.range.is_none());
    assert!(!ui.screen().contains("[y]"));
}

#[test]
fn mouse_drag_selects_overlay_characters_for_yank_and_escape_closes() {
    use crate::tui::logging::{clear_app_logs, push_app_log};
    use crossterm::event::KeyModifiers;
    let mut ui = UiHarness::new();
    clear_app_logs();
    for index in 0..10 {
        push_app_log(format!("12:00:{index:02}:INFO:rmcl: line {index}"));
    }
    ui.app.focused = FocusedArea::OverviewExpanded;
    ui.draw();
    let area = ui.app.log_overlay_inner;
    assert!(area.height >= 3);
    let click = |kind, row| MouseEvent {
        kind,
        column: area.x + 2,
        row,
        modifiers: KeyModifiers::NONE,
    };
    ui.app
        .handle_mouse_event(click(MouseEventKind::Down(MouseButton::Left), area.y));
    ui.app
        .handle_mouse_event(click(MouseEventKind::Drag(MouseButton::Left), area.y + 2));
    ui.app
        .handle_mouse_event(click(MouseEventKind::Up(MouseButton::Left), area.y + 2));
    assert_eq!(ui.app.log_selection.range, Some(((0, 2), (2, 2))));
    ui.draw();
    assert!(ui.screen().contains("[y] yank"));
    ui.key(KeyCode::Char('y'));
    assert!(
        crate::feedback::errors::peek_all_errors()
            .iter()
            .any(|event| event.message.contains("Yanked 3 line(s)"))
    );
    ui.app
        .handle_mouse_event(click(MouseEventKind::Down(MouseButton::Left), area.y));
    ui.app
        .handle_mouse_event(click(MouseEventKind::Up(MouseButton::Left), area.y));
    ui.draw();
    assert!(!ui.screen().contains("[y]"));
    ui.app
        .handle_mouse_event(click(MouseEventKind::Down(MouseButton::Left), area.y));
    ui.app
        .handle_mouse_event(click(MouseEventKind::Drag(MouseButton::Left), area.y + 1));
    ui.key(KeyCode::Esc);
    assert_ne!(ui.app.focused, FocusedArea::OverviewExpanded);
    assert!(ui.app.log_selection.range.is_none());
    clear_app_logs();
}

#[test]
fn log_level_popup_does_not_delete_logs_or_leave_on_tab() {
    use crate::instance::logs::files::LogFileEntry;
    use ratatui::layout::{Constraint, Rect};
    let mut ui = UiHarness::new();
    ui.add_instance("A");
    ui.app.focused = FocusedArea::Content;
    ui.app.content_tab = ContentTab::Logs;
    ui.app.logs_state.loaded_for = Some("A".to_owned());
    ui.app.logs_state.entries.push(LogFileEntry {
        name: "a.log".to_owned(),
        path: ui.instance_path("A").join("a.log"),
    });
    ui.app.logs_state.list_state.selected = Some(0);
    ui.app.logs_state.viewer_lines = vec!["12:00:00:INFO:rmcl: hello".to_owned()];
    ui.key(KeyCode::Char('f'));
    ui.key(KeyCode::Char('d'));
    assert_eq!(ui.app.focused, FocusedArea::Content);
    assert!(ui.app.logs_state.filter_open);
    ui.key(KeyCode::Tab);
    assert_eq!(ui.app.content_mode, ContentMode::Installed);
    ui.draw();
    assert!(ui.screen().contains("· Error"));
    assert!(ui.screen().contains("[Esc] close"));
    let popup = Rect::new(0, 0, 100, 30).centered(Constraint::Length(26), Constraint::Length(7));
    assert_eq!(
        ui.screen()
            .lines()
            .nth(popup.y as usize)
            .unwrap()
            .chars()
            .skip(popup.x as usize + 2)
            .take(12)
            .collect::<String>(),
        " Log levels "
    );
    ui.key(KeyCode::Esc);
    assert!(!ui.app.logs_state.filter_open);
}

#[test]
fn selected_unlinked_content_shows_only_the_badge() {
    let mut ui = UiHarness::new();
    ui.add_instance("A");
    let minecraft = ui
        .instance_path("A")
        .join(crate::storage::MINECRAFT_DIR_NAME);
    let path = minecraft.join("mods/local.jar");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "local mod").unwrap();
    let mut record = managed_mod_record(&minecraft, "mods/local.jar", "local", false, Vec::new());
    record.resolution = Resolution::Unmatched {
        checked_at: 1,
        providers: vec!["modrinth".to_owned()],
    };
    let mut manifest = ContentManifest::default();
    manifest.upsert(record);
    ui.app
        .mods_state
        .entries
        .push(content_entry("scoreboardinternal", path));
    ui.app.mods_state.loaded_for = Some("A".to_owned());
    ui.app.mods_state.list_state.selected = Some(0);
    ui.app
        .mods_state
        .apply_manifest(&manifest, &minecraft, ContentKind::Mod);
    ui.app.focused = FocusedArea::Content;
    ui.draw();
    assert!(ui.screen().contains("Unlinked"));
    assert!(!ui.screen().contains("No online source linked"));
    assert!(!ui.screen().contains("version switching unavailable"));
    ui.key(KeyCode::Char('v'));
    assert!(ui.app.mods_discovery_state.version_popup.is_none());

    manifest.files[0].resolution = Resolution::Pending;
    ui.app
        .mods_state
        .apply_manifest(&manifest, &minecraft, ContentKind::Mod);
    ui.draw();
    assert!(!ui.screen().contains("Unlinked"));
    assert!(!ui.screen().contains("No online source linked"));
}

#[test]
fn settings_panel_routes_legacy_edit_keys_to_tui_popups() {
    let mut ui = UiHarness::new();
    ui.add_instance("settings-test");
    ui.app.focused = FocusedArea::Settings;

    ui.key(KeyCode::Right);
    ui.key(KeyCode::Char('e'));
    assert_eq!(ui.app.focused, FocusedArea::InstanceSettings);
    ui.draw();
    assert!(ui.screen().contains("Instance Settings · settings-test"));
    assert!(!ui.screen().contains("Instance Settings *"));
    assert!(ui.screen().contains("settings-test"));
    assert!(ui.screen().contains("Game version"));
    assert!(ui.screen().contains("Memory min"));
    assert!(ui.screen().contains("Desktop"));
    assert!(ui.screen().contains('◆'));
    assert!(!ui.screen().contains('█'));
    assert!(!ui.screen().contains("● enabled"));
    assert!(!ui.screen().contains("Integration"));
    assert!(!ui.screen().contains('▰'));
    ui.key(KeyCode::Down);
    ui.key(KeyCode::Enter);
    ui.draw();
    assert!(ui.screen().contains("Mod Loader · settings-test"));
    assert!(ui.screen().contains("Fabric"));
    assert!(ui.screen().contains("Forge"));
    ui.key(KeyCode::Esc);
    for _ in 0..6 {
        ui.key(KeyCode::Down);
    }
    ui.key(KeyCode::Enter);
    for character in "-Xfoo".chars() {
        ui.key(KeyCode::Char(character));
    }
    ui.draw();
    assert!(ui.screen().contains("-Xfoo"));
    ui.key(KeyCode::Enter);
    assert_eq!(ui.app.focused, FocusedArea::InstanceSettings);
    assert_eq!(
        ui.app.instances_state.selected_instance().unwrap().jvm_args,
        ["-Xfoo"]
    );
    ui.key(KeyCode::Esc);
    assert_eq!(ui.app.focused, FocusedArea::Settings);

    ui.key(KeyCode::Char('g'));
    assert_eq!(ui.app.focused, FocusedArea::GlobalSettings);
    ui.draw();
    assert!(ui.screen().contains("Launcher Settings"));
    assert!(ui.screen().contains("Appearance"));
    assert!(ui.screen().contains("Image rendering"));
    assert!(ui.screen().contains("Memory max"));
    assert!(ui.screen().contains('◆'));
    assert!(!ui.screen().contains('█'));
    assert!(!ui.screen().contains('▰'));
    ui.key(KeyCode::Enter);
    ui.draw();
    assert!(ui.screen().contains("Theme"));
    ui.key(KeyCode::Esc);
    ui.key(KeyCode::Esc);
    assert_eq!(ui.app.focused, FocusedArea::Settings);
}

#[test]
fn launcher_settings_expand_to_storage_and_confirm_cache_cleanup() {
    let mut ui = UiHarness::new();
    ui.add_instance("global-settings-test");
    ui.key(KeyCode::Char('G'));

    for _ in 0..18 {
        ui.key(KeyCode::Char('j'));
    }
    ui.draw();
    assert!(ui.screen().contains("Storage"));
    assert!(ui.screen().contains("Instances"));
    assert!(ui.screen().contains("Metadata"));

    for _ in 18..24 {
        ui.key(KeyCode::Char('j'));
    }
    ui.key(KeyCode::Enter);
    assert_eq!(ui.app.focused, FocusedArea::ConfirmDelete);
    assert!(matches!(
        confirm::pending_target(),
        Some(confirm::ConfirmTarget::LauncherCache)
    ));
    ui.draw();
    assert!(ui.screen().contains("Clear caches"));
    ui.key(KeyCode::Esc);
    assert_eq!(ui.app.focused, FocusedArea::GlobalSettings);
}

#[test]
fn runtime_settings_use_the_shared_confirmation_popup() {
    let mut ui = UiHarness::new();
    ui.add_instance("runtime-test");
    ui.app.focused = FocusedArea::Settings;
    ui.key(KeyCode::Right);
    ui.key(KeyCode::Char('e'));
    ui.app
        .instance_settings
        .as_mut()
        .unwrap()
        .draft
        .game_version = "1.21.2".to_owned();

    for _ in 0..5 {
        ui.key(KeyCode::Char('j'));
    }
    ui.key(KeyCode::Char('l'));

    assert_eq!(ui.app.focused, FocusedArea::ConfirmDelete);
    assert!(matches!(
        confirm::pending_target(),
        Some(confirm::ConfirmTarget::InstanceRuntime { name, .. }) if name == "runtime-test"
    ));
    ui.draw();
    assert!(ui.screen().contains("Change runtime"));
    assert!(!ui.screen().contains("Target:"));
    assert!(
        ui.screen()
            .contains("Some installed mods may be incompatible")
    );
    assert!(!ui.screen().contains("incompatible."));
    assert!(
        !crate::feedback::errors::ERROR_EVENTS
            .lock()
            .unwrap()
            .iter()
            .any(|event| {
                event.level == tracing::Level::WARN
                    && event.message == "Some installed mods may be incompatible"
            })
    );

    ui.key(KeyCode::Esc);
    assert_eq!(ui.app.focused, FocusedArea::InstanceSettings);
    ui.key(KeyCode::Esc);
    assert_eq!(ui.app.focused, FocusedArea::Settings);
}

#[test]
fn jvm_arguments_use_the_same_editor_controls_as_environment() {
    let mut ui = UiHarness::new();
    ui.add_instance("jvm-clear-test");
    ui.key(KeyCode::Char('E'));
    ui.app.instance_settings.as_mut().unwrap().draft.jvm_args =
        vec!["-XX:+UseG1GC".to_owned(), "-Xss1M".to_owned()];

    for _ in 0..7 {
        ui.key(KeyCode::Char('j'));
    }
    ui.key(KeyCode::Char('d'));

    assert_eq!(ui.app.focused, FocusedArea::InstanceSettings);
    assert_eq!(
        ui.app
            .instance_settings
            .as_ref()
            .unwrap()
            .draft
            .jvm_args
            .len(),
        2
    );
    ui.draw();
    assert!(!ui.screen().contains("[d] clear"));
}

#[test]
fn enabling_a_different_automatic_java_requires_confirmation() {
    let mut ui = UiHarness::new();
    ui.add_instance("java-auto-test");
    ui.key(KeyCode::Char('E'));
    ui.app.instance_settings.as_mut().unwrap().draft.java_path = Some("/custom/java".to_owned());

    for _ in 0..4 {
        ui.key(KeyCode::Char('j'));
    }
    ui.key(KeyCode::Char('a'));

    assert_eq!(ui.app.focused, FocusedArea::ConfirmDelete);
    assert!(matches!(
        confirm::pending_target(),
        Some(confirm::ConfirmTarget::AutomaticSelection {
            setting: confirm::AutomaticSetting::Java,
            instance: Some(name),
            ..
        }) if name == "java-auto-test"
    ));
    ui.draw();
    assert!(ui.screen().contains("Enable automatic Java"));
    assert!(
        ui.screen()
            .contains("Use the automatically selected Java runtime")
    );
    assert!(!ui.screen().contains("/custom/java"));

    ui.key(KeyCode::Esc);
    assert_eq!(ui.app.focused, FocusedArea::InstanceSettings);
    assert_eq!(
        ui.app
            .instance_settings
            .as_ref()
            .unwrap()
            .draft
            .java_path
            .as_deref(),
        Some("/custom/java")
    );
    ui.key(KeyCode::Char('a'));

    ui.key(KeyCode::Enter);

    assert_eq!(ui.app.focused, FocusedArea::InstanceSettings);
    assert_eq!(
        ui.app
            .instances_state
            .selected_instance()
            .unwrap()
            .java_path,
        None
    );
}

#[test]
fn enabling_a_different_automatic_account_requires_confirmation() {
    let mut ui = UiHarness::new();
    ui.add_instance("account-auto-test");
    ui.add_account("Active");
    ui.app
        .account_state
        .store
        .accounts
        .push(crate::auth::Account {
            uuid: "preferred".to_owned(),
            username: "Preferred".to_owned(),
            account_type: crate::auth::AccountType::Microsoft,
            active: false,
            refresh_token: Some("refresh".to_owned()),
            cached_mc_token: None,
            cached_mc_token_expires_at: None,
        });
    ui.app.instances_state.instances[0].preferred_account = Some("preferred".to_owned());
    ui.app
        .instance_manager
        .save(&ui.app.instances_state.instances[0])
        .unwrap();
    ui.key(KeyCode::Char('E'));
    for _ in 0..3 {
        ui.key(KeyCode::Char('j'));
    }

    ui.key(KeyCode::Char('a'));

    assert_eq!(ui.app.focused, FocusedArea::ConfirmDelete);
    assert!(matches!(
        confirm::pending_target(),
        Some(confirm::ConfirmTarget::AutomaticSelection {
            setting: confirm::AutomaticSetting::Account,
            instance: Some(name),
        }) if name == "account-auto-test"
    ));
    ui.draw();
    assert!(ui.screen().contains("Enable automatic account"));
    assert!(ui.screen().contains("Use the currently active account"));
    assert!(!ui.screen().contains("Preferred → Active"));

    ui.key(KeyCode::Esc);
    assert_eq!(
        ui.app
            .instance_settings
            .as_ref()
            .unwrap()
            .draft
            .preferred_account
            .as_deref(),
        Some("preferred")
    );
    ui.key(KeyCode::Char('a'));
    ui.key(KeyCode::Enter);
    assert_eq!(
        ui.app
            .instances_state
            .selected_instance()
            .unwrap()
            .preferred_account,
        None
    );
}

#[test]
fn instance_settings_validation_errors_use_the_toast_buffer() {
    let mut ui = UiHarness::new();
    ui.add_instance("toast-test");
    ui.key(KeyCode::Char('E'));
    let state = ui.app.instance_settings.as_mut().unwrap();
    state.draft.loader = crate::instance::ModLoader::Vanilla;
    state.draft.loader_version = None;

    ui.key(KeyCode::Char('j'));
    ui.key(KeyCode::Char('j'));
    ui.key(KeyCode::Enter);

    assert!(
        crate::feedback::errors::ERROR_EVENTS
            .lock()
            .unwrap()
            .iter()
            .any(|error| error.message == "Vanilla does not use a loader version")
    );
}

#[test]
fn enabling_an_empty_launch_hook_uses_a_warning_toast() {
    let mut ui = UiHarness::new();
    ui.add_instance("empty-hook-test");
    ui.key(KeyCode::Char('E'));
    for _ in 0..13 {
        ui.key(KeyCode::Char('j'));
    }

    ui.key(KeyCode::Char(' '));

    assert!(
        crate::feedback::errors::ERROR_EVENTS
            .lock()
            .unwrap()
            .iter()
            .any(|event| {
                event.level == tracing::Level::WARN
                    && event.message == "Enter a pre-launch command before enabling it"
            })
    );
}

#[test]
fn settings_use_java_memory_and_resolution_controls() {
    let mut ui = UiHarness::new();
    ui.add_instance("controls-test");
    ui.key(KeyCode::Char('E'));

    for _ in 0..4 {
        ui.key(KeyCode::Char('j'));
    }
    ui.key(KeyCode::Enter);
    ui.draw();
    assert!(ui.screen().contains("Java Runtime"));
    assert!(!ui.screen().contains("auto"));
    assert!(!ui.screen().contains("custom"));
    assert!(!ui.screen().contains("Automatic"));
    assert!(!ui.screen().contains("Custom path"));
    assert!(!ui.screen().contains("Manual"));
    ui.key(KeyCode::Esc);

    ui.app.instance_settings.as_mut().unwrap().draft.memory_min = Some("512M".to_owned());
    ui.key(KeyCode::Char('j'));
    ui.key(KeyCode::Char('l'));
    assert_eq!(
        ui.app
            .instance_settings
            .as_ref()
            .unwrap()
            .draft
            .memory_min
            .as_deref(),
        Some("1G")
    );
    assert_eq!(
        ui.app
            .instances_state
            .selected_instance()
            .unwrap()
            .memory_min
            .as_deref(),
        Some("1G")
    );

    for _ in 0..6 {
        ui.key(KeyCode::Char('j'));
    }
    ui.key(KeyCode::Enter);
    ui.draw();
    assert!(ui.screen().contains("Resolution"));
    assert!(ui.screen().contains("1920x1080"));
    assert!(!ui.screen().contains("Preset"));
    assert!(!ui.screen().contains("Inherit"));
    assert!(!ui.screen().contains("custom"));
    ui.key(KeyCode::Esc);
    ui.key(KeyCode::Esc);

    ui.key(KeyCode::Char('G'));
    for _ in 0..6 {
        ui.key(KeyCode::Char('j'));
    }
    ui.key(KeyCode::Enter);
    ui.draw();
    assert!(ui.screen().contains("Java Runtime"));
    assert!(!ui.screen().contains("auto"));
    assert!(!ui.screen().contains("Automatic"));
    ui.key(KeyCode::Esc);
    ui.key(KeyCode::Esc);
}

#[test]
fn instance_launch_options_autosave_through_the_editor() {
    let mut ui = UiHarness::new();
    ui.add_instance("launch-options-test");
    ui.add_account("Player");
    ui.key(KeyCode::Char('E'));

    for _ in 0..3 {
        ui.key(KeyCode::Char('j'));
    }
    ui.key(KeyCode::Enter);
    ui.key(KeyCode::Enter);

    for _ in 0..5 {
        ui.key(KeyCode::Char('j'));
    }
    ui.key(KeyCode::Enter);
    for character in "MESA_LOADER_DRIVER_OVERRIDE=zink".chars() {
        ui.key(KeyCode::Char(character));
    }
    ui.key(KeyCode::Enter);

    ui.key(KeyCode::Char('j'));
    ui.key(KeyCode::Char('j'));
    ui.key(KeyCode::Enter);

    ui.key(KeyCode::Char('k'));
    ui.draw();
    assert!(ui.screen().contains("Java"));
    ui.key(KeyCode::Char('c'));
    for character in "/opt/lib/libglfw.so.3".chars() {
        ui.key(KeyCode::Char(character));
    }
    ui.key(KeyCode::Enter);

    let saved = ui
        .app
        .instance_manager
        .load_one("launch-options-test")
        .unwrap();
    assert_eq!(
        saved
            .environment
            .get("MESA_LOADER_DRIVER_OVERRIDE")
            .map(String::as_str),
        Some("zink")
    );
    assert_eq!(saved.window_mode, crate::instance::WindowMode::Fullscreen);
    assert_eq!(saved.preferred_account.as_deref(), Some("Player"));
    assert_eq!(saved.glfw_path.as_deref(), Some("/opt/lib/libglfw.so.3"));
}

#[test]
fn settings_panel_keeps_direct_profile_management() {
    let mut ui = UiHarness::new();
    ui.add_instance("profile-test");
    std::fs::create_dir_all(ui.instance_path("profile-test").join("minecraft")).unwrap();
    ui.app.focused = FocusedArea::Settings;

    ui.key(KeyCode::Char('a'));
    for character in "main".chars() {
        ui.key(KeyCode::Char(character));
    }
    ui.key(KeyCode::Enter);

    assert_eq!(
        ui.app
            .instances_state
            .selected_instance()
            .unwrap()
            .config_sync_profile
            .as_deref(),
        Some("main")
    );
    ui.draw();
    assert!(ui.screen().contains("main"));
}

#[test]
fn instance_wizards_open_render_and_cancel_through_app_input() {
    let mut ui = UiHarness::new();

    ui.key(KeyCode::Char('a'));
    assert_eq!(ui.app.focused, FocusedArea::Popup);
    ui.draw();
    assert!(ui.screen().contains("New Instance"));
    ui.key(KeyCode::Esc);
    assert_eq!(ui.app.focused, FocusedArea::Instances);

    ui.key(KeyCode::Char('m'));
    assert_eq!(ui.app.focused, FocusedArea::ImportPopup);
    ui.draw();
    assert!(ui.screen().contains("Browse Modpacks"));
    ui.key(KeyCode::Char('i'));
    ui.draw();
    assert!(ui.screen().contains("Import Modpack"));
    ui.key(KeyCode::Esc);
    assert_eq!(ui.app.focused, FocusedArea::ImportPopup);
    ui.key(KeyCode::Esc);
    assert_eq!(ui.app.focused, FocusedArea::Instances);
}

#[test]
fn provider_conflict_renders_and_can_be_deferred() {
    let mut ui = UiHarness::new();
    ui.app.provider_conflict = Some(ProviderConflictState {
        relative_path: "mods/example.jar".into(),
        candidates: vec![
            ProviderProject {
                provider: "modrinth".to_owned(),
                project_id: "first".to_owned(),
                version_id: "1".to_owned(),
            },
            ProviderProject {
                provider: "curseforge".to_owned(),
                project_id: "second".to_owned(),
                version_id: "2".to_owned(),
            },
        ],
        selected: 0,
    });

    ui.draw();
    assert!(ui.screen().contains("Choose provider for example.jar"));

    ui.key(KeyCode::Down);
    assert_eq!(ui.app.provider_conflict.as_ref().unwrap().selected, 1);
    ui.key(KeyCode::Esc);

    assert!(ui.app.provider_conflict.is_none());
    assert!(
        ui.app
            .dismissed_provider_conflicts
            .contains(std::path::Path::new("mods/example.jar"))
    );
}

#[test]
fn provider_conflict_selection_is_persisted() {
    let mut ui = UiHarness::new();
    ui.add_instance("Conflict");
    let relative_path = std::path::PathBuf::from("mods/example.jar");
    let file = crate::storage::InstancePaths::new(ui.instance_path("Conflict"))
        .minecraft()
        .join(&relative_path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"example").unwrap();
    let candidates = vec![
        ProviderProject {
            provider: "modrinth".to_owned(),
            project_id: "first".to_owned(),
            version_id: "1".to_owned(),
        },
        ProviderProject {
            provider: "curseforge".to_owned(),
            project_id: "second".to_owned(),
            version_id: "2".to_owned(),
        },
    ];
    let manifest = crate::instance::ContentManifest {
        version: 1,
        files: vec![crate::instance::ContentFileRecord {
            relative_path: relative_path.clone(),
            kind: crate::instance::ContentKind::Mod,
            enabled: true,
            fingerprint: crate::instance::content::manifest::fingerprint(&file).unwrap(),
            resolution: crate::instance::Resolution::Ambiguous {
                candidates: candidates.clone(),
            },
            provider_aliases: Vec::new(),
            provider_checks: Vec::new(),
            required_dependencies: Vec::new(),
            automatic_dependency: false,
            cleanup_eligible: false,
        }],
    };
    let manifest_path =
        crate::storage::InstancePaths::new(ui.instance_path("Conflict")).content_manifest();
    manifest.save(&manifest_path).unwrap();
    ui.app.content_manifest = Some(("Conflict".to_owned(), manifest));
    ui.app.provider_conflict = Some(ProviderConflictState {
        relative_path: relative_path.clone(),
        candidates,
        selected: 0,
    });

    ui.key(KeyCode::Down);
    std::fs::write(&manifest_path, b"invalid").unwrap();
    ui.key(KeyCode::Enter);
    assert_eq!(ui.app.provider_conflict.as_ref().unwrap().selected, 1);
    ui.app
        .content_manifest
        .as_ref()
        .unwrap()
        .1
        .save(&manifest_path)
        .unwrap();
    ui.key(KeyCode::Enter);

    assert!(ui.app.provider_conflict.is_none());
    let saved = crate::instance::ContentManifest::load(&manifest_path).unwrap();
    assert!(matches!(
        &saved.record(&relative_path).unwrap().resolution,
        crate::instance::Resolution::Resolved { project }
            if project.provider == "curseforge" && project.project_id == "second"
    ));
    assert_eq!(
        saved.record(&relative_path).unwrap().provider_aliases,
        vec![ProviderProject {
            provider: "modrinth".to_owned(),
            project_id: "first".to_owned(),
            version_id: "1".to_owned(),
        }]
    );
}
