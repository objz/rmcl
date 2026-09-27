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

#[test]
fn discovery_reveals_rows_before_the_whole_page_is_drained() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    let request = state.begin_search(&instance("one", "1.21.1"));
    for index in 0..10 {
        assert!(request.stream.upsert(project_entry(
            DiscoveryProject {
                id: index.to_string(),
                slug: index.to_string(),
                title: index.to_string(),
                description: String::new(),
                downloads: 0,
                icon_url: None,
                icon_bytes: None,
            },
            None,
        )));
    }
    state.list.drain_pending();
    assert_eq!(state.list.entries.len(), 4);
    assert_eq!(state.list.filtered_indices().len(), 4);
    drain_discovery_rows(&mut state);
    assert_eq!(state.list.entries.len(), 10);
}

#[tokio::test]
async fn discovery_icon_cache_reuses_only_the_matching_provider_project() {
    let mut modrinth = project_entry(project("iris"), None);
    modrinth.icon_bytes = Some(vec![1]);
    let mut curseforge = modrinth.clone();
    curseforge.provider_project.as_mut().unwrap().provider = "curseforge".to_owned();
    curseforge.icon_bytes = Some(vec![2]);
    let mut icons = cached_icons([&modrinth, &curseforge].into_iter());
    let meta = tempfile::tempdir().unwrap();
    assert_eq!(
        cached_icon_bytes(&mut icons, meta.path(), "modrinth", "iris").await,
        Some(vec![1])
    );
    assert_eq!(
        cached_icon_bytes(&mut icons, meta.path(), "curseforge", "iris").await,
        Some(vec![2])
    );
    assert_eq!(
        cached_icon_bytes(&mut icons, meta.path(), "curseforge", "other").await,
        None
    );
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

fn drain_discovery_rows(state: &mut DiscoveryState) {
    while state.list.drain_pending() {}
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
fn sort_panel_keeps_results_navigation_available() {
    use crate::instance::content::provider::DiscoverySort;

    let mut state = DiscoveryState::new_modpacks();
    state.list.entries = vec![
        project_entry(project("one"), None),
        project_entry(project("two"), None),
    ];
    state.list.list_state.selected = Some(0);

    assert!(handle_key(&KeyEvent::from(KeyCode::Char('f')), &mut state));
    assert!(state.sort_panel_open);
    assert!(state.sort_panel_focused);
    assert!(handle_key(&KeyEvent::from(KeyCode::Char('h')), &mut state));
    assert!(handle_key(&KeyEvent::from(KeyCode::Char('j')), &mut state));
    assert_eq!(state.list.list_state.selected, Some(1));

    assert!(handle_key(&KeyEvent::from(KeyCode::Char('l')), &mut state));
    assert!(handle_key(&KeyEvent::from(KeyCode::Char('l')), &mut state));
    assert!(handle_key(&KeyEvent::from(KeyCode::Char('j')), &mut state));
    assert!(handle_key(&KeyEvent::from(KeyCode::Enter), &mut state));
    assert_eq!(state.sort, DiscoverySort::Popular);
    assert!(state.search_due());
}

#[test]
fn local_mode_panel_does_not_consume_list_navigation() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    state.list.entries = vec![
        project_entry(project("one"), None),
        project_entry(project("two"), None),
    ];
    state.list.list_state.selected = Some(0);
    state.set_local_mode(true);

    assert!(handle_key(&KeyEvent::from(KeyCode::Char('f')), &mut state));
    assert!(state.sort_panel_open);
    assert!(state.sort_panel_focused);
    assert!(handle_key(&KeyEvent::from(KeyCode::Char('h')), &mut state));
    assert!(!state.sort_panel_focused);

    // In local mode j/k must be passed through (return false) so the caller
    // can navigate its own installed list.
    assert!(!handle_key(&KeyEvent::from(KeyCode::Char('j')), &mut state));
    assert!(!handle_key(&KeyEvent::from(KeyCode::Char('k')), &mut state));
}

#[test]
fn filter_panel_applies_version_environment_and_category_filters() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    handle_key(&KeyEvent::from(KeyCode::Char('f')), &mut state);
    assert_eq!(state.sort_panel_page, DiscoveryPanelPage::Filters);

    handle_key(&KeyEvent::from(KeyCode::Right), &mut state);
    assert_eq!(state.filters.game_version, GameVersionFilter::Current);
    handle_key(&KeyEvent::from(KeyCode::Char('j')), &mut state);
    handle_key(&KeyEvent::from(KeyCode::Enter), &mut state);
    assert_eq!(state.filters.environment, EnvironmentFilter::Client);
    handle_key(&KeyEvent::from(KeyCode::Char('j')), &mut state);
    handle_key(&KeyEvent::from(KeyCode::Enter), &mut state);
    assert_eq!(
        state.filters.categories.get("adventure"),
        Some(&CategoryFilter::Include)
    );
    handle_key(&KeyEvent::from(KeyCode::Enter), &mut state);
    assert_eq!(
        state.filters.categories.get("adventure"),
        Some(&CategoryFilter::Exclude)
    );
    handle_key(&KeyEvent::from(KeyCode::Enter), &mut state);
    assert!(!state.filters.categories.contains_key("adventure"));
    handle_key(&KeyEvent::from(KeyCode::Char('h')), &mut state);
    assert!(!state.sort_panel_focused);
    assert!(state.search_due());
}

