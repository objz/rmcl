// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn first_prepare_seeds_shared_config() {
    let tmp = tempfile::tempdir().unwrap();
    let meta = tmp.path().join("meta");
    let minecraft = tmp.path().join("instance/minecraft");
    create_profile(&meta, "main").unwrap();
    std::fs::create_dir_all(minecraft.join("config/nested")).unwrap();
    std::fs::write(minecraft.join("options.txt"), "local-options").unwrap();
    std::fs::write(minecraft.join("optionsshaders.txt"), "shader-options").unwrap();
    std::fs::write(minecraft.join("config/options.txt"), "local-config").unwrap();
    std::fs::write(minecraft.join("config/nested/mod.toml"), "nested").unwrap();

    assert!(prepare(Some("main"), &meta, &minecraft).unwrap());

    assert_eq!(
        std::fs::read_to_string(meta.join("state/profiles/main/options.txt")).unwrap(),
        "local-options"
    );
    assert_eq!(
        std::fs::read_to_string(meta.join("state/profiles/main/optionsshaders.txt")).unwrap(),
        "shader-options"
    );
    assert_eq!(
        std::fs::read_to_string(meta.join("state/profiles/main/config/options.txt")).unwrap(),
        "local-config"
    );
    assert_eq!(
        std::fs::read_to_string(meta.join("state/profiles/main/config/nested/mod.toml")).unwrap(),
        "nested"
    );
}

#[test]
fn prepare_mirrors_shared_config_into_instance() {
    let tmp = tempfile::tempdir().unwrap();
    let meta = tmp.path().join("meta");
    let minecraft = tmp.path().join("instance/minecraft");
    std::fs::create_dir_all(meta.join("state/profiles/main/config")).unwrap();
    std::fs::create_dir_all(minecraft.join("config")).unwrap();
    std::fs::write(
        meta.join("state/profiles/main/options.txt"),
        "shared-options",
    )
    .unwrap();
    std::fs::write(
        meta.join("state/profiles/main/config/shared.toml"),
        "shared",
    )
    .unwrap();
    std::fs::write(minecraft.join("options.txt"), "stale-options").unwrap();
    std::fs::write(minecraft.join("config/local.toml"), "stale").unwrap();

    assert!(prepare(Some("main"), &meta, &minecraft).unwrap());

    assert_eq!(
        std::fs::read_to_string(minecraft.join("options.txt")).unwrap(),
        "shared-options"
    );
    assert_eq!(
        std::fs::read_to_string(minecraft.join("config/shared.toml")).unwrap(),
        "shared"
    );
    assert!(!minecraft.join("config/local.toml").exists());
}

#[test]
fn finish_mirrors_instance_config_back_to_shared() {
    let tmp = tempfile::tempdir().unwrap();
    let meta = tmp.path().join("meta");
    let minecraft = tmp.path().join("instance/minecraft");
    std::fs::create_dir_all(meta.join("state/profiles/main/config")).unwrap();
    std::fs::write(meta.join("state/profiles/main/config/old.toml"), "old").unwrap();
    std::fs::create_dir_all(minecraft.join("config")).unwrap();
    std::fs::write(minecraft.join("options.txt"), "new-options").unwrap();
    std::fs::write(minecraft.join("config/new.toml"), "new").unwrap();

    finish(Some("main"), &meta, &minecraft).unwrap();

    assert_eq!(
        std::fs::read_to_string(meta.join("state/profiles/main/options.txt")).unwrap(),
        "new-options"
    );
    assert_eq!(
        std::fs::read_to_string(meta.join("state/profiles/main/config/new.toml")).unwrap(),
        "new"
    );
    assert!(!meta.join("state/profiles/main/config/old.toml").exists());
}

#[test]
fn prepare_releases_lock_for_another_instance() {
    let tmp = tempfile::tempdir().unwrap();
    let meta = tmp.path().join("meta");
    let minecraft = tmp.path().join("instance/minecraft");
    create_profile(&meta, "main").unwrap();
    std::fs::create_dir_all(minecraft.join("config")).unwrap();

    assert!(prepare(Some("main"), &meta, &minecraft).unwrap());
    let second = tmp.path().join("second/minecraft");
    std::fs::create_dir_all(second.join("config")).unwrap();

    assert!(prepare(Some("main"), &meta, &second).unwrap());
}

#[test]
fn profile_rejects_path_traversal() {
    let tmp = tempfile::tempdir().unwrap();
    let err = prepare(Some("../bad"), tmp.path(), tmp.path()).unwrap_err();

    assert!(matches!(err, ConfigSyncError::InvalidProfile(_)));
}

