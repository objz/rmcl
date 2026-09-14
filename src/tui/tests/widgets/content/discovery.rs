// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;
use crate::tests::TEST_LOCK;
use chrono::Utc;
use crossterm::event::KeyModifiers;
use std::collections::HashMap;

#[test]
fn bracket_keys_change_pages() {
    assert_eq!(
        page_key_direction(&KeyEvent::new(KeyCode::Char('['), KeyModifiers::NONE)),
        Some(false)
    );
    assert_eq!(
        page_key_direction(&KeyEvent::new(KeyCode::Char(']'), KeyModifiers::NONE)),
        Some(true)
    );
}

#[test]
fn discovery_requests_four_viewport_pages() {
    let mut state = DiscoveryState::new_modpacks();
    assert_eq!(state.begin_modpack_search().limit, PAGE_SIZE);

    state.set_viewport_rows(30);
    assert_eq!(state.begin_modpack_search().limit, 40);

    state.set_viewport_rows(300);
    assert_eq!(state.begin_modpack_search().limit, PAGE_SIZE);
}

fn instance(name: &str, version: &str) -> InstanceConfig {
    InstanceConfig {
        name: name.to_string(),
        game_version: version.to_string(),
        loader: ModLoader::Fabric,
        loader_version: None,
        created: Utc::now(),
        last_played: None,
        java_path: None,
        memory_max: None,
        memory_min: None,
        jvm_args: Vec::new(),
        environment: Default::default(),
        window_mode: Default::default(),
        inherit_window_mode: false,
        resolution: None,
        inherit_resolution: false,
        preferred_account: None,
        pre_launch_command: Default::default(),
        post_exit_command: Default::default(),
        glfw_path: None,
        config_sync_profile: None,
        modpack_source: None,
    }
}

fn version(id: &str) -> VersionInfo {
    VersionInfo {
        id: id.to_owned(),
        project_id: "project".to_owned(),
        name: format!("Version {id}"),
        version_number: id.to_owned(),
        game_versions: vec!["1.21.1".to_owned()],
        loaders: vec!["fabric".to_owned()],
        version_type: crate::net::modrinth::VersionType::Release,
        dependencies: Vec::new(),
        date_published: "2026-01-02T12:00:00Z".to_owned(),
        files: Vec::new(),
    }
}

fn project(id: &str) -> DiscoveryProject {
    DiscoveryProject {
        id: id.to_owned(),
        slug: id.to_owned(),
        title: id.to_owned(),
        description: String::new(),
        downloads: 0,
        icon_url: None,
        icon_bytes: None,
    }
}

#[test]
fn content_mode_toggles_both_ways() {
    assert_eq!(ContentMode::Installed.toggle(), ContentMode::Discover);
    assert_eq!(ContentMode::Discover.toggle(), ContentMode::Installed);
}

#[test]
fn duplicate_provider_titles_share_one_identity() {
    let mut modrinth = project("sodium");
    modrinth.title = "Sodium".to_owned();
    let mut curseforge = project("sodium-reforged");
    curseforge.title = "SODIUM!".to_owned();
    curseforge.slug = "sodium".to_owned();
    assert_eq!(project_identity(&modrinth), project_identity(&curseforge));
}

#[test]
fn matching_titles_with_different_slugs_remain_distinct() {
    let mut original = project("sodium");
    original.title = "Sodium".to_owned();
    let mut fork = project("sodium-reforged");
    fork.title = "Sodium".to_owned();
    assert_ne!(project_identity(&original), project_identity(&fork));
}

#[test]
fn provider_merge_preserves_ranking_and_appends_fallbacks() {
    let mut modrinth_first = project("mr-first");
    modrinth_first.title = "Shared".to_owned();
    modrinth_first.slug = "shared".to_owned();
    let modrinth_second = project("mr-second");
    let mut curseforge_duplicate = project("cf-duplicate");
    curseforge_duplicate.title = "Shared".to_owned();
    curseforge_duplicate.slug = "shared".to_owned();
    let curseforge_fallback = project("cf-fallback");

    let merged = merge_provider_results(
        vec![
            (
                "modrinth",
                DiscoveryResults {
                    projects: vec![modrinth_first, modrinth_second],
                    total_hits: 2,
                },
            ),
            (
                "curseforge",
                DiscoveryResults {
                    projects: vec![curseforge_duplicate, curseforge_fallback],
                    total_hits: 2,
                },
            ),
        ],
        "modrinth",
        HashMap::new(),
    );

    assert_eq!(
        merged
            .projects
            .iter()
            .map(|project| project.project.id.as_str())
            .collect::<Vec<_>>(),
        ["mr-first", "mr-second", "cf-fallback"]
    );
    assert_eq!(merged.sources.len(), 4);
    assert_eq!(merged.sources[0].0, merged.sources[2].0);
}

