// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
};

use crate::instance::content::entry::{ContentEntry, WorldDetails, WorldGameMode};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::Widget,
};

use super::{
    ContentListState, ContentStreamOrder, PendingContentImage, WatcherEventHandling,
    available_description_width, description_text_width, diff_directory, diff_event_paths,
    ellipsize, load_provider_metadata, read_dir_stems, right_aligned_footer_spans,
    square_icon_columns, title_suffix_spans, version_change_spans, watcher_event_handling,
    world_descriptions, world_game_mode_color,
};

fn entry(name: &str) -> ContentEntry {
    ContentEntry {
        file_stem: name.to_lowercase(),
        name: name.to_owned(),
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
        path: PathBuf::from(name.to_lowercase()),
        icon_lines: None,
    }
}

#[test]
fn selected_provider_project_tracks_the_filtered_selection() {
    let mut state = ContentListState {
        entries: vec![entry("Local only"), entry("Provider match")],
        ..Default::default()
    };
    state.entries[1].provider_project = Some(crate::instance::ProviderProject {
        provider: "modrinth".to_owned(),
        project_id: "matched".to_owned(),
        version_id: "version".to_owned(),
    });
    state.search.query = "local".to_owned();
    state.list_state.selected = Some(0);

    assert!(!state.selected_has_provider_project());
    state.search.query = "provider".to_owned();
    assert!(state.selected_has_provider_project());
}

#[test]
fn installed_file_size_sort_changes_direction() {
    let temp = tempfile::tempdir().unwrap();
    let small = temp.path().join("small.jar");
    let large = temp.path().join("large.jar");
    std::fs::write(&small, b"a").unwrap();
    std::fs::write(&large, b"longer").unwrap();
    let mut state = ContentListState {
        entries: vec![entry("Small"), entry("Large")],
        local_sort_index: 6,
        ..Default::default()
    };
    state.entries[0].path = small;
    state.entries[1].path = large;
    assert_eq!(state.filtered_indices(), [0, 1]);
    state.local_sort_descending = true;
    assert_eq!(state.filtered_indices(), [1, 0]);
    std::fs::write(&state.entries[0].path, b"much longer now").unwrap();
    assert_eq!(state.filtered_indices(), [1, 0]);
    state.set_entries(state.entries.clone());
    assert_eq!(state.filtered_indices(), [0, 1]);
    state.search.query = "large".to_owned();
    assert_eq!(state.filtered_indices(), [1]);
    state.search.query.clear();
    assert_eq!(state.filtered_indices(), [0, 1]);
}

#[test]
fn sort_and_filter_keep_the_selected_row_number() {
    use crate::tui::widgets::content::discovery::{DiscoveryFilters, GameVersionFilter};

    let mut state = ContentListState {
        entries: vec![
            entry("Alpha"),
            entry("Beta"),
            entry("Gamma"),
            entry("Delta"),
        ],
        ..Default::default()
    };
    state.list_state.selected = Some(2);
    let filters = DiscoveryFilters {
        game_version: GameVersionFilter::Any,
        ..Default::default()
    };
    state.set_installed_options(&filters, 5, true, "1.21.1", false);
    assert_eq!(state.list_state.selected, Some(2));
    assert_eq!(state.selected_entry().unwrap().name, "Beta");
    state.list_state.selected = Some(0);
    state.set_installed_options(&filters, 5, false, "1.21.1", false);
    assert_eq!(state.list_state.selected, Some(0));
    assert_eq!(state.selected_entry().unwrap().name, "Alpha");

    state.list_state.selected = Some(2);
    state.search.query = "Beta".to_owned();
    state.set_search_filtering(true);
    assert_eq!(state.list_state.selected, Some(0));
    assert_eq!(state.selected_entry().unwrap().name, "Beta");
}

#[test]
fn cached_rows_keep_matching_rendered_icons_only() {
    let picker = ratatui_image::picker::Picker::halfblocks();
    let mut state = ContentListState::default();
    let mut project = entry("Alpha");
    project.icon_bytes = Some(vec![1]);
    project.provider_project = Some(crate::instance::ProviderProject {
        provider: "modrinth".to_owned(),
        project_id: "alpha".to_owned(),
        version_id: String::new(),
    });
    state.entries.push(project.clone());
    state.image_protocols.insert(
        project.file_stem.clone(),
        picker.new_resize_protocol(image::DynamicImage::new_rgba8(1, 1)),
    );
    state.set_entries(vec![project.clone()]);
    assert!(state.image_protocols.contains_key("alpha"));
    assert!(state.requested_images.contains("alpha"));

    project.provider_project.as_mut().unwrap().provider = "curseforge".to_owned();
    state.set_entries(vec![project]);
    assert!(state.image_protocols.is_empty());
    assert!(state.requested_images.is_empty());
}

#[test]
fn file_sort_cache_refreshes_when_watcher_replaces_a_file() {
    let temp = tempfile::tempdir().unwrap();
    let small = temp.path().join("small.jar");
    let large = temp.path().join("large.jar");
    std::fs::write(&small, b"a").unwrap();
    std::fs::write(&large, b"longer").unwrap();
    let mut state = ContentListState {
        entries: vec![entry("Small"), entry("Large")],
        local_sort_index: 6,
        ..Default::default()
    };
    state.entries[0].path = small.clone();
    state.entries[1].path = large;
    assert_eq!(state.filtered_indices(), [0, 1]);
    std::fs::write(&small, b"much longer now").unwrap();
    let mut replacement = entry("Small");
    replacement.path = small;
    *state.watcher_diff.lock().unwrap() = Some(super::WatcherDiff {
        toggled: Vec::new(),
        removed: vec!["small".to_owned()],
        added: vec![replacement],
    });
    state.drain_watcher();
    assert_eq!(state.filtered_indices(), [1, 0]);
}