#[test]
fn profile_rejects_builtin_names() {
    for profile in [
        "none",
        "default",
        "local",
        "instance default",
        "local default",
    ] {
        let err = validate_profile(profile).unwrap_err();
        assert!(matches!(err, ConfigSyncError::InvalidProfile(_)));
    }
}

#[test]
fn prepare_ignores_deleted_profile() {
    let tmp = tempfile::tempdir().unwrap();
    let meta = tmp.path().join("meta");
    let minecraft = tmp.path().join("instance/minecraft");
    std::fs::create_dir_all(minecraft.join("config")).unwrap();

    let lock = prepare(Some("deleted"), &meta, &minecraft).unwrap();

    assert!(!lock);
    assert!(!meta.join("state/profiles/deleted").exists());
}

#[test]
fn create_profile_trims_and_lists_profiles() {
    let tmp = tempfile::tempdir().unwrap();

    let profile = create_profile(tmp.path(), " main ").unwrap();
    let profiles = list_profiles(tmp.path()).unwrap();

    assert_eq!(profile, "main");
    assert_eq!(profiles, vec!["main"]);
}

#[test]
fn delete_profile_removes_profile_dir() {
    let tmp = tempfile::tempdir().unwrap();
    create_profile(tmp.path(), "main").unwrap();

    delete_profile(tmp.path(), "main").unwrap();

    assert!(list_profiles(tmp.path()).unwrap().is_empty());
}

#[test]
fn switch_to_profile_backs_up_local_config_and_restores_none() {
    let tmp = tempfile::tempdir().unwrap();
    let meta = tmp.path().join("meta");
    let instance = tmp.path().join("instance");
    let minecraft = instance.join(crate::storage::MINECRAFT_DIR_NAME);
    std::fs::create_dir_all(minecraft.join("config")).unwrap();
    std::fs::write(minecraft.join("options.txt"), "local-options").unwrap();
    std::fs::write(minecraft.join("config/local.txt"), "local").unwrap();

    let selected = switch_profile("inst", None, Some("main"), &meta, &instance).unwrap();
    assert_eq!(selected.as_deref(), Some("main"));
    assert_eq!(
        std::fs::read_to_string(instance.join("rmcl/content/config/options.txt")).unwrap(),
        "local-options"
    );
    assert_eq!(
        std::fs::read_to_string(instance.join("rmcl/content/config/config/local.txt")).unwrap(),
        "local"
    );

    std::fs::write(minecraft.join("options.txt"), "shared-options").unwrap();
    std::fs::write(minecraft.join("config/shared.txt"), "shared").unwrap();
    let selected = switch_profile("inst", Some("main"), None, &meta, &instance).unwrap();

    assert_eq!(selected, None);
    assert_eq!(
        std::fs::read_to_string(meta.join("state/profiles/main/options.txt")).unwrap(),
        "shared-options"
    );
    assert_eq!(
        std::fs::read_to_string(meta.join("state/profiles/main/config/shared.txt")).unwrap(),
        "shared"
    );
    assert_eq!(
        std::fs::read_to_string(minecraft.join("options.txt")).unwrap(),
        "local-options"
    );
    assert_eq!(
        std::fs::read_to_string(minecraft.join("config/local.txt")).unwrap(),
        "local"
    );
    assert!(!minecraft.join("config/shared.txt").exists());
}

#[test]
fn switch_from_deleted_profile_restores_local_without_recreating_profile() {
    let tmp = tempfile::tempdir().unwrap();
    let meta = tmp.path().join("meta");
    let instance = tmp.path().join("instance");
    let minecraft = instance.join(crate::storage::MINECRAFT_DIR_NAME);
    std::fs::create_dir_all(minecraft.join("config")).unwrap();
    std::fs::create_dir_all(instance.join("rmcl/content/config/config")).unwrap();
    std::fs::write(minecraft.join("options.txt"), "deleted-profile-options").unwrap();
    std::fs::write(
        instance.join("rmcl/content/config/options.txt"),
        "local-options",
    )
    .unwrap();
    std::fs::write(
        instance.join("rmcl/content/config/config/local.txt"),
        "local",
    )
    .unwrap();

    let selected = switch_profile("inst", Some("deleted"), None, &meta, &instance).unwrap();

    assert_eq!(selected, None);
    assert_eq!(
        std::fs::read_to_string(minecraft.join("options.txt")).unwrap(),
        "local-options"
    );
    assert_eq!(
        std::fs::read_to_string(minecraft.join("config/local.txt")).unwrap(),
        "local"
    );
    assert!(!meta.join("state/profiles/deleted").exists());
}