#[test]
fn filters_match_environment_and_include_exclude_categories() {
    let filters = DiscoveryFilters {
        game_version: GameVersionFilter::Specific(std::collections::BTreeMap::from([(
            "1.20.1".to_owned(),
            CategoryFilter::Exclude,
        )])),
        environment: EnvironmentFilter::Client,
        categories: std::collections::BTreeMap::from([
            ("adventure".to_owned(), CategoryFilter::Include),
            ("cursed".to_owned(), CategoryFilter::Exclude),
        ]),
    };
    let metadata = crate::net::modrinth::DiscoveryMetadata {
        categories: vec!["adventure".to_owned()],
        versions: vec!["1.21.1".to_owned()],
        client_side: "required".to_owned(),
        server_side: "unsupported".to_owned(),
    };
    assert!(filters.matches(Some(&metadata)));
    let excluded_version = crate::net::modrinth::DiscoveryMetadata {
        versions: vec!["1.20.1".to_owned()],
        ..metadata.clone()
    };
    assert!(!filters.matches(Some(&excluded_version)));
    let excluded = crate::net::modrinth::DiscoveryMetadata {
        categories: vec!["adventure".to_owned(), "cursed".to_owned()],
        ..metadata
    };
    assert!(!filters.matches(Some(&excluded)));
}

#[test]
fn preferred_provider_categories_only_map_shared_meanings() {
    crate::net::curseforge::seed_discovery_categories_for_test();
    let map =
        |slug, from, to, kind, modpacks| category_for_provider(slug, from, to, kind, modpacks);
    assert_eq!(
        map("library", "modrinth", "curseforge", ContentKind::Mod, false),
        Some("library-api")
    );
    assert_eq!(
        map(
            "library-api",
            "curseforge",
            "modrinth",
            ContentKind::Mod,
            false
        ),
        Some("library")
    );
    assert_eq!(
        map(
            "optimization",
            "modrinth",
            "curseforge",
            ContentKind::Mod,
            false
        ),
        Some("performance")
    );
    assert_eq!(
        map(
            "magic",
            "curseforge",
            "modrinth",
            ContentKind::DataPack,
            false
        ),
        Some("magic")
    );
    assert_eq!(
        map(
            "tech",
            "curseforge",
            "modrinth",
            ContentKind::ResourcePack,
            true
        ),
        Some("technology")
    );
    assert_eq!(
        map("create", "curseforge", "modrinth", ContentKind::Mod, false),
        None
    );
    assert_eq!(
        map(
            "blocks",
            "modrinth",
            "curseforge",
            ContentKind::ResourcePack,
            false
        ),
        None
    );
    let filters = DiscoveryFilters {
        categories: std::collections::BTreeMap::from([(
            map("library", "modrinth", "curseforge", ContentKind::Mod, false)
                .unwrap()
                .to_owned(),
            CategoryFilter::Include,
        )]),
        ..Default::default()
    };
    let metadata = crate::net::modrinth::DiscoveryMetadata {
        categories: vec!["library-api".to_owned()],
        ..Default::default()
    };
    assert!(filters.matches(Some(&metadata)));
}