#[test]
fn provider_merge_keeps_same_provider_title_collisions() {
    let mut first = project("first");
    first.title = "Same".to_owned();
    let mut second = project("second");
    second.title = "Same".to_owned();

    let merged = merge_provider_results(
        vec![(
            "modrinth",
            DiscoveryResults {
                projects: vec![first, second],
                total_hits: 2,
            },
        )],
        "modrinth",
        HashMap::new(),
    );

    assert_eq!(merged.projects.len(), 2);
    assert_ne!(merged.projects[0].stem, merged.projects[1].stem);
}

#[test]
fn provider_merge_keeps_the_preferred_project_across_pages() {
    let mut fallback = project("shared");
    fallback.title = "Shared".to_owned();
    let identity = project_identity(&fallback);
    let known = HashMap::from([(identity, ("shared".to_owned(), "modrinth".to_owned()))]);

    let merged = merge_provider_results(
        vec![(
            "curseforge",
            DiscoveryResults {
                projects: vec![fallback],
                total_hits: 200,
            },
        )],
        "modrinth",
        known,
    );

    assert!(merged.projects.is_empty());
    assert_eq!(merged.sources[0].0, "shared");
    assert_eq!(merged.total_hits, 200);
}

#[test]
fn provider_merge_uses_the_longest_provider_result_range() {
    let merged = merge_provider_results(
        vec![
            (
                "modrinth",
                DiscoveryResults {
                    projects: vec![],
                    total_hits: 20,
                },
            ),
            (
                "curseforge",
                DiscoveryResults {
                    projects: vec![],
                    total_hits: 200,
                },
            ),
        ],
        "modrinth",
        HashMap::new(),
    );

    assert_eq!(merged.total_hits, 200);
}

#[test]
fn version_popup_switches_to_the_other_provider() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    state
        .list
        .entries
        .push(project_entry(project("sodium"), None));
    state.list.list_state.selected = Some(0);
    let request = state.begin_search(&instance("test", "1.21.1"));
    DiscoveryState::push_provider_result(
        &request.pending,
        request.generation,
        request.offset,
        Ok(DiscoveryPageResult {
            received: 1,
            total_hits: 1,
        }),
        vec![
            (
                "sodium".to_owned(),
                crate::instance::ProviderProject {
                    provider: "modrinth".to_owned(),
                    project_id: "mr".to_owned(),
                    version_id: String::new(),
                },
            ),
            (
                "sodium".to_owned(),
                crate::instance::ProviderProject {
                    provider: "curseforge".to_owned(),
                    project_id: "cf".to_owned(),
                    version_id: String::new(),
                },
            ),
        ],
    );
    state.drain_pending();
    state
        .list
        .entries
        .push(project_entry(project("sodium"), None));
    state.list.list_state.selected = Some(0);
    let first = state.begin_versions().unwrap();
    state.version_popup.as_mut().unwrap().loading = false;
    let second = state.switch_version_source().unwrap();
    assert_ne!(second.provider, first.provider);
    assert_eq!(
        state.version_popup.as_ref().unwrap().provider,
        second.provider
    );
}

#[test]
fn datapack_versions_select_a_world_before_dependency_resolution() {
    let temp = tempfile::tempdir().unwrap();
    let minecraft = temp.path().join("minecraft");
    let world_path = minecraft.join("saves/world-folder");
    std::fs::create_dir_all(world_path.join("datapacks")).unwrap();
    let installed = world_path.join("datapacks/project.zip");
    std::fs::write(&installed, b"zip").unwrap();

    let mut state = DiscoveryState::new(ContentKind::DataPack);
    state
        .list
        .entries
        .push(project_entry(project("project"), Some(installed.clone())));
    state.list.list_state.selected = Some(0);
    let request = state.begin_versions().unwrap();
    let mut datapack_version = version("1.0.0");
    datapack_version.loaders = vec!["datapack".to_owned()];
    DiscoveryState::push_action_result(
        &request.pending,
        DiscoveryActionResult::Versions {
            request_id: request.request_id,
            project_id: request.project_id,
            result: Ok(vec![datapack_version]),
        },
    );
    state.drain_pending();

    let world = crate::instance::scan_one_world(&world_path, "world-folder", true);
    assert!(state.begin_world_selection(vec![world]));
    assert_eq!(
        state.version_popup.as_ref().unwrap().worlds.entries[0]
            .icon_lines
            .as_ref()
            .unwrap()
            .len(),
        3
    );
    let mut manifest = crate::instance::ContentManifest::default();
    manifest.upsert(crate::instance::ContentFileRecord {
        relative_path: PathBuf::from("saves/world-folder/datapacks/project.zip"),
        kind: ContentKind::DataPack,
        enabled: true,
        fingerprint: crate::instance::FileFingerprint {
            size: 3,
            modified_ns: 1,
            hashes: Default::default(),
        },
        resolution: crate::instance::Resolution::Resolved {
            project: crate::instance::ProviderProject {
                provider: "modrinth".to_owned(),
                project_id: "project".to_owned(),
                version_id: "old".to_owned(),
            },
        },
        provider_aliases: Vec::new(),
        provider_checks: Vec::new(),
        required_dependencies: Vec::new(),
        automatic_dependency: false,
        cleanup_eligible: false,
    });

    assert!(state.select_world(Some(&manifest), &minecraft));
    let dependency = state.begin_dependency_resolution().unwrap();
    assert_eq!(dependency.root.kind, ContentKind::DataPack);
    assert_eq!(
        dependency.root.target_world.as_deref(),
        Some(world_path.as_path())
    );
    assert_eq!(
        dependency.root.installed_path.as_deref(),
        Some(installed.as_path())
    );
}

