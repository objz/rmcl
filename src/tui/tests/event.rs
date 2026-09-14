// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;
use crate::tui::tests::harness::UiHarness;

#[test]
fn edited_instance_config_reloads_into_the_ui() {
    let mut ui = UiHarness::new();
    ui.add_instance("Edited");
    let mut config = ui.app.instances_state.selected_instance().unwrap().clone();
    config.memory_max = Some("8G".to_owned());
    ui.app.instance_manager.save(&config).unwrap();

    ui.app
        .reload_edited_config(&ui.instance_path("Edited").join("instance.json"));

    assert_eq!(
        ui.app
            .instances_state
            .selected_instance()
            .unwrap()
            .memory_max
            .as_deref(),
        Some("8G")
    );
}

#[test]
fn runtime_update_merges_only_confirmed_fields_into_latest_config() {
    let mut ui = UiHarness::new();
    ui.add_instance("Merge Runtime");
    let previous = ui.app.instances_state.selected_instance().unwrap().clone();
    let mut updated = previous.clone();
    updated.game_version = "1.21.2".to_owned();
    updated.memory_max = Some("8G".to_owned());
    let mut current = previous.clone();
    current.config_sync_profile = Some("shared".to_owned());
    current.preferred_account = Some("newer-account".to_owned());

    let merged = merge_instance_settings(&previous, &updated, current);

    assert_eq!(merged.game_version, "1.21.2");
    assert_eq!(merged.memory_max.as_deref(), Some("8G"));
    assert_eq!(merged.config_sync_profile.as_deref(), Some("shared"));
    assert_eq!(merged.preferred_account.as_deref(), Some("newer-account"));
}

#[test]
fn completed_background_instance_is_drained_into_the_ui() {
    let mut ui = UiHarness::new();
    ui.add_instance("Existing");
    ui.add_instance("Pending");
    let mut config = ui.app.instances_state.instances.pop().unwrap();
    ui.app.mods_state.loaded_for = Some("Pending".to_owned());
    ui.app.reconciliation_for = Some(("Pending".to_owned(), config.created));
    ui.app.content_manifest = Some((
        "Pending".to_owned(),
        crate::instance::ContentManifest::default(),
    ));
    config.created += chrono::TimeDelta::seconds(1);
    ui.app.instances_state.list_state.selected = Some(0);
    PENDING_INSTANCES.lock().unwrap().push(config);

    ui.app.drain_pending_instances();

    assert_eq!(
        ui.app.instances_state.selected_instance().unwrap().name,
        "Pending"
    );
    assert!(PENDING_INSTANCES.lock().unwrap().is_empty());
    assert!(ui.app.reconciliation_for.is_none());
    assert!(ui.app.content_manifest.is_none());
    assert!(ui.app.mods_state.loaded_for.is_none());

    ui.draw();
    assert_eq!(ui.app.mods_state.loaded_for.as_deref(), Some("Pending"));
}

#[test]
fn unrelated_background_result_does_not_complete_runtime_update() {
    let mut ui = UiHarness::new();
    ui.add_instance("Pending Runtime");
    ui.app
        .pending_instance_settings_updates
        .insert("Pending Runtime".to_owned());
    let config = ui.app.instances_state.selected_instance().unwrap().clone();
    PENDING_INSTANCES.lock().unwrap().push(config);

    ui.app.drain_pending_instances();

    assert!(
        ui.app
            .pending_instance_settings_updates
            .contains("Pending Runtime")
    );
}

#[test]
fn pending_runtime_update_blocks_instance_rename() {
    let mut ui = UiHarness::new();
    ui.add_instance("Pending Rename");
    ui.app
        .pending_instance_settings_updates
        .insert("Pending Rename".to_owned());

    ui.key(crossterm::event::KeyCode::Char('r'));

    assert!(ui.app.instances_state.renaming.is_none());
    assert_eq!(
        crate::feedback::errors::peek_error().map(|error| error.message),
        Some(RUNTIME_UPDATE_PENDING_MESSAGE.to_owned())
    );
}