#[test]
fn curseforge_discovery_uses_its_categories_and_supported_sorts() {
    crate::net::curseforge::seed_discovery_categories_for_test();
    let mut state = DiscoveryState::new(ContentKind::Mod);
    state.category_provider = "curseforge".to_owned();
    assert!(
        state
            .categories()
            .iter()
            .any(|(slug, label)| *slug == "library-api" && *label == "API and Library")
    );
    assert!(
        !state
            .categories()
            .iter()
            .any(|(slug, _)| *slug == "library")
    );
    assert_eq!(state.category_start(), 1);
    assert!(!state.has_environment_filter());
    assert_eq!(
        state.sorts(),
        &[
            crate::instance::content::provider::DiscoverySort::Relevance,
            crate::instance::content::provider::DiscoverySort::Popular,
            crate::instance::content::provider::DiscoverySort::Released,
            crate::instance::content::provider::DiscoverySort::Downloads,
            crate::instance::content::provider::DiscoverySort::Updated
        ]
    );
    state.filter_panel_selected = 1 + state
        .categories()
        .iter()
        .position(|(slug, _)| *slug == "library-api")
        .unwrap();
    state.apply_selected_filter();
    assert_eq!(
        state.filters.categories.get("library-api"),
        Some(&CategoryFilter::Include)
    );
}

#[test]
fn discovery_sort_cycles_up_down_then_best_match_up() {
    use crate::instance::content::provider::DiscoverySort;
    let mut state = DiscoveryState::new(ContentKind::Mod);
    assert_eq!(state.sort, DiscoverySort::Relevance);
    assert!(!state.sort_reversed);
    state.sort_panel_selected = state
        .sorts()
        .iter()
        .position(|sort| *sort == DiscoverySort::Downloads)
        .unwrap();
    state.apply_selected_sort();
    assert_eq!(state.sort, DiscoverySort::Downloads);
    assert!(!state.sort_reversed);
    state.apply_selected_sort();
    assert!(state.sort_reversed);
    state.apply_selected_sort();
    assert_eq!(state.sort, DiscoverySort::Relevance);
    assert!(!state.sort_reversed);
    state.sort_panel_selected = 0;
    state.apply_selected_sort();
    assert!(state.sort_reversed);
    state.reset_sort();
    assert_eq!(state.sort, DiscoverySort::Relevance);
    assert!(!state.sort_reversed);
}

#[test]
fn any_version_filter_requests_all_project_versions() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    state.filters.game_version = GameVersionFilter::Any;
    state
        .list
        .entries
        .push(project_entry(project("project"), None));
    state.list.list_state.selected = Some(0);

    assert!(state.begin_versions().unwrap().all_game_versions);
}

#[test]
fn specific_version_filter_is_kept_for_project_versions() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    state.filters.game_version = GameVersionFilter::Specific(std::collections::BTreeMap::from([(
        "1.20.1".to_owned(),
        CategoryFilter::Include,
    )]));
    state
        .list
        .entries
        .push(project_entry(project("project"), None));
    state.list.list_state.selected = Some(0);

    let request = state.begin_versions().unwrap();
    assert!(!request.all_game_versions);
    assert_eq!(request.game_version_overrides, ["1.20.1"]);
}

#[test]
fn multiple_version_filter_is_kept_for_project_versions() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    state.filters.game_version = GameVersionFilter::Specific(
        [
            ("1.20.1".to_owned(), CategoryFilter::Include),
            ("1.21.1".to_owned(), CategoryFilter::Include),
        ]
        .into_iter()
        .collect(),
    );
    state
        .list
        .entries
        .push(project_entry(project("project"), None));
    state.list.list_state.selected = Some(0);

    let request = state.begin_versions().unwrap();
    assert!(request.all_game_versions);
    assert_eq!(request.game_version_overrides, ["1.20.1", "1.21.1"]);
}