#[test]
fn installed_name_fallback_sorts_both_directions() {
    let mut state = ContentListState {
        entries: vec![entry("Zebra"), entry("Axiom")],
        ..Default::default()
    };
    state.set_installed_options(
        &crate::tui::widgets::content::discovery::DiscoveryFilters {
            game_version: crate::tui::widgets::content::discovery::GameVersionFilter::Any,
            ..Default::default()
        },
        5,
        false,
        "1.21.1",
        false,
    );
    assert_eq!(state.filtered_indices(), [1, 0]);
    state.local_sort_descending = true;
    assert_eq!(state.filtered_indices(), [0, 1]);
}

#[test]
fn installed_filters_use_cached_project_and_version_metadata() {
    use crate::tui::widgets::content::discovery::{
        CategoryFilter, DiscoveryFilters, GameVersionFilter,
    };

    let mut state = ContentListState {
        entries: vec![entry("Unmatched"), entry("Matching")],
        ..Default::default()
    };
    state.entries[1].provider_project = Some(crate::instance::ProviderProject {
        provider: "modrinth".to_owned(),
        project_id: "matching".to_owned(),
        version_id: "version".to_owned(),
    });
    state.project_metadata.insert(
        ("modrinth".to_owned(), "matching".to_owned()),
        crate::net::modrinth::ProjectInfo {
            categories: vec!["adventure".to_owned()],
            ..Default::default()
        },
    );
    state.version_metadata.insert(
        ("modrinth".to_owned(), "version".to_owned()),
        crate::net::modrinth::VersionInfo {
            id: "version".to_owned(),
            project_id: "matching".to_owned(),
            name: String::new(),
            version_number: String::new(),
            game_versions: vec!["1.21.1".to_owned()],
            loaders: Vec::new(),
            version_type: Default::default(),
            dependencies: Vec::new(),
            date_published: String::new(),
            files: Vec::new(),
        },
    );
    let filters = DiscoveryFilters {
        game_version: GameVersionFilter::Specific(std::collections::BTreeMap::from([(
            "1.21.1".to_owned(),
            CategoryFilter::Include,
        )])),
        categories: std::collections::BTreeMap::from([(
            "adventure".to_owned(),
            CategoryFilter::Include,
        )]),
        ..Default::default()
    };
    state.set_installed_options(&filters, 0, false, "1.21.1", true);
    assert_eq!(state.filtered_indices(), [1]);
    state.local_game_version = "1.20.1".to_owned();
    state.local_filters.game_version = GameVersionFilter::Current;
    assert!(state.filtered_indices().is_empty());
}

#[test]
fn world_cards_preview_up_to_three_datapacks() {
    let lines = world_descriptions(&WorldDetails {
        game_mode: None,
        last_played: None,
        minecraft_version: Some("1.21.1".to_owned()),
        size: Some("4.0 MB".to_owned()),
        datapacks: ["A", "B", "C", "D"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
    });

    assert_eq!(
        lines,
        ["1.21.1  •  4.0 MB", "  • A", "  • B", "  • C", "  +1 more"]
    );
}

#[test]
fn toggling_a_selected_entry_renames_and_updates_it() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("example.jar");
    std::fs::write(&path, b"mod").unwrap();
    let mut content = entry("Example");
    content.path = path.clone();
    let mut state = ContentListState::default();
    state.entries.push(content);
    state.list_state.selected = Some(0);

    state.toggle_selected();

    assert!(!path.exists());
    assert_eq!(
        state.entries[0].path,
        temp.path().join("example.jar.disabled")
    );
    assert!(!state.entries[0].enabled);
}

#[test]
fn content_watcher_ignores_file_access_events() {
    assert_eq!(
        watcher_event_handling(&notify::EventKind::Access(notify::event::AccessKind::Any)),
        WatcherEventHandling::Ignore
    );
}

#[test]
fn content_watcher_handles_mutations_without_full_rescan() {
    assert_eq!(
        watcher_event_handling(&notify::EventKind::Modify(notify::event::ModifyKind::Any)),
        WatcherEventHandling::Paths
    );
    assert_eq!(
        watcher_event_handling(&notify::EventKind::Modify(notify::event::ModifyKind::Name(
            notify::event::RenameMode::Both
        ))),
        WatcherEventHandling::Rescan
    );
    assert_eq!(
        watcher_event_handling(&notify::EventKind::Any),
        WatcherEventHandling::Rescan
    );
}

#[test]
fn content_watcher_keeps_a_renamed_disabled_entry() {
    let temp = tempfile::tempdir().unwrap();
    let enabled = temp.path().join("example.jar");
    let disabled = temp.path().join("example.jar.disabled");
    std::fs::write(&enabled, b"mod").unwrap();
    let known = Arc::new(Mutex::new(read_dir_stems(temp.path(), ".jar")));

    std::fs::rename(enabled, &disabled).unwrap();
    let diff = diff_directory(temp.path(), ".jar", None, &known).unwrap();

    assert_eq!(diff.toggled, vec![("example".to_owned(), false, disabled)]);
    assert!(diff.removed.is_empty());
    assert!(diff.added.is_empty());
}