#[test]
fn runtime_settings_update_keeps_the_editor_open_and_handles_results() {
    let mut ui = UiHarness::new();
    ui.add_instance("Runtime Settings");
    ui.key(crossterm::event::KeyCode::Char('E'));
    let state = ui.app.instance_settings.as_mut().unwrap();
    state.draft.game_version = "1.21.2".to_owned();
    state.mark_runtime_update_pending();
    ui.app
        .pending_instance_settings_updates
        .insert("Runtime Settings".to_owned());

    ui.key(crossterm::event::KeyCode::Esc);
    assert!(ui.app.instance_settings.is_none());
    ui.key(crossterm::event::KeyCode::Char('E'));
    assert!(
        ui.app
            .instance_settings
            .as_ref()
            .unwrap()
            .runtime_update_pending_for("Runtime Settings")
    );

    let mut updated = ui.app.instances_state.selected_instance().unwrap().clone();
    updated.game_version = "1.21.2".to_owned();
    ui.app.instance_manager.save(&updated).unwrap();
    COMPLETED_INSTANCE_SETTINGS_UPDATES
        .lock()
        .unwrap()
        .push(updated);
    ui.app.drain_completed_instance_settings_updates();

    let state = ui.app.instance_settings.as_ref().unwrap();
    assert_eq!(state.draft.game_version, "1.21.2");
    assert!(!state.runtime_update_pending_for("Runtime Settings"));
    assert!(ui.app.pending_instance_settings_updates.is_empty());

    let state = ui.app.instance_settings.as_mut().unwrap();
    state.draft.game_version = "1.21.3".to_owned();
    state.mark_runtime_update_pending();
    ui.app
        .pending_instance_settings_updates
        .insert("Runtime Settings".to_owned());
    FAILED_INSTANCE_SETTINGS_UPDATES
        .lock()
        .unwrap()
        .push("Runtime Settings".to_owned());
    ui.app.drain_failed_instance_settings_updates();

    let state = ui.app.instance_settings.as_ref().unwrap();
    assert_eq!(state.draft.game_version, "1.21.2");
    assert!(!state.runtime_update_pending_for("Runtime Settings"));
    assert!(ui.app.pending_instance_settings_updates.is_empty());
}

#[test]
fn structural_settings_update_repairs_runtime_before_persisting() {
    use sha1::{Digest, Sha1};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn sha1(bytes: &[u8]) -> String {
        format!("{:x}", Sha1::digest(bytes))
    }

    let _guard = crate::tests::TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let server = MockServer::start().await;
        let client_jar = b"client";
        let library_jar = b"library";
        for (endpoint, body) in [
            ("/client.jar", client_jar.as_slice()),
            ("/library.jar", library_jar.as_slice()),
        ] {
            Mock::given(method("GET"))
                .and(path(endpoint))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
                .expect(1)
                .mount(&server)
                .await;
        }
        Mock::given(method("GET"))
            .and(path("/assets.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "objects": {}
            })))
            .expect(1)
            .mount(&server)
            .await;

        let temp = tempfile::tempdir().unwrap();
        let instances_dir = temp.path().join("instances");
        let meta_dir = temp.path().join("meta");
        let instance_dir = instances_dir.join("Migrating");
        std::fs::create_dir_all(&instance_dir).unwrap();
        let manager = InstanceManager::new(&instances_dir, &meta_dir);
        let mut previous = crate::instance::InstanceConfig {
            name: "Migrating".to_owned(),
            game_version: "1.20.1".to_owned(),
            loader: crate::instance::ModLoader::Vanilla,
            loader_version: None,
            created: chrono::Utc::now(),
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
        };
        manager.save(&previous).unwrap();

        let metadata = crate::storage::MetadataPaths::new(&meta_dir);
        let version_dir = metadata.versions().join("1.21.2");
        std::fs::create_dir_all(&version_dir).unwrap();
        std::fs::write(
            version_dir.join("meta.json"),
            serde_json::to_vec(&serde_json::json!({
                "id": "1.21.2",
                "mainClass": "net.minecraft.client.main.Main",
                "arguments": { "game": [], "jvm": [] },
                "assetIndex": {
                    "id": "1.21.2",
                    "url": format!("{}/assets.json", server.uri()),
                    "sha1": "unused"
                },
                "downloads": {
                    "client": {
                        "url": format!("{}/client.jar", server.uri()),
                        "sha1": sha1(client_jar),
                        "size": client_jar.len()
                    }
                },
                "libraries": [{
                    "name": "example:test:1",
                    "downloads": {
                        "artifact": {
                            "url": format!("{}/library.jar", server.uri()),
                            "path": "example/test/1/test-1.jar",
                            "sha1": sha1(library_jar),
                            "size": library_jar.len()
                        }
                    }
                }],
                "javaVersion": { "majorVersion": 21 }
            }))
            .unwrap(),
        )
        .unwrap();

        let mut updated = previous.clone();
        updated.game_version = "1.21.2".to_owned();
        updated.memory_max = Some("4G".to_owned());
        let applied = apply_instance_settings_update(&manager, &previous, updated)
            .await
            .unwrap();

        assert_eq!(applied.game_version, "1.21.2");
        previous = manager.load_one("Migrating").unwrap();
        assert_eq!(previous.game_version, "1.21.2");
        assert_eq!(previous.memory_max.as_deref(), Some("4G"));
        assert_eq!(
            std::fs::read(version_dir.join("1.21.2.jar")).unwrap(),
            client_jar
        );
        assert_eq!(
            std::fs::read(metadata.libraries().join("example/test/1/test-1.jar")).unwrap(),
            library_jar
        );
        assert!(metadata.assets().join("indexes/1.21.2.json").exists());
        crate::feedback::progress::clear();
    });
}