#[test]
fn project_metadata_is_split_between_title_and_footer_badges() {
    let entry = project_entry(
        DiscoveryProject {
            id: "example".to_owned(),
            slug: "example".to_owned(),
            title: "Example".to_owned(),
            description: "Project description".to_owned(),
            downloads: 1_234,
            icon_url: None,
            icon_bytes: None,
        },
        Some(PathBuf::from("example.jar")),
    );

    assert_eq!(entry.title_suffix.as_deref(), Some("Installed"));
    assert_eq!(entry.footer_label.as_deref(), Some("1.2K downloads"));
    assert_eq!(entry.description, "Project description");
    assert_eq!(entry.path, PathBuf::from("example"));
    assert_eq!(entry.installed_path, Some(PathBuf::from("example.jar")));
}

#[test]
fn install_and_change_version_popups_only_differ_in_title() {
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
        .push(project_entry(project.clone(), None));
    state.list.list_state.selected = Some(0);
    state.begin_versions().unwrap();
    assert_eq!(
        state.version_popup.as_ref().unwrap().title(),
        "Install Project"
    );

    state.version_popup = None;
    state.list.entries[0] = project_entry(project, Some(PathBuf::from("mods/project.jar")));
    state.begin_versions().unwrap();
    assert_eq!(
        state.version_popup.as_ref().unwrap().title(),
        "Change Project version"
    );
}

#[test]
fn installed_version_popup_tracks_current_version_and_reinstalls_it() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    let entry = crate::instance::content::entry::ContentEntry {
        file_stem: "project".to_owned(),
        name: "Project".to_owned(),
        source_slug: None,
        installed_path: None,
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
        path: PathBuf::from("mods/project.jar"),
        icon_lines: None,
    };
    let record = crate::instance::ContentFileRecord {
        relative_path: entry.path.clone(),
        kind: ContentKind::Mod,
        enabled: true,
        fingerprint: crate::instance::FileFingerprint {
            size: 1,
            modified_ns: 1,
            hashes: Default::default(),
        },
        resolution: crate::instance::Resolution::Resolved {
            project: crate::instance::ProviderProject {
                provider: "modrinth".to_owned(),
                project_id: "project".to_owned(),
                version_id: "current".to_owned(),
            },
        },
        provider_aliases: vec![crate::instance::ProviderProject {
            provider: "curseforge".to_owned(),
            project_id: "42".to_owned(),
            version_id: "84".to_owned(),
        }],
        provider_checks: vec!["modrinth".to_owned(), "curseforge".to_owned()],
        required_dependencies: Vec::new(),
        automatic_dependency: false,
        cleanup_eligible: false,
    };

    let request = state
        .begin_installed_versions(&entry, &record, None)
        .unwrap();
    let first_provider = request.provider.clone();
    let first_version = request.current_version_id.clone().unwrap();
    assert_eq!(state.version_popup.as_ref().unwrap().sources.len(), 2);
    state.version_popup.as_mut().unwrap().loading = false;
    state.version_popup.as_mut().unwrap().versions = vec![version(&first_version)];
    assert_eq!(
        state.version_popup.as_ref().unwrap().title(),
        "Reinstall Project"
    );

    let switched = state.switch_version_source().unwrap();
    assert_ne!(switched.provider, first_provider);
    let switched_version = switched.current_version_id.clone().unwrap();

    state.version_popup.as_mut().unwrap().loading = false;
    state.version_popup.as_mut().unwrap().versions = vec![version(&switched_version)];
    let request = state.begin_dependency_resolution().unwrap();
    assert!(request.root.force_reinstall);
}