#[test]
fn pure_toggle_does_not_request_reconciliation() {
    let mut state = ContentListState::default();
    let mut content = entry("Example");
    content.path = PathBuf::from("mods/example.jar.disabled");
    content.enabled = false;
    state.entries.push(content);
    *state.watcher_diff.lock().unwrap() = Some(super::WatcherDiff {
        toggled: vec![(
            "example".to_owned(),
            false,
            PathBuf::from("mods/example.jar.disabled"),
        )],
        removed: Vec::new(),
        added: Vec::new(),
    });

    let update = state.drain_watcher();

    assert!(!update.requires_reconcile);
    assert_eq!(update.toggles.len(), 1);
    assert_eq!(
        update.toggles[0].old_path,
        PathBuf::from("mods/example.jar")
    );
}

#[test]
fn content_watcher_replaces_a_version_without_removing_its_row() {
    let mut state = ContentListState::default();
    let mut installed = entry("Example Mod");
    installed.file_stem = "example-1.0".to_owned();
    installed.path = PathBuf::from("mods/example-1.0.jar");
    installed.description = "Cached description".to_owned();
    installed.provider_description = true;
    installed.icon_bytes = Some(vec![1, 2, 3]);
    state.entries.push(installed);
    *state.watcher_diff.lock().unwrap() = Some(super::WatcherDiff {
        toggled: Vec::new(),
        removed: vec!["example-1.0".to_owned()],
        added: Vec::new(),
    });

    state.drain_watcher();
    assert_eq!(state.entries.len(), 1);

    let mut replacement = entry("Example Mod");
    replacement.file_stem = "example-2.0".to_owned();
    replacement.path = PathBuf::from("mods/example-2.0.jar");
    *state.watcher_diff.lock().unwrap() = Some(super::WatcherDiff {
        toggled: Vec::new(),
        removed: Vec::new(),
        added: vec![replacement],
    });

    state.drain_watcher();

    assert_eq!(state.entries.len(), 1);
    assert_eq!(state.entries[0].path, PathBuf::from("mods/example-2.0.jar"));
    assert_eq!(state.entries[0].description, "Cached description");
    assert_eq!(state.entries[0].icon_bytes, Some(vec![1, 2, 3]));
}

#[test]
fn content_watcher_removes_a_file_after_the_replacement_grace_period() {
    let mut state = ContentListState::default();
    let mut installed = entry("Removed Mod");
    installed.file_stem = "removed".to_owned();
    installed.path = PathBuf::from("mods/removed.jar");
    state.entries.push(installed);
    state.pending_removals.insert(
        "removed".to_owned(),
        std::time::Instant::now() - super::REMOVAL_GRACE,
    );

    let update = state.drain_watcher();

    assert!(state.entries.is_empty());
    assert!(update.requires_reconcile);
}

#[test]
fn irrelevant_watcher_paths_do_not_emit_an_empty_diff() {
    let temp = tempfile::tempdir().unwrap();
    let known = Arc::new(Mutex::new(HashMap::new()));
    let paths = vec![temp.path().join("notes.txt")];
    assert!(diff_event_paths(temp.path(), &paths, ".jar", None, &known).is_none());
}

#[test]
fn square_columns_follow_terminal_cell_ratio() {
    assert_eq!(square_icon_columns(3, (8, 16)), 6);
    assert_eq!(square_icon_columns(3, (8, 18)), 7);
    assert_eq!(square_icon_columns(6, (8, 18)), 14);
}

#[test]
fn square_columns_handle_missing_cell_size() {
    assert_eq!(square_icon_columns(3, (0, 0)), 3);
}

#[test]
fn title_badge_is_rendered_after_a_small_gap() {
    let label_style = Style::default()
        .fg(Color::Black)
        .bg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let spans = title_suffix_spans(Some("Installed"), Style::default(), label_style);
    let text = spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<Vec<_>>()
        .concat();

    assert_eq!(text, "   Installed ");
    assert_eq!(spans[1].style, label_style);
    assert!(title_suffix_spans(None, Style::default(), label_style).is_empty());
}

#[test]
fn title_suffix_keeps_label_style_after_the_row_background_is_applied() {
    let label_style = Style::default()
        .fg(Color::Black)
        .bg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let text = Text::from(Line::from(title_suffix_spans(
        Some("downloads"),
        Style::default(),
        label_style,
    )))
    .style(Style::default().bg(Color::Black));
    let area = Rect::new(0, 0, 20, 1);
    let mut buffer = Buffer::empty(area);

    text.render(area, &mut buffer);

    let label_cell = buffer.cell((5, 0)).unwrap();
    assert_eq!(label_cell.fg, Color::Black);
    assert_eq!(label_cell.bg, Color::Cyan);
    assert!(label_cell.modifier.contains(Modifier::BOLD));
}

#[test]
fn world_modes_use_distinct_theme_roles() {
    let theme = crate::config::theme::THEME.as_ref();
    assert_eq!(
        world_game_mode_color(WorldGameMode::Survival),
        theme.success()
    );
    assert_eq!(world_game_mode_color(WorldGameMode::Creative), theme.info());
    assert_eq!(
        world_game_mode_color(WorldGameMode::Adventure),
        theme.warning()
    );
    assert_eq!(
        world_game_mode_color(WorldGameMode::Spectator),
        theme.text_dim()
    );
    assert_eq!(
        world_game_mode_color(WorldGameMode::Hardcore),
        theme.error()
    );
}