#[test]
fn failed_structural_settings_update_keeps_previous_config() {
    let _guard = crate::tests::TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let temp = tempfile::tempdir().unwrap();
        let instances_dir = temp.path().join("instances");
        let meta_dir = temp.path().join("meta");
        std::fs::create_dir_all(instances_dir.join("Stable")).unwrap();
        let manager = InstanceManager::new(&instances_dir, &meta_dir);
        let previous = crate::instance::InstanceConfig {
            name: "Stable".to_owned(),
            game_version: "1.20.1".to_owned(),
            loader: crate::instance::ModLoader::Vanilla,
            loader_version: None,
            created: chrono::Utc::now(),
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
        };
        manager.save(&previous).unwrap();

        let metadata = crate::storage::MetadataPaths::new(&meta_dir);
        let version_dir = metadata.versions().join("missing-runtime");
        std::fs::create_dir_all(&version_dir).unwrap();
        std::fs::write(version_dir.join("meta.json"), b"not json").unwrap();
        let mut updated = previous.clone();
        updated.game_version = "missing-runtime".to_owned();

        assert!(
            apply_instance_settings_update(&manager, &previous, updated)
                .await
                .is_err()
        );
        assert_eq!(manager.load_one("Stable").unwrap().game_version, "1.20.1");
        crate::feedback::progress::clear();
    });
}

#[test]
fn editor_kind_is_detected_from_the_executable_name() {
    assert!(editor_runs_in_terminal("/usr/bin/nvim"));
    assert!(editor_runs_in_terminal("nano"));
    assert!(!editor_runs_in_terminal("/usr/bin/code"));
}

#[test]
fn overlay_count_tracks_independent_popup_layers() {
    let mut ui = UiHarness::new();
    assert_eq!(ui.app.overlay_count(), 0);

    ui.app.instances_state.show_import_popup = true;
    ui.app.account_state.add_mode = widgets::account::AddMode::ChooseType;
    assert_eq!(ui.app.overlay_count(), 2);

    ui.app.account_state.add_mode = widgets::account::AddMode::None;
    assert_eq!(ui.app.overlay_count(), 1);
}

#[test]
fn terminal_image_markers_exclude_normal_text_and_toggle() {
    use std::num::NonZeroU16;

    let mut buffer = ratatui::buffer::Buffer::empty(ratatui::layout::Rect::new(4, 7, 3, 1));
    buffer[(4, 7)]
        .set_symbol("\x1b_Gimage\x1b\\")
        .set_diff_option(ratatui::buffer::CellDiffOption::ForcedWidth(
            NonZeroU16::new(1).unwrap(),
        ));
    buffer[(5, 7)].set_symbol("text").set_diff_option(
        ratatui::buffer::CellDiffOption::ForcedWidth(NonZeroU16::new(1).unwrap()),
    );
    buffer[(6, 7)].set_symbol("\x1b[0m");

    mark_terminal_images(&mut buffer, false);
    assert_eq!(buffer[(4, 7)].symbol(), "\x1b_Gimage\x1b\\\u{200b}");
    assert_eq!(buffer[(5, 7)].symbol(), "text");
    assert_eq!(buffer[(6, 7)].symbol(), "\x1b[0m");

    buffer[(4, 7)].set_symbol("\x1b_Gimage\x1b\\");
    mark_terminal_images(&mut buffer, true);
    assert_eq!(buffer[(4, 7)].symbol(), "\x1b_Gimage\x1b\\\u{200b}\u{200b}");
}

#[test]
fn terminal_image_cells_change_when_an_overlay_opens_or_closes() {
    assert!(terminal_image_cells_changed(
        &[true, false, false],
        &[true, true, false]
    ));
    assert!(terminal_image_cells_changed(
        &[true, true, false],
        &[true, false, false]
    ));
    assert!(!terminal_image_cells_changed(&[], &[true]));
    assert!(!terminal_image_cells_changed(
        &[true, false, true],
        &[true, false, true],
    ));
}