#[test]
fn compatible_versions_populate_the_open_popup() {
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
    state.list.entries.push(project_entry(project, None));
    state.list.list_state.selected = Some(0);
    let request = state.begin_versions().unwrap();
    DiscoveryState::push_action_result(
        &request.pending,
        DiscoveryActionResult::Versions {
            request_id: request.request_id,
            project_id: request.project_id,
            result: Ok(vec![version("1.0.0"), version("1.1.0")]),
        },
    );

    state.drain_pending();

    let popup = state.version_popup.as_ref().unwrap();
    assert!(!popup.loading);
    assert_eq!(popup.versions.len(), 2);
    assert_eq!(popup.selected, 0);
}

#[test]
fn modpacks_choose_minecraft_before_filtering_pack_versions() {
    let mut state = DiscoveryState::new_modpacks();
    state
        .list
        .entries
        .push(project_entry(project("pack"), None));
    state.list.list_state.selected = Some(0);
    let request = state.begin_versions().unwrap();
    let mut older = version("1.0.0");
    older.game_versions = vec!["1.20.1".to_owned(), "fabric".to_owned()];
    DiscoveryState::push_action_result(
        &request.pending,
        DiscoveryActionResult::Versions {
            request_id: request.request_id,
            project_id: request.project_id,
            result: Ok(vec![older, version("2.0.0")]),
        },
    );
    state.drain_pending();

    let popup = state.version_popup.as_mut().unwrap();
    assert!(popup.selecting_minecraft_version);
    assert_eq!(popup.minecraft_versions, ["1.21.1", "1.20.1"]);
    popup.selected = 1;

    assert!(state.select_minecraft_version());
    let popup = state.version_popup.as_ref().unwrap();
    assert_eq!(popup.selected_minecraft_version.as_deref(), Some("1.20.1"));
    assert_eq!(
        popup
            .visible_versions()
            .map(|version| version.version_number.as_str())
            .collect::<Vec<_>>(),
        ["1.0.0"]
    );
}

#[test]
fn managed_modpack_versions_open_directly_and_mark_reinstall() {
    let mut state = DiscoveryState::new_modpacks();
    let source = crate::instance::ProviderProject {
        provider: "modrinth".to_owned(),
        project_id: "pack".to_owned(),
        version_id: "current".to_owned(),
    };

    let request = state
        .begin_managed_modpack_versions("Managed Pack", source)
        .unwrap();
    assert_eq!(request.current_version_id.as_deref(), Some("current"));
    let popup = state.version_popup.as_mut().unwrap();
    assert!(!popup.selecting_minecraft_version);
    popup.loading = false;
    popup.versions = vec![version("current"), version("older")];

    assert_eq!(popup.title(), "Reinstall Managed Pack");
    popup.selected = 1;
    assert_eq!(popup.title(), "Change Managed Pack version");
}

#[test]
fn project_page_loads_for_the_selected_discovery_entry() {
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
    state.list.entries.push(project_entry(project, None));
    state.list.list_state.selected = Some(0);

    let request = state.begin_project_page().unwrap();
    assert!(state.project_page_open());
    DiscoveryState::push_action_result(
        &request.pending,
        DiscoveryActionResult::ProjectPage {
            request_id: request.request_id,
            project_id: request.project_id,
            result: Ok(crate::net::modrinth::ProjectInfo {
                id: "project".to_owned(),
                slug: "project".to_owned(),
                title: "Project page".to_owned(),
                description: "Short description".to_owned(),
                body: "Long **Markdown** description.".to_owned(),
                icon_url: None,
                categories: Vec::new(),
                additional_categories: Vec::new(),
                project_type: "mod".to_owned(),
                loaders: Vec::new(),
            }),
        },
    );

    state.drain_pending();
    let page = state.project_page.as_ref().unwrap();
    assert_eq!(page.title, "Project page");
    assert!(page.document.is_some());
    state.project_page = None;
    assert!(state.begin_project_page().is_none());
    assert!(
        state
            .project_page
            .as_ref()
            .is_some_and(|page| page.document.is_some())
    );
}

#[test]
fn project_page_navigation_is_bounded_and_can_go_back() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    state.project_page = Some(ProjectPageState {
        request_id: 1,
        project_id: "project".to_owned(),
        title: "Project".to_owned(),
        document: Some(crate::tui::widgets::markdown::Document::new(
            "Project", "Body",
        )),
        error: None,
        scroll: 0,
        max_scroll: 20,
    });

    handle_key(
        &KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE),
        &mut state,
    );
    assert_eq!(state.project_page.as_ref().unwrap().scroll, 10);
    handle_key(
        &KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE),
        &mut state,
    );
    assert_eq!(state.project_page.as_ref().unwrap().scroll, 20);
    handle_key(
        &KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
        &mut state,
    );
    assert!(!state.project_page_open());
}