#[test]
fn filter_version_picker_cycles_include_and_exclude_inline() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    handle_key(&KeyEvent::from(KeyCode::Char('f')), &mut state);
    handle_key(&KeyEvent::from(KeyCode::Enter), &mut state);
    assert!(state.filter_version_picker_open);
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

    handle_key(&KeyEvent::from(KeyCode::Char('j')), &mut state);
    handle_key(&KeyEvent::from(KeyCode::Char('j')), &mut state);
    handle_key(&KeyEvent::from(KeyCode::Enter), &mut state);
    handle_key(&KeyEvent::from(KeyCode::Enter), &mut state);
    handle_key(&KeyEvent::from(KeyCode::Char('j')), &mut state);
    handle_key(&KeyEvent::from(KeyCode::Enter), &mut state);

    assert_eq!(
        state.filters.game_version,
        GameVersionFilter::Specific(std::collections::BTreeMap::from([
            ("1.20.1".to_owned(), CategoryFilter::Include),
            ("1.21.1".to_owned(), CategoryFilter::Exclude),
        ]))
    );
    assert!(state.filter_version_picker_open);
    assert!(state.search_due());
}

#[test]
fn curseforge_version_picker_only_offers_supported_includes() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    state.category_provider = "curseforge".to_owned();
    *state.filter_game_versions.lock().unwrap() =
        crate::tui::widgets::popups::LoadState::Loaded(vec![
            crate::instance::loader::GameVersion {
                id: "1.21.1".to_owned(),
                stable: true,
            },
        ]);
    state.filter_version_picker_index = 2;
    state.toggle_filter_game_version();
    assert_eq!(
        state.filters.game_version,
        GameVersionFilter::Specific(std::collections::BTreeMap::from([(
            "1.21.1".to_owned(),
            CategoryFilter::Include
        )]))
    );
    state.toggle_filter_game_version();
    assert_eq!(state.filters.game_version, GameVersionFilter::Current);
}

#[test]
fn resets_only_the_active_filter_section_or_sort() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    state.filters.environment = EnvironmentFilter::Client;
    state
        .filters
        .categories
        .insert("adventure".to_owned(), CategoryFilter::Include);
    state.filters.game_version = GameVersionFilter::Any;
    handle_key(&KeyEvent::from(KeyCode::Char('f')), &mut state);

    handle_key(&KeyEvent::from(KeyCode::Char('r')), &mut state);

    assert_eq!(state.filters.game_version, GameVersionFilter::Any);
    assert_eq!(state.filters.environment, EnvironmentFilter::Any);
    assert!(state.filters.categories.is_empty());
    assert!(state.search_due());

    state.sort = crate::instance::content::provider::DiscoverySort::Popular;
    handle_key(&KeyEvent::from(KeyCode::Char('l')), &mut state);
    handle_key(&KeyEvent::from(KeyCode::Char('r')), &mut state);
    assert_eq!(
        state.sort,
        crate::instance::content::provider::DiscoverySort::Relevance
    );
    assert_eq!(state.filters.game_version, GameVersionFilter::Any);

    handle_key(&KeyEvent::from(KeyCode::Char('h')), &mut state);
    handle_key(&KeyEvent::from(KeyCode::Enter), &mut state);
    assert!(state.filter_version_picker_open);
    handle_key(&KeyEvent::from(KeyCode::Char('r')), &mut state);
    assert_eq!(state.filters.game_version, GameVersionFilter::Current);
}

#[test]
fn modpack_version_reset_restores_any() {
    let mut state = DiscoveryState::new_modpacks();
    state.filters.game_version = GameVersionFilter::Current;
    handle_key(&KeyEvent::from(KeyCode::Char('f')), &mut state);
    handle_key(&KeyEvent::from(KeyCode::Enter), &mut state);
    handle_key(&KeyEvent::from(KeyCode::Char('r')), &mut state);
    assert_eq!(state.filters.game_version, GameVersionFilter::Any);
}

#[test]
fn installed_and_discovery_filters_stay_separate() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    state.filters.environment = EnvironmentFilter::Client;
    state.set_local_mode(true);
    assert_eq!(state.filters.game_version, GameVersionFilter::Any);
    assert_eq!(state.filters.environment, EnvironmentFilter::Any);
    state.filters.environment = EnvironmentFilter::Server;
    state.filters.game_version = GameVersionFilter::Current;
    state.set_local_mode(false);
    assert_eq!(state.filters.environment, EnvironmentFilter::Client);
    assert_eq!(state.filters.game_version, GameVersionFilter::Current);
    state.set_local_mode(true);
    assert_eq!(state.filters.environment, EnvironmentFilter::Server);
    assert_eq!(state.filters.game_version, GameVersionFilter::Current);
    state.reset_game_versions();
    assert_eq!(state.filters.game_version, GameVersionFilter::Any);
    assert!(!state.search_due());
}