#[test]
fn descriptions_are_ellipsized_to_the_available_cell_width() {
    assert_eq!(ellipsize("short", 5), "short");
    assert_eq!(ellipsize("a longer description", 10), "a longe...");
    assert_eq!(ellipsize("narrow", 3), "...");
    assert_eq!(ellipsize("narrow", 2), "..");
    assert_eq!(ellipsize("界界界", 5), "界...");
}

#[test]
fn description_width_reserves_the_row_chrome() {
    assert_eq!(available_description_width(100, 6, true), 91);
    assert_eq!(available_description_width(100, 0, false), 98);
    assert_eq!(available_description_width(4, 6, true), 0);
}

#[test]
fn description_width_reserves_the_download_metadata() {
    assert_eq!(description_text_width(40, 14, true), 25);
    assert_eq!(description_text_width(10, 14, true), 0);
    assert_eq!(description_text_width(40, 0, true), 40);
}

#[test]
fn footer_metadata_is_right_aligned_without_a_separator() {
    let mut spans = vec![Span::raw("Description")];
    spans.extend(right_aligned_footer_spans(
        30,
        "Description",
        true,
        vec![Span::raw("1.2K downloads")],
    ));
    let line = Line::from(spans);

    assert_eq!(line.width(), 30);
    assert_eq!(line.to_string(), "Description     1.2K downloads");
}

#[test]
fn version_changes_render_as_two_labels_with_a_directional_arrow() {
    let spans = version_change_spans("1.0", "2.0");

    assert_eq!(Line::from(spans).to_string(), " 1.0   ➜   2.0 ");
}

#[test]
fn content_stream_inserts_entries_and_icons_incrementally() {
    let mut state = ContentListState::default();
    let stream = state.start_stream("remote");

    assert!(stream.send(entry("Zulu")));
    state.drain_pending();
    assert_eq!(state.entries[0].name, "Zulu");
    assert!(!state.loading);

    assert!(stream.send(entry("Alpha")));
    assert!(stream.send_icon(
        "alpha".to_owned(),
        PathBuf::from("alpha"),
        vec![1, 2, 3],
        None
    ));
    state.drain_pending();

    assert_eq!(
        state
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["Alpha", "Zulu"]
    );
    assert_eq!(
        state.entries[0].icon_bytes.as_deref(),
        Some([1, 2, 3].as_slice())
    );
}

#[test]
fn source_stream_preserves_remote_result_order() {
    let mut state = ContentListState::default();
    let stream = state.start_source_stream("remote");
    assert!(stream.send(entry("Zulu")));
    assert!(stream.send(entry("Alpha")));

    state.drain_pending();

    assert_eq!(
        state
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["Zulu", "Alpha"]
    );
}

#[test]
fn source_refresh_reconciles_without_rebuilding_unchanged_entries() {
    let mut state = ContentListState::default();
    let initial = state.start_source_stream("remote");
    let mut alpha = entry("Alpha");
    alpha.icon_bytes = Some(vec![1, 2, 3]);
    assert!(initial.upsert(alpha));
    assert!(initial.upsert(entry("Beta")));
    assert!(initial.upsert(entry("Gamma")));
    state.drain_pending();
    state.list_state.selected = Some(0);

    let refresh = state.refresh_source_stream("remote");
    let mut alpha_update = entry("Alpha");
    alpha_update.description = "Updated".to_owned();
    assert!(refresh.upsert(alpha_update));
    assert!(refresh.upsert(entry("Delta")));
    assert!(refresh.retain(HashSet::from(["alpha".to_owned(), "delta".to_owned(),])));
    state.drain_pending();

    assert_eq!(
        state
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        ["Alpha", "Delta"]
    );
    assert_eq!(state.entries[0].description, "Updated");
    assert_eq!(
        state.entries[0].icon_bytes.as_deref(),
        Some([1, 2, 3].as_slice())
    );
    assert_eq!(state.list_state.selected, Some(0));
    assert!(!state.loading);
}

#[test]
fn discovery_preview_moves_rows_before_the_final_order_arrives() {
    let mut state = ContentListState::default();
    let initial = state.start_source_stream("remote");
    initial.upsert(entry("Alpha"));
    initial.upsert(entry("Beta"));
    state.drain_pending();
    state.list_state.selected = Some(0);
    let refresh = state.refresh_source_stream("remote");
    refresh.preview(entry("Beta"));
    refresh.preview(entry("Gamma"));
    state.drain_pending();
    let names = |state: &ContentListState| {
        state
            .entries
            .iter()
            .map(|entry| entry.name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&state), ["Beta", "Gamma", "Alpha"]);
    assert_eq!(state.list_state.selected, Some(0));
    assert_eq!(state.selected_entry().unwrap().name, "Beta");
    refresh.upsert(entry("Delta"));
    refresh.order(vec![
        "beta".to_owned(),
        "delta".to_owned(),
        "gamma".to_owned(),
    ]);
    state.drain_pending();
    assert_eq!(names(&state), ["Beta", "Delta", "Gamma"]);
    assert_eq!(state.list_state.selected, Some(0));
    refresh.append_preview();
    refresh.preview(entry("Epsilon"));
    state.drain_pending();
    assert_eq!(names(&state), ["Beta", "Delta", "Gamma", "Epsilon"]);
}