#[test]
fn version_popup_owns_navigation_over_a_project_page() {
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
    state.list.entries.push(project_entry(project, None));
    state.list.list_state.selected = Some(0);
    state.project_page = Some(ProjectPageState {
        request_id: 1,
        project_id: "project".to_owned(),
        title: "Project".to_owned(),
        document: None,
        error: None,
        scroll: 0,
        max_scroll: 20,
    });
    let request = state.begin_versions().unwrap();
    DiscoveryState::push_action_result(
        &request.pending,
        DiscoveryActionResult::Versions {
            request_id: request.request_id,
            project_id: request.project_id,
            result: Ok(vec![version("1.0.0"), version("2.0.0")]),
        },
    );
    state.drain_pending();

    handle_key(
        &KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        &mut state,
    );

    assert_eq!(state.version_popup.as_ref().unwrap().selected, 1);
    assert_eq!(state.project_page.as_ref().unwrap().scroll, 0);
}

#[test]
fn confirmation_can_return_to_version_selection() {
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
    state.list.entries.push(project_entry(project, None));
    state.list.list_state.selected = Some(0);
    let request = state.begin_versions().unwrap();
    DiscoveryState::push_action_result(
        &request.pending,
        DiscoveryActionResult::Versions {
            request_id: request.request_id,
            project_id: request.project_id,
            result: Ok(vec![version("1.0.0")]),
        },
    );
    state.drain_pending();
    assert!(state.begin_confirmation());

    handle_key(
        &KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE),
        &mut state,
    );

    assert!(!state.version_popup.as_ref().unwrap().confirming);
    assert!(state.version_popup.is_some());
}

#[test]
fn dependency_resolution_opens_the_existing_confirmation() {
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
    state.list.entries.push(project_entry(project, None));
    state.list.list_state.selected = Some(0);
    let versions = state.begin_versions().unwrap();
    DiscoveryState::push_action_result(
        &versions.pending,
        DiscoveryActionResult::Versions {
            request_id: versions.request_id,
            project_id: versions.project_id,
            result: Ok(vec![version("1.0.0")]),
        },
    );
    state.drain_pending();
    let request = state.begin_dependency_resolution().unwrap();
    assert!(state.version_popup.as_ref().unwrap().loading);
    let root_version = request.root.version.clone();
    DiscoveryState::push_action_result(
        &request.pending,
        DiscoveryActionResult::Dependencies {
            request_id: request.request_id,
            project_id: request.project_id,
            result: Ok(crate::instance::content::dependencies::DependencyPlan {
                items: vec![crate::instance::content::dependencies::PlannedInstall {
                    provider: "modrinth".to_owned(),
                    project_id: "project".to_owned(),
                    title: "Project".to_owned(),
                    version: root_version,
                    installed_path: None,
                    kind: crate::instance::ContentKind::Mod,
                    destination: std::path::PathBuf::from("mods"),
                    provider_aliases: Vec::new(),
                    required_dependencies: Vec::new(),
                    automatic_dependency: false,
                    cleanup_eligible: false,
                    replacement: false,
                }],
                root_count: 1,
                optional_dependencies: 0,
            }),
        },
    );

    state.drain_pending();

    let popup = state.version_popup.as_ref().unwrap();
    assert!(popup.confirming);
    assert!(!popup.loading);
    assert!(popup.dependency_plan.is_some());
    assert!(state.begin_install().unwrap().dependency_plan.is_some());
}

#[test]
fn discovery_delete_only_clears_the_matching_installed_badge() {
    let first_path = PathBuf::from("mods/first.jar");
    let second_path = PathBuf::from("mods/second.jar");
    let project = |id: &str| DiscoveryProject {
        id: id.to_owned(),
        slug: id.to_owned(),
        title: id.to_owned(),
        description: String::new(),
        downloads: 0,
        icon_url: None,
        icon_bytes: None,
    };
    let mut state = DiscoveryState::new(ContentKind::Mod);
    state
        .list
        .entries
        .push(project_entry(project("first"), Some(first_path.clone())));
    state
        .list
        .entries
        .push(project_entry(project("second"), Some(second_path.clone())));
    state.list.list_state.selected = Some(0);

    let pending = state.pending_installed_delete().unwrap();
    assert_eq!(pending.path, first_path);
    assert!(state.clear_installed_path(&pending.path));

    assert_eq!(state.list.entries.len(), 2);
    assert!(state.list.entries[0].installed_path.is_none());
    assert!(state.list.entries[0].title_suffix.is_none());
    assert_eq!(
        state.list.entries[1].installed_path.as_deref(),
        Some(second_path.as_path())
    );
    assert_eq!(
        state.list.entries[1].title_suffix.as_deref(),
        Some("Installed")
    );
    assert!(state.pending_installed_delete().is_none());
}