#[test]
fn installed_sort_panel_has_local_fields_and_direction() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    state.set_local_mode(true);
    assert_eq!(state.local_sort_index, 5);
    assert!(!state.local_sort_descending);
    handle_key(&KeyEvent::from(KeyCode::Char('f')), &mut state);
    handle_key(&KeyEvent::from(KeyCode::Char('l')), &mut state);
    handle_key(&KeyEvent::from(KeyCode::Char('j')), &mut state);
    handle_key(&KeyEvent::from(KeyCode::Enter), &mut state);
    assert_eq!(state.local_sort_index, 6);
    assert!(!state.local_sort_descending);
    handle_key(&KeyEvent::from(KeyCode::Right), &mut state);
    assert_eq!(state.local_sort_index, 6);
    assert!(!state.local_sort_descending);
    handle_key(&KeyEvent::from(KeyCode::Enter), &mut state);
    assert!(state.local_sort_descending);
    handle_key(&KeyEvent::from(KeyCode::Enter), &mut state);
    assert_eq!(state.local_sort_index, 5);
    assert!(!state.local_sort_descending);
    handle_key(&KeyEvent::from(KeyCode::Enter), &mut state);
    assert_eq!(state.local_sort_index, 6);
    assert!(!state.local_sort_descending);
    handle_key(&KeyEvent::from(KeyCode::Char('j')), &mut state);
    handle_key(&KeyEvent::from(KeyCode::Char('j')), &mut state);
    assert_eq!(state.sort_panel_selected, 2);
    handle_key(&KeyEvent::from(KeyCode::Char('r')), &mut state);
    assert_eq!(state.local_sort_index, 5);
    handle_key(&KeyEvent::from(KeyCode::Char('k')), &mut state);
    handle_key(&KeyEvent::from(KeyCode::Char('k')), &mut state);
    handle_key(&KeyEvent::from(KeyCode::Enter), &mut state);
    assert_eq!(state.local_sort_index, 5);
    assert!(state.local_sort_descending);
    handle_key(&KeyEvent::from(KeyCode::Enter), &mut state);
    assert_eq!(state.local_sort_index, 5);
    assert!(!state.local_sort_descending);
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
                    metadata: HashMap::new(),
                    received: 2,
                    total_hits: 2,
                },
            ),
            (
                "curseforge",
                DiscoveryResults {
                    projects: vec![curseforge_duplicate, curseforge_fallback],
                    metadata: HashMap::new(),
                    received: 2,
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
                metadata: HashMap::new(),
                received: 2,
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
                metadata: HashMap::new(),
                received: 1,
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
                    metadata: HashMap::new(),
                    received: 20,
                    total_hits: 20,
                },
            ),
            (
                "curseforge",
                DiscoveryResults {
                    projects: vec![],
                    metadata: HashMap::new(),
                    received: 50,
                    total_hits: 200,
                },
            ),
        ],
        "modrinth",
        HashMap::new(),
    );

    assert_eq!(merged.total_hits, 200);
    assert_eq!(merged.received, 50);
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
            result: Box::new(Ok(crate::net::modrinth::ProjectInfo {
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
                ..crate::net::modrinth::ProjectInfo::default()
            })),
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
fn project_pages_with_matching_ids_are_cached_by_provider() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    let mut entry = project_entry(project("shared"), None);
    state.project_pages.insert(
        ("modrinth".to_owned(), "shared".to_owned()),
        crate::net::modrinth::ProjectInfo {
            id: "shared".to_owned(),
            title: "Modrinth project".to_owned(),
            ..Default::default()
        },
    );
    entry.provider_project.as_mut().unwrap().provider = "curseforge".to_owned();
    state.list.entries.push(entry);
    state.list.list_state.selected = Some(0);

    let request = state.begin_project_page().unwrap();
    assert_eq!(request.provider, "curseforge");
    assert!(request.cached_project.is_none());
    assert!(state.project_page.as_ref().unwrap().document.is_none());
}

