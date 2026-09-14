// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;
use crate::instance::models::ModLoader;
use tempfile::TempDir;

// tmp owns the temp directory; its Drop impl cleans up everything when
// the test ends. the returned InstanceManager points at tmp.path() so
// tests can join("name") off of either to refer to the same locations.
fn test_manager() -> (InstanceManager, TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let meta = tmp.path().join("meta");
    std::fs::create_dir_all(&meta).unwrap();
    (InstanceManager::new(tmp.path().to_path_buf(), meta), tmp)
}

fn dummy_config(name: &str) -> InstanceConfig {
    InstanceConfig {
        name: name.to_string(),
        game_version: "1.20.1".to_string(),
        loader: ModLoader::Vanilla,
        loader_version: None,
        created: chrono::Utc::now(),
        last_played: None,
        java_path: None,
        memory_max: None,
        memory_min: None,
        jvm_args: vec![],
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

#[test]
fn validate_name_accepts_safe_names() {
    assert!(validate_name("my-instance").is_ok());
    assert!(validate_name("test_world").is_ok());
}

#[test]
fn validate_name_rejects_empty_traversal_and_hidden() {
    assert!(validate_name("").is_err());
    assert!(validate_name("path/traversal").is_err());
    assert!(validate_name(".hidden").is_err());
    assert!(validate_name("line\nbreak").is_err());
}

#[test]
fn delete_missing_instance_returns_not_found() {
    let (manager, _tmp) = test_manager();
    let result = manager.delete("ghost-instance");
    assert!(matches!(result, Err(InstanceError::NotFound(_))));
}

#[test]
fn delete_rejects_path_traversal() {
    let tmp = tempfile::tempdir().unwrap();
    let instances = tmp.path().join("instances");
    let meta = tmp.path().join("meta");
    std::fs::create_dir_all(&instances).unwrap();
    std::fs::create_dir_all(&meta).unwrap();
    let manager = InstanceManager::new(instances, meta);
    let outside = tmp.path().join("outside-instance");
    std::fs::create_dir_all(&outside).unwrap();

    let result = manager.delete("../outside-instance");

    assert!(matches!(result, Err(InstanceError::InvalidName(_))));
    assert!(outside.exists());
}

#[test]
fn save_then_load_all_round_trips_config() {
    let (manager, tmp) = test_manager();
    std::fs::create_dir_all(tmp.path().join("test-save")).unwrap();
    manager.save(&dummy_config("test-save")).expect("save");

    let all = manager.load_all();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].name, "test-save");
    assert_eq!(all[0].game_version, "1.20.1");
}

#[test]
fn load_all_accepts_numeric_memory() {
    let (manager, tmp) = test_manager();
    let instance_dir = tmp.path().join("test-memory");
    std::fs::create_dir_all(&instance_dir).unwrap();
    std::fs::write(
        instance_dir.join("instance.json"),
        r#"{
  "name": "test-memory",
  "game_version": "1.7.10",
  "loader": "forge",
  "loader_version": "10.13.4.1614",
  "created": "2026-04-20T18:04:25.567993893Z",
  "memory_max": 8,
  "memory_min": 512
}"#,
    )
    .expect("write config");

    let all = manager.load_all();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].memory_max.as_deref(), Some("8G"));
    assert_eq!(all[0].memory_min.as_deref(), Some("512M"));
}

#[test]
fn load_one_missing_returns_not_found() {
    let (manager, _tmp) = test_manager();
    let result = manager.load_one("ghost-instance");
    assert!(matches!(result, Err(InstanceError::NotFound(_))));
}

#[test]
fn rename_moves_dir_and_updates_config_name() {
    let (manager, tmp) = test_manager();
    let old_dir = tmp.path().join("old-name");
    std::fs::create_dir_all(&old_dir).unwrap();
    manager.save(&dummy_config("old-name")).expect("save");

    manager.rename("old-name", "new-name").expect("rename");

    assert!(!old_dir.exists(), "old dir should be gone");
    let new_dir = tmp.path().join("new-name");
    assert!(new_dir.exists(), "new dir should exist");
    let reloaded = manager.load_one("new-name").expect("load_one new-name");
    assert_eq!(reloaded.name, "new-name");
}

#[test]
fn rename_to_same_name_is_noop() {
    let (manager, tmp) = test_manager();
    let dir = tmp.path().join("same");
    std::fs::create_dir_all(&dir).unwrap();
    manager.save(&dummy_config("same")).expect("save");
    manager.rename("same", "same").expect("noop rename");
    assert!(dir.exists());
}

#[test]
fn rename_empty_target_rejects() {
    let (manager, tmp) = test_manager();
    std::fs::create_dir_all(tmp.path().join("orig")).unwrap();
    manager.save(&dummy_config("orig")).expect("save");
    let err = manager.rename("orig", "   ").unwrap_err();
    assert!(matches!(err, InstanceError::InvalidName(_)));
}

#[test]
fn rename_traversal_target_rejects() {
    let (manager, tmp) = test_manager();
    std::fs::create_dir_all(tmp.path().join("orig")).unwrap();
    manager.save(&dummy_config("orig")).expect("save");
    let err = manager.rename("orig", "../escape").unwrap_err();
    assert!(matches!(err, InstanceError::InvalidName(_)));
    assert!(!tmp.path().parent().unwrap().join("escape").exists());
    assert!(tmp.path().join("orig").exists(), "source must be untouched");
}

#[test]
fn rename_missing_source_errors() {
    let (manager, _tmp) = test_manager();
    let err = manager.rename("ghost", "anything").unwrap_err();
    assert!(matches!(err, InstanceError::NotFound(_)));
}

#[test]
fn rename_target_exists_errors() {
    let (manager, tmp) = test_manager();
    std::fs::create_dir_all(tmp.path().join("source")).unwrap();
    std::fs::create_dir_all(tmp.path().join("collision")).unwrap();
    manager.save(&dummy_config("source")).expect("save src");
    manager.save(&dummy_config("collision")).expect("save dst");
    let err = manager.rename("source", "collision").unwrap_err();
    assert!(matches!(err, InstanceError::AlreadyExists(_)));
}

#[test]
fn rename_with_invalid_source_is_rejected() {
    let (manager, _tmp) = test_manager();
    let err = manager.rename("../outside", "safe-name").unwrap_err();
    assert!(matches!(err, InstanceError::InvalidName(_)));
}

#[test]
fn save_rejects_invalid_instance_name() {
    let (manager, _tmp) = test_manager();
    let err = manager.save(&dummy_config("../outside")).unwrap_err();
    assert!(matches!(err, InstanceError::InvalidName(_)));
}

#[test]
fn touch_last_played_updates_field() {
    let (manager, tmp) = test_manager();
    std::fs::create_dir_all(tmp.path().join("ticker")).unwrap();
    manager.save(&dummy_config("ticker")).expect("save");
    assert!(manager.load_one("ticker").unwrap().last_played.is_none());

    manager.touch_last_played("ticker").expect("touch");
    let reloaded = manager.load_one("ticker").unwrap();
    let stamp = reloaded
        .last_played
        .expect("last_played should be Some now");
    let age = chrono::Utc::now() - stamp;
    assert!(
        age.num_seconds().abs() < 5,
        "last_played should be roughly now, got age {age:?}"
    );
}
