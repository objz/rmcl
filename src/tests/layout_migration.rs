// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn migration_backs_up_and_renames_instance_directories() {
    let temp = tempfile::tempdir().unwrap();
    let instances = temp.path().join("instances");
    let meta = temp.path().join("meta");
    let config = temp.path().join("config.toml");
    let instance = instances.join("Example");
    fs::create_dir_all(instance.join(".minecraft/saves/world")).unwrap();
    fs::create_dir_all(instance.join(".rmcl/config-sync/local-config/config")).unwrap();
    fs::write(instance.join(".minecraft/saves/world/level.dat"), b"world").unwrap();
    fs::write(
        instance.join(".rmcl/config-sync/local-config/options.txt"),
        b"options",
    )
    .unwrap();
    fs::write(&config, b"[paths]").unwrap();
    fs::create_dir_all(meta.join("config-sync/profiles/main")).unwrap();
    fs::write(
        meta.join("config-sync/profiles/main/options.txt"),
        b"profile",
    )
    .unwrap();

    let mut progress = Vec::new();
    let backup = run(&instances, &meta, &config, |update| progress.push(update)).unwrap();

    assert_eq!(
        fs::read(instance.join("minecraft/saves/world/level.dat")).unwrap(),
        b"world"
    );
    assert_eq!(
        fs::read(instance.join("rmcl/content/config/options.txt")).unwrap(),
        b"options"
    );
    assert!(backup.join("instances/Example/.minecraft").exists());
    assert_eq!(
        fs::read_to_string(backup.join("config/config.toml")).unwrap(),
        "[paths]"
    );
    let upgraded_config = fs::read_to_string(&config).unwrap();
    assert!(upgraded_config.contains("check_modpack_updates = true"));
    assert!(upgraded_config.contains("resolution = [854, 480]"));
    assert!(backup.join("profiles/legacy/main/options.txt").exists());
    assert!(
        backup
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("backup-")
    );
    assert!(!meta.join("config-sync").exists());
    assert!(progress.iter().any(|update| {
        update.item_total.is_some_and(|total| total > 0)
            && update.item_current.is_some_and(|current| current > 0)
    }));
    assert_eq!(
        marker_version(&MetadataPaths::new(&meta).layout_marker()),
        Some(LAYOUT_VERSION)
    );
    assert!(cache_rebuild_pending(&meta));
    assert_eq!(run(&instances, &meta, &config, |_| {}).unwrap(), backup);
    finish_cache_rebuild(&meta).unwrap();
    assert!(!cache_rebuild_pending(&meta));
}

#[test]
fn migration_resumes_from_the_recorded_journal() {
    let temp = tempfile::tempdir().unwrap();
    let instances = temp.path().join("instances");
    let meta = temp.path().join("meta");
    let config = temp.path().join("config.toml");
    let done = instances.join("Done");
    let pending = instances.join("Pending");
    fs::create_dir_all(done.join("minecraft")).unwrap();
    fs::create_dir_all(pending.join(".minecraft/saves")).unwrap();

    let metadata = MetadataPaths::new(&meta);
    fs::create_dir_all(metadata.state()).unwrap();
    fs::create_dir_all(metadata.backups()).unwrap();
    let backup = metadata.backups().join("backup-resume");
    fs::create_dir_all(&backup).unwrap();
    write_json_atomic(
        &metadata.migration_journal(),
        &MigrationJournal {
            version: LAYOUT_VERSION,
            backup_dir: backup.clone(),
            completed: vec!["backup".to_owned(), "instance:Done".to_owned()],
        },
    )
    .unwrap();

    assert_eq!(run(&instances, &meta, &config, |_| {}).unwrap(), backup);
    assert!(done.join("minecraft").exists());
    assert!(pending.join("minecraft/saves").exists());
    assert!(!metadata.migration_journal().exists());
}