#[test]
fn successful_install_marks_the_project_and_closes_the_popup() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner());
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
    state.list.entries.push(project_entry(project, None));
    state.list.list_state.selected = Some(0);
    let versions_request = state.begin_versions().unwrap();
    DiscoveryState::push_action_result(
        &versions_request.pending,
        DiscoveryActionResult::Versions {
            request_id: versions_request.request_id,
            project_id: versions_request.project_id,
            result: Ok(vec![version("1.0.0")]),
        },
    );
    state.drain_pending();
    assert!(state.begin_confirmation());
    let install = state.begin_install().unwrap();
    assert!(state.version_popup.is_none());
    DiscoveryState::push_action_result(
        &install.pending,
        DiscoveryActionResult::Install {
            request_id: install.request_id,
            generation: install.generation,
            project_id: install.project_id,
            project_title: install.project_title,
            result: Ok(InstallCompletion {
                path: PathBuf::from("mods/project.jar"),
                replaced: false,
                skipped: false,
                orphaned_dependencies: Vec::new(),
            }),
        },
    );

    state.drain_pending();
    assert!(state.version_popup.is_none());
    assert_eq!(
        state.list.entries[0].title_suffix.as_deref(),
        Some("Installed")
    );
    assert_eq!(
        state.list.entries[0].installed_path,
        Some(PathBuf::from("mods/project.jar"))
    );
    assert_eq!(state.list.entries[0].path, PathBuf::from("project"));
}

#[test]
fn installed_labels_follow_exact_manifest_projects() {
    let project = DiscoveryProject {
        id: "project-id".to_owned(),
        slug: "example-project".to_owned(),
        title: "Example Project".to_owned(),
        description: String::new(),
        downloads: 0,
        icon_url: None,
        icon_bytes: None,
    };
    let mut state = DiscoveryState::new(ContentKind::Mod);
    state.list.entries.push(project_entry(project, None));

    let mut manifest = crate::instance::ContentManifest::default();
    manifest.upsert(crate::instance::ContentFileRecord {
        relative_path: PathBuf::from("mods/example-project-1.0.0.jar"),
        kind: crate::instance::ContentKind::Mod,
        enabled: true,
        fingerprint: crate::instance::FileFingerprint {
            size: 1,
            modified_ns: 1,
            hashes: Default::default(),
        },
        resolution: crate::instance::Resolution::Resolved {
            project: crate::instance::ProviderProject {
                provider: "modrinth".to_owned(),
                project_id: "project-id".to_owned(),
                version_id: "version".to_owned(),
            },
        },
        provider_aliases: Vec::new(),
        provider_checks: Vec::new(),
        required_dependencies: Vec::new(),
        automatic_dependency: false,
        cleanup_eligible: false,
    });
    let mut sources = vec![(
        "example-project".to_owned(),
        crate::instance::ProviderProject {
            provider: "modrinth".to_owned(),
            project_id: "project-id".to_owned(),
            version_id: String::new(),
        },
    )];
    refresh_source_installed_versions(
        &mut sources,
        &DiscoveryTarget::Content(Box::new(ContentDiscoveryTarget {
            instance: instance("test", "1.21.1"),
            kind: ContentKind::Mod,
            manifest: Some(manifest.clone()),
            minecraft_dir: PathBuf::from("first"),
        })),
    );
    assert_eq!(sources[0].1.version_id, "version");

    state.refresh_installed_manifest(&manifest, std::path::Path::new("first"));
    assert_eq!(
        state.list.entries[0].title_suffix.as_deref(),
        Some("Installed")
    );
    assert_eq!(
        state.list.entries[0].installed_path,
        Some(PathBuf::from("first/mods/example-project-1.0.0.jar"))
    );
    state.list.list_state.selected = Some(0);
    let request = state.begin_versions().unwrap();
    assert_eq!(request.current_version_id.as_deref(), Some("version"));
    assert_eq!(
        state
            .version_popup
            .as_ref()
            .unwrap()
            .current_version_id
            .as_deref(),
        Some("version")
    );
    state.version_popup = None;

    state.refresh_installed_manifest(
        &crate::instance::ContentManifest::default(),
        std::path::Path::new("first"),
    );
    assert_eq!(state.list.entries[0].title_suffix, None);
    assert_eq!(state.list.entries[0].installed_path, None);
    assert_eq!(state.list.entries[0].path, PathBuf::from("project-id"));
}

#[test]
fn changing_instance_invalidates_results() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    let first = instance("one", "1.21.1");
    let second = instance("two", "1.21.1");
    let _request = state.begin_search(&first);

    assert!(!state.needs_search(&first));
    assert!(state.needs_search(&second));
}