#[test]
fn project_page_navigation_is_bounded_and_can_go_back() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    state.project_page = Some(ProjectPageState {
        request_id: 1,
        project_id: "project".to_owned(),
        provider: "modrinth".to_owned(),
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
        provider: "modrinth".to_owned(),
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

fn confirming_state_with_plan() -> DiscoveryState {
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
    let root_version = request.root.version.clone();
    let mut dep_version = version("0.9.0");
    dep_version.project_id = "dependency".to_owned();
    let planned = |title: &str, version: VersionInfo| {
        crate::instance::content::dependencies::PlannedInstall {
            provider: "modrinth".to_owned(),
            project_id: title.to_owned(),
            title: title.to_owned(),
            version,
            installed_path: None,
            kind: crate::instance::ContentKind::Mod,
            destination: std::path::PathBuf::from("mods"),
            provider_aliases: Vec::new(),
            required_dependencies: Vec::new(),
            automatic_dependency: false,
            cleanup_eligible: false,
            replacement: true,
        }
    };
    DiscoveryState::push_action_result(
        &request.pending,
        DiscoveryActionResult::Dependencies {
            request_id: request.request_id,
            project_id: request.project_id,
            result: Ok(crate::instance::content::dependencies::DependencyPlan {
                items: vec![
                    planned("project", root_version),
                    planned("dependency", dep_version),
                ],
                root_count: 1,
                optional_dependencies: 1,
            }),
        },
    );
    state.drain_pending();
    assert!(state.version_popup.as_ref().unwrap().confirming);
    state
}

#[test]
fn confirming_popup_toggles_skip_dependencies_with_s() {
    let mut state = confirming_state_with_plan();
    assert!(!state.version_popup.as_ref().unwrap().skip_dependencies);

    assert!(handle_key(&KeyEvent::from(KeyCode::Char('s')), &mut state));
    assert!(state.version_popup.as_ref().unwrap().skip_dependencies);
    assert!(handle_key(&KeyEvent::from(KeyCode::Char('s')), &mut state));
    assert!(!state.version_popup.as_ref().unwrap().skip_dependencies);

    // Toggling is blocked while loading or installing.
    state.version_popup.as_mut().unwrap().loading = true;
    assert!(handle_key(&KeyEvent::from(KeyCode::Char('s')), &mut state));
    assert!(!state.version_popup.as_ref().unwrap().skip_dependencies);
    state.version_popup.as_mut().unwrap().loading = false;
    state.version_popup.as_mut().unwrap().installing = true;
    assert!(handle_key(&KeyEvent::from(KeyCode::Char('s')), &mut state));
    assert!(!state.version_popup.as_ref().unwrap().skip_dependencies);
}

#[test]
fn skip_dependencies_does_nothing_without_dependency_changes() {
    let mut state = confirming_state_with_plan();
    let popup = state.version_popup.as_mut().unwrap();
    let plan = popup.dependency_plan.as_mut().unwrap();
    plan.items.truncate(plan.root_count);
    plan.optional_dependencies = 0;
    assert!(!plan.has_dependency_changes());

    assert!(handle_key(&KeyEvent::from(KeyCode::Char('s')), &mut state));
    assert!(!state.version_popup.as_ref().unwrap().skip_dependencies);

    let install = state.begin_install().unwrap();
    assert_eq!(install.dependency_plan.unwrap().items.len(), 1);
}

#[test]
fn skipped_dependencies_install_only_the_root() {
    let mut state = confirming_state_with_plan();
    assert!(handle_key(&KeyEvent::from(KeyCode::Char('s')), &mut state));

    let install = state.begin_install().unwrap();
    let plan = install.dependency_plan.as_ref().unwrap();
    assert_eq!(plan.items.len(), 1);
    assert_eq!(plan.items[0].title, "project");
    assert_eq!(plan.root_count, 1);
    assert_eq!(plan.optional_dependencies, 0);
}

#[test]
fn fresh_dependency_plan_resets_skip_dependencies() {
    let mut state = confirming_state_with_plan();
    assert!(handle_key(&KeyEvent::from(KeyCode::Char('s')), &mut state));
    assert!(state.version_popup.as_ref().unwrap().skip_dependencies);

    let popup = state.version_popup.as_ref().unwrap();
    let (request_id, project_id, pending) = (
        popup.request_id,
        popup.project_id.clone(),
        state.pending_actions.clone(),
    );
    DiscoveryState::push_action_result(
        &pending,
        DiscoveryActionResult::Dependencies {
            request_id,
            project_id,
            result: Ok(crate::instance::content::dependencies::DependencyPlan {
                items: Vec::new(),
                root_count: 0,
                optional_dependencies: 0,
            }),
        },
    );
    state.drain_pending();
    assert!(!state.version_popup.as_ref().unwrap().skip_dependencies);
}

#[test]
fn installed_mode_defaults_to_any_game_version() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    assert_eq!(state.filters.game_version, GameVersionFilter::Current);
    assert_eq!(state.active_filter_count(), 0);

    state.set_local_mode(true);
    assert_eq!(state.filters.game_version, GameVersionFilter::Any);
    assert_eq!(state.active_filter_count(), 0);

    // The same Any filter counts as active in discovery mode.
    state.set_local_mode(false);
    state.filters.game_version = GameVersionFilter::Any;
    assert_eq!(state.active_filter_count(), 1);
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
    drain_discovery_rows(&mut state);
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
    drain_discovery_rows(&mut state);
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
    drain_discovery_rows(&mut state);

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
    drain_discovery_rows(&mut state);
    state.search.query = "sodium".to_owned();
    state.search_changed();

    let refresh = state.begin_search(&instance);

    assert!(refresh.reconcile);
    assert_eq!(state.list.entries.len(), 2);
    assert!(!state.list.loading);
}

#[tokio::test]
async fn filtered_refresh_waits_for_finished_icons_before_switching_rows() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    let picker = ratatui_image::picker::Picker::halfblocks();
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(1, 1)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let instance = instance("one", "1.21.1");
    let initial = state.begin_search(&instance);
    initial.stream.upsert(project_entry(project("old"), None));
    initial
        .stream
        .upsert(project_entry(project("another"), None));
    drain_discovery_rows(&mut state);
    state.list.list_state.selected = Some(1);

    state.filter_panel_selected = state.category_start();
    state.apply_selected_filter();
    assert!(!state.filters.categories.is_empty());
    let refresh = state.begin_search(&instance);
    assert!(refresh.reconcile);
    let stems = (0..8)
        .map(|index| {
            let mut project = project(&format!("new-{index}"));
            if index == 7 {
                project.icon_url = Some("https://example.invalid/icon".to_owned());
            } else {
                project.icon_bytes = Some(png.get_ref().clone());
            }
            let stem = project.id.clone();
            refresh.stream.upsert(project_entry(project, None));
            stem
        })
        .collect();
    state.drain_list(&picker);
    assert_eq!(state.list.entries.len(), 2);
    assert_eq!(state.list.entries[0].name, "old");
    refresh.stream.order(stems);
    DiscoveryState::push_result(
        &refresh.pending,
        refresh.generation,
        0,
        Ok(DiscoveryPageResult {
            received: 8,
            total_hits: 8,
        }),
    );
    state.drain_pending();
    state.drain_list(&picker);
    assert_eq!(state.list.entries[0].name, "old");
    assert!(state.preparing_list.as_ref().unwrap().has_pending_icons());
    refresh.stream.send_icon_unavailable(
        "new-7".to_owned(),
        "new-7".into(),
        Some(("modrinth".to_owned(), "new-7".to_owned())),
    );
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while state.preparing_list.is_some() {
            state.drain_list(&picker);
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(state.list.entries.len(), 8);
    assert_eq!(state.list.list_state.selected, Some(1));
    assert!(!state.list.has_pending_icons());
    assert_eq!(state.list.filtered_indices().len(), 8);
    assert!(
        state
            .list
            .entries
            .iter()
            .all(|entry| entry.name.starts_with("new-"))
    );
}

#[test]
fn discovery_activity_tracks_search_pages_and_icon_loading() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    let instance = instance("one", "1.21.1");
    assert_eq!(state.activity_label(), None);
    let first = state.begin_search(&instance);
    assert_eq!(state.activity_label(), Some("Searching Discovery..."));
    let mut entry = project_entry(project("icon"), None);
    entry.provider_icon = true;
    first.stream.upsert(entry);
    state.list.drain_pending();
    DiscoveryState::push_result(
        &first.pending,
        first.generation,
        0,
        Ok(DiscoveryPageResult {
            received: 1,
            total_hits: 100,
        }),
    );
    state.drain_pending();
    assert_eq!(state.activity_label(), Some("Loading Discovery icons..."));
    first
        .stream
        .send_icon_unavailable("icon".to_owned(), "icon".into(), None);
    state.list.drain_pending();
    assert_eq!(state.activity_label(), None);
    let _next = state.begin_next_page().unwrap();
    assert_eq!(state.activity_label(), Some("Loading more results..."));
    state.page_loading = false;
    state.filter_version_picker_open = true;
    *state.filter_game_versions.lock().unwrap() = crate::tui::widgets::popups::LoadState::Loading;
    assert_eq!(state.activity_label(), Some("Loading game versions..."));
}