#[test]
fn discovery_stream_shows_each_row_after_its_icon_is_ready() {
    let mut state = ContentListState::default();
    let stream = state.start_source_stream("discovery");
    state.show_source_rows_progressively();
    let mut alpha = entry("Alpha");
    alpha.provider_icon = true;
    alpha.icon_bytes = Some(vec![1]);
    stream.upsert(alpha);
    stream.upsert(entry("Beta"));
    state.drain_pending();
    assert_eq!(
        state
            .filtered_indices()
            .iter()
            .map(|&i| state.entries[i].name.as_str())
            .collect::<Vec<_>>(),
        ["Beta"]
    );

    state
        .pending_images
        .lock()
        .unwrap()
        .push(PendingContentImage {
            file_stem: "alpha".to_owned(),
            path: state.entries[0].path.clone(),
            source: None,
            icon_bytes: vec![1],
            icon_lines: crate::instance::content::fallback_icon(),
            image: None,
        });
    state.drain_image_loads(&ratatui_image::picker::Picker::halfblocks());
    assert_eq!(state.filtered_indices().len(), 2);
}

#[test]
fn stale_decoded_icon_does_not_replace_a_new_source_or_new_bytes() {
    let mut state = ContentListState {
        stream_order: ContentStreamOrder::Source,
        ..Default::default()
    };
    let mut project = entry("Alpha");
    project.provider_project = Some(crate::instance::ProviderProject {
        provider: "modrinth".to_owned(),
        project_id: "123".to_owned(),
        version_id: String::new(),
    });
    project.icon_bytes = Some(vec![1]);
    state.entries.push(project.clone());
    let stale = PendingContentImage {
        file_stem: project.file_stem.clone(),
        path: project.path.clone(),
        source: project.provider_project.clone(),
        icon_bytes: vec![1],
        icon_lines: crate::instance::content::fallback_icon(),
        image: None,
    };
    state.pending_entry_images.insert(project.file_stem.clone());
    state.entries[0].provider_project.as_mut().unwrap().provider = "curseforge".to_owned();
    state.pending_images.lock().unwrap().push(stale);
    state.drain_image_loads(&ratatui_image::picker::Picker::halfblocks());
    assert!(state.pending_entry_images.contains(&project.file_stem));

    state.entries[0].provider_project = project.provider_project;
    state.entries[0].icon_bytes = Some(vec![2]);
    state
        .pending_images
        .lock()
        .unwrap()
        .push(PendingContentImage {
            file_stem: project.file_stem.clone(),
            path: project.path,
            source: state.entries[0].provider_project.clone(),
            icon_bytes: vec![1],
            icon_lines: crate::instance::content::fallback_icon(),
            image: None,
        });
    state.drain_image_loads(&ratatui_image::picker::Picker::halfblocks());
    assert!(state.pending_entry_images.contains(&project.file_stem));
}

#[test]
fn current_version_keeps_unidentified_installed_files_visible() {
    let mut state = ContentListState::default();
    state.entries.push(entry("Local file"));
    state.set_installed_options(
        &crate::tui::widgets::content::discovery::DiscoveryFilters::default(),
        5,
        false,
        "1.21.1",
        false,
    );
    assert_eq!(state.filtered_indices(), [0]);
}

#[test]
fn stale_provider_icon_cannot_replace_a_new_provider_row() {
    let mut state = ContentListState::default();
    let stream = state.start_source_stream("discovery");
    let mut project = entry("Alpha");
    project.provider_project = Some(crate::instance::ProviderProject {
        provider: "modrinth".to_owned(),
        project_id: "123".to_owned(),
        version_id: String::new(),
    });
    project.provider_icon = true;
    stream.upsert(project);
    stream.send_icon(
        "alpha".to_owned(),
        PathBuf::from("alpha"),
        vec![1],
        Some(("curseforge".to_owned(), "456".to_owned())),
    );
    stream.send_icon_unavailable(
        "alpha".to_owned(),
        PathBuf::from("alpha"),
        Some(("curseforge".to_owned(), "456".to_owned())),
    );
    state.drain_pending();
    assert!(state.entries[0].icon_bytes.is_none());
    assert!(state.entries[0].provider_icon);
    assert!(state.has_pending_icons());
    stream.send_icon(
        "alpha".to_owned(),
        PathBuf::from("alpha"),
        vec![2],
        Some(("modrinth".to_owned(), "123".to_owned())),
    );
    state.drain_pending();
    assert_eq!(state.entries[0].icon_bytes.as_deref(), Some([2].as_slice()));
}

#[test]
fn source_refresh_reuses_a_decoded_icon_only_for_the_same_provider() {
    let mut state = ContentListState::default();
    let stream = state.start_source_stream("discovery");
    let mut project = entry("Alpha");
    project.icon_bytes = Some(vec![1]);
    project.provider_icon = true;
    project.provider_project = Some(crate::instance::ProviderProject {
        provider: "modrinth".to_owned(),
        project_id: "123".to_owned(),
        version_id: String::new(),
    });
    stream.upsert(project.clone());
    state.drain_pending();
    let mut rendered_icon = crate::instance::content::fallback_icon();
    rendered_icon[0][0].symbol = 'X';
    state.entries[0].icon_lines = Some(rendered_icon);
    state.pending_entry_images.remove("alpha");
    state.requested_images.insert("alpha".to_owned());

    let refresh = state.refresh_source_stream("discovery");
    refresh.upsert(project.clone());
    state.drain_pending();
    assert_eq!(
        state.entries[0].icon_lines.as_ref().unwrap()[0][0].symbol,
        'X'
    );
    assert!(!state.has_pending_icons());

    project.provider_project.as_mut().unwrap().provider = "curseforge".to_owned();
    refresh.upsert(project);
    state.drain_pending();
    assert!(state.has_pending_icons());
    assert!(!state.requested_images.contains("alpha"));
}