#[test]
fn unavailable_vanilla_discovery_clears_cached_results() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    state.list.entries.push(project_entry(
        DiscoveryProject {
            id: "cached".to_owned(),
            slug: "cached".to_owned(),
            title: "Cached".to_owned(),
            description: String::new(),
            downloads: 0,
            icon_url: None,
            icon_bytes: None,
        },
        None,
    ));
    let mut vanilla = instance("vanilla", "1.21.1");
    vanilla.loader = ModLoader::Vanilla;
    assert_eq!(
        state.unavailable_message(&vanilla),
        Some("Vanilla does not support mods.")
    );

    state.set_unavailable(&vanilla);

    assert!(state.list.entries.is_empty());
    assert!(!state.page_loading);
    assert!(state.exhausted);
}

#[test]
fn changing_instance_compatibility_invalidates_results() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    let original = instance("one", "1.21.1");
    let mut other_version = original.clone();
    other_version.game_version = "1.20.1".to_owned();
    let mut other_loader = original.clone();
    other_loader.loader = ModLoader::NeoForge;
    let _request = state.begin_search(&original);

    assert!(state.needs_search(&other_version));
    assert!(state.needs_search(&other_loader));
}

#[test]
fn stale_search_result_is_ignored() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    let instance = instance("one", "1.21.1");
    let old = state.begin_search(&instance);
    let _new = state.begin_search(&instance);
    DiscoveryState::push_result(
        &old.pending,
        old.generation,
        old.offset,
        Ok(DiscoveryPageResult {
            received: 20,
            total_hits: 99,
        }),
    );

    state.drain_pending();

    assert_eq!(state.total_hits, 0);
    assert!(state.list.loading);
}

#[test]
fn next_page_prefetches_before_selection_reaches_the_end() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    state.set_viewport_rows(30);
    let instance = instance("one", "1.21.1");
    let first = state.begin_search(&instance);
    for index in 0..PAGE_SIZE {
        assert!(first.stream.send(project_entry(
            DiscoveryProject {
                id: index.to_string(),
                slug: index.to_string(),
                title: index.to_string(),
                description: String::new(),
                downloads: 0,
                icon_url: None,
                icon_bytes: None,
            },
            None
        )));
    }
    state.list.drain_pending();
    DiscoveryState::push_result(
        &first.pending,
        first.generation,
        first.offset,
        Ok(DiscoveryPageResult {
            received: PAGE_SIZE,
            total_hits: 300,
        }),
    );
    state.drain_pending();
    state.list.list_state.selected = Some(80);

    let next = state.begin_next_page().expect("next page should prefetch");
    assert_eq!(next.offset, PAGE_SIZE);
    assert!(state.begin_next_page().is_none());
}

#[test]
fn large_page_fills_a_tall_viewport_without_another_request() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    state.set_viewport_rows(90);
    let first = state.begin_search(&instance("one", "1.21.1"));
    for index in 0..PAGE_SIZE {
        assert!(first.stream.send(project_entry(
            DiscoveryProject {
                id: index.to_string(),
                slug: index.to_string(),
                title: index.to_string(),
                description: String::new(),
                downloads: 0,
                icon_url: None,
                icon_bytes: None,
            },
            None
        )));
    }
    state.list.drain_pending();
    DiscoveryState::push_result(
        &first.pending,
        first.generation,
        first.offset,
        Ok(DiscoveryPageResult {
            received: PAGE_SIZE,
            total_hits: 300,
        }),
    );
    state.drain_pending();

    assert!(state.begin_next_page().is_none());
}

#[test]
fn typing_keeps_loaded_results_until_remote_search_is_due() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    let request = state.begin_search(&instance("one", "1.21.1"));
    for title in ["Sodium", "Lithium"] {
        assert!(request.stream.send(project_entry(
            DiscoveryProject {
                id: title.to_lowercase(),
                slug: title.to_lowercase(),
                title: title.to_owned(),
                description: String::new(),
                downloads: 0,
                icon_url: None,
                icon_bytes: None,
            },
            None
        )));
    }
    state.list.drain_pending();

    handle_key(
        &KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE),
        &mut state,
    );
    for character in "sod".chars() {
        handle_key(
            &KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE),
            &mut state,
        );
    }

    assert_eq!(state.list.filtered_indices(), vec![0, 1]);
    assert_eq!(state.list.search.query, "sod");
    assert!(!state.search_due());
    state.search_changed_at = Some(std::time::Instant::now() - SEARCH_DEBOUNCE);
    assert!(state.search_due());
}

