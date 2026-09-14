// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn instance_paths_use_visible_directories() {
    let paths = InstancePaths::new("/instances/example");
    assert_eq!(
        paths.minecraft(),
        PathBuf::from("/instances/example/minecraft")
    );
    assert_eq!(
        paths.content_manifest(),
        PathBuf::from("/instances/example/rmcl/content/manifest.json")
    );
    assert_eq!(
        paths.local_config(),
        PathBuf::from("/instances/example/rmcl/content/config")
    );
}

#[test]
fn metadata_paths_separate_state_and_cache() {
    let paths = MetadataPaths::new("/meta");
    assert_eq!(paths.profiles(), PathBuf::from("/meta/state/profiles"));
    assert_eq!(
        paths.provider_icons("modrinth"),
        PathBuf::from("/meta/cache/providers/modrinth/icons")
    );
    assert_eq!(
        paths.versions(),
        PathBuf::from("/meta/cache/minecraft/versions")
    );
    assert_eq!(
        paths.java_installations(),
        PathBuf::from("/meta/cache/java/installations.json")
    );
    assert_eq!(
        paths.migration_journal(),
        PathBuf::from("/meta/state/migration.json")
    );
}

#[test]
fn disposable_cache_cleanup_preserves_runtime_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let paths = MetadataPaths::new(temp.path());
    std::fs::create_dir_all(paths.provider_icons("modrinth")).unwrap();
    std::fs::create_dir_all(paths.java_installations().parent().unwrap()).unwrap();
    std::fs::create_dir_all(paths.temporary()).unwrap();
    std::fs::create_dir_all(paths.versions()).unwrap();
    std::fs::write(paths.java_installations(), "[]").unwrap();

    clear_disposable_caches(temp.path()).unwrap();

    assert!(!paths.cache().join("providers").exists());
    assert!(!paths.cache().join("java").exists());
    assert!(paths.temporary().exists());
    assert!(paths.versions().exists());
}

#[test]
fn atomic_write_replaces_existing_file() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state.json");
    std::fs::write(&path, b"old").unwrap();
    write_atomic(&path, b"new").unwrap();
    assert_eq!(std::fs::read(path).unwrap(), b"new");
}