#[test]
fn discovery_icon_decode_survives_installed_version_binding() {
    let mut state = ContentListState::default();
    let stream = state.start_source_stream("discovery");
    let mut project = entry("Alpha");
    project.icon_bytes = Some(vec![1]);
    project.provider_project = Some(crate::instance::ProviderProject {
        provider: "modrinth".to_owned(),
        project_id: "123".to_owned(),
        version_id: String::new(),
    });
    stream.upsert(project.clone());
    state.drain_pending();
    state.entries[0]
        .provider_project
        .as_mut()
        .unwrap()
        .version_id = "installed".to_owned();
    let mut icon_lines = crate::instance::content::fallback_icon();
    icon_lines[0][0].symbol = 'X';
    state
        .pending_images
        .lock()
        .unwrap()
        .push(PendingContentImage {
            file_stem: project.file_stem,
            path: project.path,
            source: project.provider_project,
            icon_bytes: vec![1],
            icon_lines,
            image: None,
        });
    state.drain_image_loads(&ratatui_image::picker::Picker::halfblocks());
    assert!(!state.has_pending_icons());
    assert_eq!(
        state.entries[0].icon_lines.as_ref().unwrap()[0][0].symbol,
        'X'
    );
}

#[test]
fn provider_icons_are_requested_only_for_visible_missing_icons() {
    let mut state = ContentListState::default();
    let mut visible = entry("Visible");
    visible.icon_lines = Some(crate::instance::content::fallback_icon());
    visible.provider_project = Some(crate::instance::ProviderProject {
        provider: "modrinth".to_owned(),
        project_id: "visible-project".to_owned(),
        version_id: "version".to_owned(),
    });
    let mut offscreen = entry("Offscreen");
    offscreen.icon_lines = Some(crate::instance::content::fallback_icon());
    offscreen.provider_project = Some(crate::instance::ProviderProject {
        provider: "modrinth".to_owned(),
        project_id: "offscreen-project".to_owned(),
        version_id: "version".to_owned(),
    });
    state.entries = vec![visible, offscreen];
    state.rebuild_display_metadata();

    let projects = state.visible_provider_projects(&[0, 1], 3);

    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].project_id, "visible-project");
}

#[test]
fn complete_local_pack_metadata_does_not_request_provider_fallbacks() {
    let mut state = ContentListState::default();
    let mut visible = entry("Visible");
    visible.description = "Local description".to_owned();
    visible.icon_bytes = Some(vec![1, 2, 3]);
    visible.icon_lines = Some(crate::instance::content::fallback_icon());
    visible.provider_project = Some(crate::instance::ProviderProject {
        provider: "modrinth".to_owned(),
        project_id: "visible-project".to_owned(),
        version_id: "version".to_owned(),
    });
    state.entries = vec![visible];
    state.rebuild_display_metadata();

    assert!(state.visible_provider_projects(&[0], 3).is_empty());
}

#[tokio::test]
async fn streamed_entries_wait_for_their_rendered_icon() {
    let mut state = ContentListState::default();
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(1, 1)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let mut with_icon = entry("With icon");
    with_icon.icon_bytes = Some(png.into_inner());
    let stream = state.start_stream("local");

    assert!(stream.send(with_icon));
    state.drain_pending();
    assert!(state.filtered_indices().is_empty());

    let picker = ratatui_image::picker::Picker::halfblocks();
    state.request_image_loads(&picker);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            state.drain_image_loads(&picker);
            if !state.filtered_indices().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("icon render completed");

    assert_eq!(state.filtered_indices(), vec![0]);
}

#[test]
fn streamed_entries_without_icons_are_visible_immediately() {
    let mut state = ContentListState::default();
    let stream = state.start_stream("local");

    assert!(stream.send(entry("Without icon")));
    state.drain_pending();

    assert_eq!(state.filtered_indices(), vec![0]);
}