#[test]
fn rapidly_cycling_a_category_discards_superseded_rows_and_results() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    let picker = ratatui_image::picker::Picker::halfblocks();
    let instance = instance("one", "1.21.1");
    let first = state.begin_search(&instance);
    assert!(
        first
            .stream
            .upsert(project_entry(project("original"), None))
    );
    drain_discovery_rows(&mut state);
    state.filter_panel_selected = state.category_start()
        + state
            .categories()
            .iter()
            .position(|(slug, _)| *slug == "management")
            .unwrap();

    state.apply_selected_filter(); // include
    assert!(!first.stream.upsert(project_entry(project("late"), None)));
    DiscoveryState::push_result(
        &first.pending,
        first.generation,
        0,
        Ok(DiscoveryPageResult {
            received: 10,
            total_hits: 100,
        }),
    );
    state.drain_pending();
    state.list.drain_pending();
    assert_eq!(state.list.entries.len(), 1);
    assert_eq!(state.list.entries[0].name, "original");
    assert_eq!(state.total_hits, 0);

    let include = state.begin_search(&instance);
    include
        .stream
        .upsert(project_entry(project("wrong-filter"), None));
    state.drain_list(&picker);
    assert_eq!(state.list.entries[0].name, "original");
    state.apply_selected_filter(); // exclude before include returns
    assert!(
        !include
            .stream
            .upsert(project_entry(project("wrong-filter"), None))
    );
    state.list.drain_pending();
    assert_eq!(state.list.entries.len(), 1);
    assert!(state.search_due());

    let exclude = state.begin_search(&instance);
    exclude
        .stream
        .upsert(project_entry(project("excluded"), None));
    exclude.stream.order(vec!["excluded".to_owned()]);
    DiscoveryState::push_result(
        &exclude.pending,
        exclude.generation,
        0,
        Ok(DiscoveryPageResult {
            received: 1,
            total_hits: 1,
        }),
    );
    state.drain_pending();
    state.drain_list(&picker);
    assert_eq!(state.list.entries[0].name, "excluded");
}