#[test]
fn stale_partial_backup_is_replaced() {
    let temp = tempfile::tempdir().unwrap();
    let instances = temp.path().join("instances");
    let backup = temp.path().join("backups/backup-test");
    let partial = backup.with_extension("partial");
    fs::create_dir_all(instances.join("Example")).unwrap();
    fs::write(instances.join("Example/data.txt"), b"current").unwrap();
    fs::create_dir_all(&partial).unwrap();
    fs::write(partial.join("stale.txt"), b"stale").unwrap();

    backup_user_data(
        &instances,
        &temp.path().join("missing-config.toml"),
        &temp.path().join("meta"),
        &backup,
        |_, _, _| {},
    )
    .unwrap();

    assert_eq!(
        fs::read(backup.join("instances/Example/data.txt")).unwrap(),
        b"current"
    );
    assert!(!backup.join("stale.txt").exists());
    assert!(!partial.exists());
}

#[test]
fn backup_rejects_overlap_and_insufficient_space() {
    let temp = tempfile::tempdir().unwrap();
    let instances = temp.path().join("instances");
    fs::create_dir_all(&instances).unwrap();

    let overlap =
        validate_backup_destination(&instances, &instances.join("backup"), 0).unwrap_err();
    assert!(matches!(overlap, MigrationError::BackupOverlap(_)));

    let outside = temp.path().join("backups/backup");
    let insufficient = validate_backup_destination(&instances, &outside, u64::MAX).unwrap_err();
    assert!(matches!(
        insufficient,
        MigrationError::InsufficientSpace { .. }
    ));
}

#[test]
fn migration_need_follows_legacy_data_marker_and_pending_rebuild() {
    let temp = tempfile::tempdir().unwrap();
    let instances = temp.path().join("instances");
    let meta = temp.path().join("meta");
    fs::create_dir_all(instances.join("Example/.minecraft")).unwrap();

    assert!(is_needed(&instances, &meta));
    initialize_new_layout(&meta).unwrap();
    assert!(!is_needed(&instances, &meta));
    fs::write(MetadataPaths::new(&meta).layout_marker(), b"invalid marker").unwrap();
    assert!(is_needed(&instances, &meta));
    initialize_new_layout(&meta).unwrap();
    fs::write(
        MetadataPaths::new(&meta).cache_rebuild_pending(),
        LAYOUT_VERSION.to_string(),
    )
    .unwrap();
    assert!(is_needed(&instances, &meta));
}

#[cfg(unix)]
#[test]
fn backup_preserves_symlinks() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("target.txt"), b"target").unwrap();
    std::os::unix::fs::symlink("target.txt", source.join("link.txt")).unwrap();

    copy_dir_recursive_with_progress(&source, &destination, &mut |_, _| {}).unwrap();

    assert_eq!(
        fs::read_link(destination.join("link.txt")).unwrap(),
        PathBuf::from("target.txt")
    );
}

#[test]
fn conflicting_visible_and_hidden_directories_are_not_merged() {
    let temp = tempfile::tempdir().unwrap();
    let instance = temp.path().join("Example");
    fs::create_dir_all(instance.join(".minecraft")).unwrap();
    fs::create_dir_all(instance.join("minecraft")).unwrap();
    let error = migrate_instance(&instance, "Example").unwrap_err();
    assert!(matches!(error, MigrationError::PathConflict { .. }));
}

#[test]
fn merge_conflicts_do_not_overwrite_existing_profiles() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("legacy");
    let destination = temp.path().join("current");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::create_dir_all(destination.join("nested")).unwrap();
    fs::write(source.join("nested/options.txt"), b"legacy").unwrap();
    fs::write(destination.join("nested/options.txt"), b"current").unwrap();

    let error = move_or_merge(&source, &destination).unwrap_err();

    assert!(matches!(error, MigrationError::MergeConflict { .. }));
    assert_eq!(
        fs::read(destination.join("nested/options.txt")).unwrap(),
        b"current"
    );
    assert_eq!(
        fs::read(source.join("nested/options.txt")).unwrap(),
        b"legacy"
    );
}