#[tokio::test]
async fn installed_icon_decode_survives_manifest_binding_and_cache_restore() {
    let mut state = ContentListState::default();
    state.set_installed_options(
        &crate::tui::widgets::content::discovery::DiscoveryFilters::default(),
        5,
        false,
        "26.2",
        false,
    );
    let stream = state.start_stream("main");
    let mut mod_entry = entry("Fabric API");
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(1, 1)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    mod_entry.icon_bytes = Some(png.into_inner());
    mod_entry.icon_lines =
        crate::instance::content::make_icon_pixels(mod_entry.icon_bytes.as_ref().unwrap(), 6, 3);
    stream.send(mod_entry);
    state.drain_pending();
    assert!(state.filtered_indices().is_empty());
    let picker = ratatui_image::picker::Picker::halfblocks();
    state.request_image_loads(&picker);
    state.entries[0].provider_project = Some(crate::instance::ProviderProject {
        provider: "modrinth".to_owned(),
        project_id: "fabric-api".to_owned(),
        version_id: "version".to_owned(),
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while state.filtered_indices().is_empty() {
            state.drain_image_loads(&picker);
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("installed icon decoded after manifest binding");

    let directory = tempfile::tempdir().unwrap();
    state.start_load(
        directory.path(),
        "other",
        crate::instance::content::mods::scan_one_mod,
        "jar",
    );
    state.start_load(
        directory.path(),
        "main",
        crate::instance::content::mods::scan_one_mod,
        "jar",
    );
    assert!(state.filtered_indices().is_empty());
    state.request_image_loads(&picker);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while state.filtered_indices().is_empty() {
            state.drain_image_loads(&picker);
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("cached installed icon decoded");
}

#[test]
fn rendering_visible_entries_restores_the_first_selection() {
    let mut state = ContentListState::default();
    state.entries.push(entry("First"));
    state.rebuild_display_metadata();
    let picker = ratatui_image::picker::Picker::halfblocks();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 5)).unwrap();

    terminal
        .draw(|frame| {
            super::render(
                frame,
                frame.area(),
                &mut state,
                true,
                "Loading...",
                "Empty",
                &picker,
                false,
                false,
            );
        })
        .unwrap();

    assert_eq!(state.list_state.selected, Some(0));
}

#[test]
fn incompatible_installed_version_renders_red_footer() {
    use crate::net::modrinth::{VersionInfo, VersionType};

    fn installed_entry(
        name: &str,
        project_id: &str,
        version_id: &str,
        footer: &str,
    ) -> ContentEntry {
        let mut item = entry(name);
        item.footer_label = Some(footer.to_owned());
        item.provider_project = Some(crate::instance::ProviderProject {
            provider: "modrinth".to_owned(),
            project_id: project_id.to_owned(),
            version_id: version_id.to_owned(),
        });
        item
    }
    fn version_metadata(game_versions: &[&str]) -> VersionInfo {
        VersionInfo {
            id: "version".to_owned(),
            project_id: "project".to_owned(),
            name: "Version".to_owned(),
            version_number: "1.0".to_owned(),
            game_versions: game_versions
                .iter()
                .map(|version| (*version).to_owned())
                .collect(),
            loaders: vec!["fabric".to_owned()],
            version_type: VersionType::Release,
            dependencies: Vec::new(),
            date_published: String::new(),
            files: Vec::new(),
        }
    }

    let mut state = ContentListState {
        entries: vec![
            installed_entry("Good", "good", "good-v1", "1.0.0+mc1.21.1"),
            installed_entry("Bad", "bad", "bad-v1", "0.9.0+mc1.20.1"),
        ],
        ..ContentListState::default()
    };
    state.local_game_version = "1.21.1".to_owned();
    state.version_metadata.insert(
        ("modrinth".to_owned(), "good-v1".to_owned()),
        version_metadata(&["1.21.1"]),
    );
    state.version_metadata.insert(
        ("modrinth".to_owned(), "bad-v1".to_owned()),
        version_metadata(&["1.20.1"]),
    );
    state.rebuild_display_metadata();
    let picker = ratatui_image::picker::Picker::halfblocks();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 8)).unwrap();
    terminal
        .draw(|frame| {
            super::render(
                frame,
                frame.area(),
                &mut state,
                true,
                "Loading...",
                "Empty",
                &picker,
                false,
                false,
            );
        })
        .unwrap();

    let theme = crate::config::theme::THEME.as_ref();
    let buffer = terminal.backend().buffer().clone();
    let row_text = |y: u16| {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol().to_owned())
            .collect::<String>()
    };
    let footer_color = |footer: &str| {
        (0..buffer.area.height).find_map(|y| {
            let row = row_text(y);
            row.find(footer).map(|start| {
                let start = u16::try_from(start).unwrap();
                buffer[(start, y)].fg
            })
        })
    };
    assert_eq!(footer_color("1.0.0+mc1.21.1"), Some(theme.text()));
    assert_eq!(footer_color("0.9.0+mc1.20.1"), Some(theme.error()));
}

#[test]
fn multiline_rendering_uses_the_space_beside_large_icons() {
    let mut world = entry("World");
    world.world_details = Some(WorldDetails {
        game_mode: Some(WorldGameMode::Survival),
        last_played: None,
        minecraft_version: Some("1.21.1".to_owned()),
        size: Some("2.0 MB".to_owned()),
        datapacks: Vec::new(),
    });
    world.icon_lines = Some(crate::instance::content::fallback_icon_large());
    let mut state = ContentListState::default();
    state.entries.push(world);
    state.rebuild_display_metadata();
    let picker = ratatui_image::picker::Picker::halfblocks();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(70, 6)).unwrap();

    terminal
        .draw(|frame| {
            super::render(
                frame,
                frame.area(),
                &mut state,
                true,
                "Loading...",
                "Empty",
                &picker,
                false,
                true,
            );
        })
        .unwrap();

    insta::assert_snapshot!(terminal.backend().to_string());
}