#[test]
fn switch_between_profiles_saves_old_and_loads_new() {
    let tmp = tempfile::tempdir().unwrap();
    let meta = tmp.path().join("meta");
    let instance = tmp.path().join("instance");
    let minecraft = instance.join(crate::storage::MINECRAFT_DIR_NAME);
    std::fs::create_dir_all(minecraft.join("config")).unwrap();
    std::fs::write(minecraft.join("options.txt"), "changed-a-options").unwrap();
    std::fs::write(minecraft.join("config/a.txt"), "changed-a").unwrap();
    create_profile(&meta, "a").unwrap();
    std::fs::create_dir_all(meta.join("state/profiles/b/config")).unwrap();
    std::fs::write(
        meta.join("state/profiles/b/options.txt"),
        "profile-b-options",
    )
    .unwrap();
    std::fs::write(meta.join("state/profiles/b/config/b.txt"), "profile-b").unwrap();

    let selected = switch_profile("inst", Some("a"), Some("b"), &meta, &instance).unwrap();

    assert_eq!(selected.as_deref(), Some("b"));
    assert_eq!(
        std::fs::read_to_string(meta.join("state/profiles/a/options.txt")).unwrap(),
        "changed-a-options"
    );
    assert_eq!(
        std::fs::read_to_string(meta.join("state/profiles/a/config/a.txt")).unwrap(),
        "changed-a"
    );
    assert_eq!(
        std::fs::read_to_string(minecraft.join("options.txt")).unwrap(),
        "profile-b-options"
    );
    assert_eq!(
        std::fs::read_to_string(minecraft.join("config/b.txt")).unwrap(),
        "profile-b"
    );
    assert!(!minecraft.join("config/a.txt").exists());
}

#[test]
fn failed_profile_save_restores_previous_files() {
    let tmp = tempfile::tempdir().unwrap();
    let meta = tmp.path().join("meta");
    let instance = tmp.path().join("instance");
    let minecraft = instance.join(crate::storage::MINECRAFT_DIR_NAME);
    std::fs::create_dir_all(minecraft.join("config")).unwrap();
    std::fs::create_dir_all(meta.join("state/profiles/main/config")).unwrap();
    std::fs::write(minecraft.join("options.txt"), "local-options").unwrap();
    std::fs::write(minecraft.join("config/local.txt"), "local").unwrap();
    std::fs::write(
        meta.join("state/profiles/main/options.txt"),
        "shared-options",
    )
    .unwrap();
    std::fs::write(meta.join("state/profiles/main/config/shared.txt"), "shared").unwrap();

    let error = switch_profile_and_persist("inst", None, Some("main"), &meta, &instance, |_| {
        Err(std::io::Error::other("disk full"))
    })
    .unwrap_err();

    assert!(matches!(error, ConfigSyncError::Save(_)));
    assert_eq!(
        std::fs::read_to_string(minecraft.join("options.txt")).unwrap(),
        "local-options"
    );
    assert_eq!(
        std::fs::read_to_string(minecraft.join("config/local.txt")).unwrap(),
        "local"
    );
    assert!(!minecraft.join("config/shared.txt").exists());
}

#[test]
fn second_instance_uses_profile_options_saved_by_first_instance() {
    let tmp = tempfile::tempdir().unwrap();
    let meta = tmp.path().join("meta");
    let first = tmp.path().join("first/minecraft");
    let second_instance = tmp.path().join("second");
    let second = second_instance.join(crate::storage::MINECRAFT_DIR_NAME);
    std::fs::create_dir_all(first.join("config")).unwrap();
    std::fs::create_dir_all(second.join("config")).unwrap();
    std::fs::write(first.join("options.txt"), "first-default").unwrap();
    std::fs::write(second.join("options.txt"), "second-local").unwrap();
    create_profile(&meta, "main").unwrap();

    assert!(prepare(Some("main"), &meta, &first).unwrap());
    std::fs::write(first.join("options.txt"), "changed-in-main").unwrap();
    finish(Some("main"), &meta, &first).unwrap();

    switch_profile("second", None, Some("main"), &meta, &second_instance).unwrap();

    assert_eq!(
        std::fs::read_to_string(second.join("options.txt")).unwrap(),
        "changed-in-main"
    );
    assert_eq!(
        std::fs::read_to_string(second_instance.join("rmcl/content/config/options.txt")).unwrap(),
        "second-local"
    );
}