#[test]
fn discovery_restores_cached_sort_and_continues_pagination() {
    let mut state = DiscoveryState::new(ContentKind::Mod);
    let instance = instance("one", "1.21.1");
    let first = state.begin_search(&instance);
    assert!(first.stream.upsert(project_entry(
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
    )));
    assert!(first.stream.upsert(project_entry(project("second"), None)));
    drain_discovery_rows(&mut state);
    DiscoveryState::push_result(
        &first.pending,
        first.generation,
        0,
        Ok(DiscoveryPageResult {
            received: 2,
            total_hits: 20,
        }),
    );
    state.drain_pending();
    state.sort = crate::instance::content::provider::DiscoverySort::Popular;
    let other = state.begin_search(&instance);
    assert!(!other.cached);
    state.list.list_state.selected = Some(1);
    state.sort = crate::instance::content::provider::DiscoverySort::Relevance;
    let restored = state.begin_search(&instance);
    assert!(restored.cached);
    assert!(!state.list.loading);
    assert_eq!(state.list.entries[0].name, "Cached");
    assert_eq!(state.list.list_state.selected, Some(1));
    assert_eq!(state.next_offset, 2);
    assert_eq!(state.total_hits, 20);
    DiscoveryState::push_result(
        &other.pending,
        other.generation,
        0,
        Ok(DiscoveryPageResult {
            received: 3,
            total_hits: 99,
        }),
    );
    state.drain_pending();
    assert_eq!(state.total_hits, 20);
    let next = state.begin_next_page().unwrap();
    assert_eq!(next.offset, 2);
    assert!(!next.cached);
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
    drain_discovery_rows(&mut state);
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
    drain_discovery_rows(&mut state);
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
    drain_discovery_rows(&mut state);
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