#[test]
fn pager_tracks_viewport_pages_and_jumps_to_their_first_item() {
    assert_eq!(
        super::pager_pages(0, 12),
        vec![Some(0), Some(1), Some(2), Some(3)]
    );
    assert_eq!(
        super::pager_pages(4, 12),
        vec![Some(0), None, Some(3), Some(4), Some(5)]
    );
    assert_eq!(
        super::pager_pages(11, 12),
        vec![Some(0), None, Some(9), Some(10), Some(11)]
    );

    let area = Rect::new(0, 0, 40, 13);
    let (list_area, pager) = super::pagination_layout(area, 5);
    assert_eq!(list_area, area);
    assert_eq!(pager.map(|(_, page_size)| page_size), Some(4));
    assert!(super::pagination_layout(area, 4).1.is_none());

    let mut state = ContentListState {
        entries: (1..=10)
            .map(|number| {
                let mut item = entry(&format!("Project {number}"));
                item.icon_lines = Some(crate::instance::content::fallback_icon());
                item
            })
            .collect(),
        ..ContentListState::default()
    };
    state.rebuild_display_metadata();
    let picker = ratatui_image::picker::Picker::halfblocks();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(40, 13)).unwrap();

    terminal
        .draw(|frame| {
            super::render(
                frame,
                frame.area(),
                &mut state,
                true,
                "Loading...",
                "Empty",
                &picker,
                true,
                false,
            );
        })
        .unwrap();

    let pagination = state.pagination.as_ref().expect("pager");
    assert_eq!(pagination.page_size, 4);
    assert_eq!(pagination.page_count, 3);
    let page_two = pagination
        .hits
        .iter()
        .find(|(_, page)| *page == 1)
        .map(|(area, _)| (area.x, area.y))
        .expect("page two hit target");
    assert!(state.click_page(page_two.0, page_two.1));
    assert_eq!(state.list_state.selected, Some(4));
    assert!(state.next_page());
    assert_eq!(state.list_state.selected, Some(8));
    assert!(state.previous_page());
    assert_eq!(state.list_state.selected, Some(4));
}

#[test]
fn manifest_metadata_keeps_an_embedded_icon_renderer() {
    let minecraft_dir = PathBuf::from("instance/minecraft");
    let mut state = ContentListState::default();
    let mut installed = entry("Installed");
    installed.path = minecraft_dir.join("mods/installed.jar");
    installed.icon_bytes = Some(vec![1, 2, 3]);
    state.entries.push(installed);
    let picker = ratatui_image::picker::Picker::halfblocks();
    state.image_protocols.insert(
        "installed".to_owned(),
        picker.new_resize_protocol(image::DynamicImage::new_rgba8(1, 1)),
    );
    let mut manifest = crate::instance::ContentManifest::default();
    manifest.upsert(crate::instance::ContentFileRecord {
        relative_path: PathBuf::from("mods/installed.jar"),
        kind: crate::instance::ContentKind::Mod,
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
                version_id: "version".to_owned(),
            },
        },
        provider_aliases: Vec::new(),
        provider_checks: Vec::new(),
        required_dependencies: Vec::new(),
        automatic_dependency: false,
        cleanup_eligible: false,
    });

    state.apply_manifest(&manifest, &minecraft_dir, crate::instance::ContentKind::Mod);

    assert!(state.image_protocols.contains_key("installed"));
}

#[test]
fn provider_metadata_fills_a_missing_installed_description() {
    let mut state = ContentListState::default();
    let mut installed = entry("Shader");
    installed.provider_project = Some(crate::instance::ProviderProject {
        provider: "modrinth".to_owned(),
        project_id: "shader-project".to_owned(),
        version_id: "version".to_owned(),
    });
    state.entries.push(installed);
    state
        .pending_provider_icons
        .lock()
        .unwrap()
        .push(super::PendingProviderIcon {
            provider: "modrinth".to_owned(),
            project_id: "shader-project".to_owned(),
            bytes: Vec::new(),
            description: "A cached shader description".to_owned(),
            project: crate::net::modrinth::ProjectInfo::default(),
            version_id: "version".to_owned(),
            version: None,
        });

    assert!(state.drain_provider_icons());
    assert_eq!(state.entries[0].description, "A cached shader description");
    assert!(state.entries[0].provider_description);
    assert_eq!(state.entries[0].title_suffix, None);
}

#[tokio::test]
async fn provider_metadata_loads_from_cache_without_network() {
    let temp = tempfile::tempdir().unwrap();
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgba8(1, 1)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let png = png.into_inner();
    let icon_path = crate::storage::MetadataPaths::new(temp.path())
        .provider_icons("modrinth")
        .join("cached-project.img");
    std::fs::create_dir_all(icon_path.parent().unwrap()).unwrap();
    std::fs::write(&icon_path, &png).unwrap();
    let project_path = crate::storage::MetadataPaths::new(temp.path())
        .provider_projects("modrinth")
        .join("cached-project.json");
    std::fs::create_dir_all(project_path.parent().unwrap()).unwrap();
    std::fs::write(
        project_path,
        serde_json::to_vec(&crate::net::modrinth::ProjectInfo {
            id: "cached-project".to_owned(),
            slug: "cached-project".to_owned(),
            title: "Cached project".to_owned(),
            description: "Cached description".to_owned(),
            body: String::new(),
            icon_url: None,
            categories: Vec::new(),
            additional_categories: Vec::new(),
            project_type: "mod".to_owned(),
            loaders: Vec::new(),
            ..crate::net::modrinth::ProjectInfo::default()
        })
        .unwrap(),
    )
    .unwrap();

    let installed = crate::instance::ProviderProject {
        provider: "modrinth".to_owned(),
        project_id: "cached-project".to_owned(),
        version_id: "cached-version".to_owned(),
    };
    let (bytes, project) = load_provider_metadata(
        &crate::net::HttpClient::new(),
        temp.path(),
        &installed,
        false,
    )
    .await
    .unwrap();

    assert_eq!(bytes, png);
    assert_eq!(project.description, "Cached description");
}