#[test]
fn search_refresh_keeps_rows_until_the_diff_arrives() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    let instance = instance("one", "1.21.1");
    let initial = state.begin_search(&instance);
    for title in ["Sodium", "Lithium"] {
        assert!(initial.stream.upsert(project_entry(
            DiscoveryProject {
                id: title.to_lowercase(),
                slug: title.to_lowercase(),
                title: title.to_owned(),
                description: String::new(),
                downloads: 0,
                icon_url: None,
                icon_bytes: (title == "Sodium").then(|| vec![1, 2, 3]),
            },
            None
        )));
    }
    state.list.drain_pending();
    state.search.query = "sodium".to_owned();
    state.search_changed();

    let refresh = state.begin_search(&instance);

    assert!(refresh.reconcile);
    assert!(refresh.loaded_icon_stems.contains("sodium"));
    assert!(!refresh.loaded_icon_stems.contains("lithium"));
    assert_eq!(state.list.entries.len(), 2);
    assert!(!state.list.loading);
}

#[test]
fn pagination_continues_across_multiple_pages() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    let instance = instance("one", "1.21.1");
    let first = state.begin_search(&instance);
    for index in 0..PAGE_SIZE {
        assert!(first.stream.upsert(project_entry(
            DiscoveryProject {
                id: index.to_string(),
                slug: index.to_string(),
                title: index.to_string(),
                description: String::new(),
                downloads: 0,
                icon_url: None,
                icon_bytes: None,
            },
            None
        )));
    }
    state.list.drain_pending();
    DiscoveryState::push_result(
        &first.pending,
        first.generation,
        first.offset,
        Ok(DiscoveryPageResult {
            received: PAGE_SIZE,
            total_hits: 300,
        }),
    );
    state.drain_pending();
    state.list.list_state.selected = Some(PAGE_SIZE - MIN_PREFETCH_ITEMS);

    let second = state.begin_next_page().unwrap();
    for index in PAGE_SIZE..PAGE_SIZE * 2 {
        assert!(second.stream.upsert(project_entry(
            DiscoveryProject {
                id: index.to_string(),
                slug: index.to_string(),
                title: index.to_string(),
                description: String::new(),
                downloads: 0,
                icon_url: None,
                icon_bytes: None,
            },
            None
        )));
    }
    state.list.drain_pending();
    DiscoveryState::push_result(
        &second.pending,
        second.generation,
        second.offset,
        Ok(DiscoveryPageResult {
            received: PAGE_SIZE,
            total_hits: 300,
        }),
    );
    state.drain_pending();
    state.list.list_state.selected = Some(PAGE_SIZE * 2 - MIN_PREFETCH_ITEMS);

    let third = state.begin_next_page().unwrap();
    assert_eq!(third.offset, PAGE_SIZE * 2);
}

#[test]
fn permanent_pagination_failure_stops_without_discarding_loaded_entries() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    let first = state.begin_search(&instance("one", "1.21.1"));
    for index in 0..PAGE_SIZE {
        assert!(first.stream.upsert(project_entry(
            DiscoveryProject {
                id: index.to_string(),
                slug: index.to_string(),
                title: index.to_string(),
                description: String::new(),
                downloads: 0,
                icon_url: None,
                icon_bytes: None,
            },
            None
        )));
    }
    state.list.drain_pending();
    DiscoveryState::push_result(
        &first.pending,
        first.generation,
        first.offset,
        Ok(DiscoveryPageResult {
            received: PAGE_SIZE,
            total_hits: 300,
        }),
    );
    state.drain_pending();
    state.list.list_state.selected = Some(PAGE_SIZE - MIN_PREFETCH_ITEMS);
    let second = state.begin_next_page().unwrap();
    DiscoveryState::push_result(
        &second.pending,
        second.generation,
        second.offset,
        Err(DiscoveryPageError {
            message: "invalid response".to_owned(),
            retryable: false,
        }),
    );
    state.drain_pending();

    assert!(state.begin_next_page().is_none());
    assert_eq!(state.list.entries.len(), PAGE_SIZE);
    assert!(state.exhausted);
}

#[test]
fn transient_pagination_failure_retries_the_same_offset_after_a_delay() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    let first = state.begin_search(&instance("one", "1.21.1"));
    DiscoveryState::push_result(
        &first.pending,
        first.generation,
        first.offset,
        Ok(DiscoveryPageResult {
            received: PAGE_SIZE,
            total_hits: 300,
        }),
    );
    state.drain_pending();

    let second = state.begin_next_page().unwrap();
    DiscoveryState::push_result(
        &second.pending,
        second.generation,
        second.offset,
        Err(DiscoveryPageError {
            message: "connection reset".to_owned(),
            retryable: true,
        }),
    );
    state.drain_pending();

    assert!(state.begin_next_page().is_none());
    state.retry_page_at = Some(std::time::Instant::now() - PAGE_RETRY_BASE_DELAY);
    assert_eq!(state.begin_next_page().unwrap().offset, PAGE_SIZE);
}
