// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use crossterm::event::{KeyCode, MouseEventKind};
use std::path::{Path, PathBuf};

use super::harness::UiHarness;
use crate::instance::content::entry::ContentEntry;
use crate::instance::{
    ContentFileRecord, ContentKind, ContentManifest, FileFingerprint, ProviderProject, Resolution,
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
    let path = ui
        .instance_path("Unmatched")
        .join(crate::storage::MINECRAFT_DIR_NAME)
        .join("mods/unknown.jar");
    ui.app.mods_state.entries = vec![content_entry("Unknown mod", path)];
    ui.app.mods_state.list_state.selected = Some(0);
    let record = managed_mod_record("mods/unknown.jar", "known", false, Vec::new());
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
    let world = ui
        .instance_path("Datapacks")
        .join(crate::storage::MINECRAFT_DIR_NAME)
        .join("saves/World");
    let path = world.join("datapacks/unknown.zip");
    ui.app.world_datapacks_state.entries = vec![content_entry("Unknown datapack", path)];
    ui.app.world_datapacks_state.list_state.selected = Some(0);
    ui.app.open_world_datapacks = Some(("World".to_owned(), world));
    let mut record = managed_mod_record(
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
    path: &str,
    project_id: &str,
    automatic_dependency: bool,
    required_dependencies: Vec<ProviderProject>,
) -> ContentFileRecord {
    ContentFileRecord {
        relative_path: PathBuf::from(path),
        kind: ContentKind::Mod,
        enabled: true,
        fingerprint: FileFingerprint {
            size: 1,
            modified_ns: 1,
            hashes: Default::default(),
        },
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
            managed_mod_record("mods/root.jar", "root", false, vec![dependency]),
            managed_mod_record("mods/library.jar", "library", true, Vec::new()),
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
fn deleting_a_required_library_warns_but_can_continue() {
    let mut ui = UiHarness::new();
    ui.add_instance("Required");
    let minecraft = ui
        .instance_path("Required")
        .join(crate::storage::MINECRAFT_DIR_NAME);
    let library_path = minecraft.join("mods/library.jar");
    std::fs::create_dir_all(library_path.parent().unwrap()).unwrap();
    std::fs::write(&library_path, b"l").unwrap();
    ContentManifest {
        version: 1,
        files: vec![
            managed_mod_record(
                "mods/root.jar",
                "root",
                false,
                vec![ProviderProject {
                    provider: "modrinth".to_owned(),
                    project_id: "library".to_owned(),
                    version_id: "library-version".to_owned(),
                }],
            ),
            managed_mod_record("mods/library.jar", "library", true, Vec::new()),
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
            fingerprint: crate::instance::FileFingerprint {
                size: 1,
                modified_ns: 0,
                hashes: Default::default(),
            },
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
