// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn failed_publication_restores_the_current_backup_and_prior_moves() {
    let temp = tempfile::tempdir().unwrap();
    let old = temp.path().join("old.jar");
    let prior = temp.path().join("prior.jar");
    let occupied = temp.path().join("occupied.jar");
    std::fs::write(&old, b"original").unwrap();
    std::fs::write(&prior, b"prior").unwrap();
    std::fs::create_dir(&occupied).unwrap();
    let mut transaction = FileTransaction::new(StagingDirectory::new(temp.path()).unwrap());
    let first_backup = transaction.staging().join("first-backup");
    let backup = transaction.staging().join("backup");
    let download = transaction.staging().join("download");
    let first_download = transaction.staging().join("first-download");
    std::fs::write(&download, b"new").unwrap();
    std::fs::write(&first_download, b"new-prior").unwrap();
    transaction.move_path(&prior, &first_backup).unwrap();
    transaction.move_path(&first_download, &prior).unwrap();
    transaction.move_path(&old, &backup).unwrap();
    let error = transaction.move_path(&download, &occupied).unwrap_err();
    let result: io::Result<()> = transaction.finish(Err(error));
    assert!(result.is_err());
    assert_eq!(std::fs::read(old).unwrap(), b"original");
    assert_eq!(std::fs::read(prior).unwrap(), b"prior");
    assert!(occupied.is_dir());
}

#[test]
fn rollback_failure_keeps_backups_and_reports_both_paths() {
    let temp = tempfile::tempdir().unwrap();
    let old = temp.path().join("old.jar");
    std::fs::write(&old, b"original").unwrap();
    let mut transaction = FileTransaction::new(StagingDirectory::new(temp.path()).unwrap());
    let staging = transaction.staging().to_owned();
    let backup = staging.join("backup");
    transaction.move_path(&old, &backup).unwrap();
    std::fs::write(&old, b"foreign").unwrap();
    let result: io::Result<()> = transaction.finish(Err(io::Error::other("publication failed")));
    let message = result.unwrap_err().to_string();
    assert!(message.contains("publication failed"));
    assert!(message.contains("rollback failed"));
    assert!(message.contains(&old.display().to_string()));
    assert!(message.contains(&backup.display().to_string()));
    drop(transaction);
    assert_eq!(std::fs::read(backup).unwrap(), b"original");
    assert_eq!(std::fs::read(old).unwrap(), b"foreign");
    assert!(staging.exists());
}

#[test]
fn a_failed_publication_undo_keeps_its_backup_and_restores_independent_files() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first.jar");
    let second = temp.path().join("second.jar");
    std::fs::write(&first, b"old-first").unwrap();
    std::fs::write(&second, b"old-second").unwrap();
    let mut transaction = FileTransaction::new(StagingDirectory::new(temp.path()).unwrap());
    let staging = transaction.staging().to_owned();
    let first_download = staging.join("first-download");
    let second_download = staging.join("second-download");
    let first_backup = staging.join("first-backup");
    let second_backup = staging.join("second-backup");
    std::fs::write(&first_download, b"new-first").unwrap();
    std::fs::write(&second_download, b"new-second").unwrap();
    transaction.move_path(&first, &first_backup).unwrap();
    transaction.move_path(&first_download, &first).unwrap();
    transaction.move_path(&second, &second_backup).unwrap();
    transaction.move_path(&second_download, &second).unwrap();
    std::fs::write(&second_download, b"foreign").unwrap();

    let result: io::Result<()> =
        transaction.finish(Err(io::Error::other("manifest publication failed")));
    let message = result.unwrap_err().to_string();
    assert!(message.contains("manifest publication failed"));
    assert!(message.contains(&second_download.display().to_string()));
    assert!(message.contains(&second_backup.display().to_string()));
    drop(transaction);
    assert_eq!(std::fs::read(first).unwrap(), b"old-first");
    assert_eq!(std::fs::read(second_backup).unwrap(), b"old-second");
    assert_eq!(std::fs::read(second).unwrap(), b"new-second");
    assert_eq!(std::fs::read(second_download).unwrap(), b"foreign");
    assert!(staging.exists());
}

#[test]
fn unwinding_restores_files_and_retains_the_recovery_directory() {
    let temp = tempfile::tempdir().unwrap();
    let old = temp.path().join("old.jar");
    std::fs::write(&old, b"old").unwrap();
    let mut transaction = FileTransaction::new(StagingDirectory::new(temp.path()).unwrap());
    let staging = transaction.staging().to_owned();
    let backup = staging.join("backup");
    let result = std::panic::catch_unwind(move || {
        transaction.move_path(&old, &backup).unwrap();
        panic!("commit failed");
    });
    assert!(result.is_err());
    assert_eq!(std::fs::read(temp.path().join("old.jar")).unwrap(), b"old");
    assert!(staging.exists());
}

fn managed_mod(instance_dir: &Path) -> (PathBuf, PathBuf, ContentFileRecord) {
    let paths = crate::storage::InstancePaths::new(instance_dir.join("instance"));
    let minecraft = paths.minecraft();
    let manifest_path = paths.content_manifest();
    let path = minecraft.join("mods/example.jar");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"original").unwrap();
    let record = ContentFileRecord {
        relative_path: "mods/example.jar".into(),
        kind: super::super::manifest::ContentKind::Mod,
        enabled: true,
        fingerprint: fingerprint(&path).unwrap(),
        resolution: crate::instance::Resolution::Resolved {
            project: super::super::manifest::ProviderProject {
                provider: "modrinth".to_owned(),
                project_id: "example".to_owned(),
                version_id: "old".to_owned(),
            },
        },
        provider_aliases: Vec::new(),
        provider_checks: vec!["modrinth".to_owned()],
        required_dependencies: Vec::new(),
        automatic_dependency: true,
        cleanup_eligible: true,
    };
    ContentManifest {
        version: 1,
        files: vec![record.clone()],
    }
    .save(&manifest_path)
    .unwrap();
    (minecraft, manifest_path, record)
}

#[test]
fn toggles_and_removals_share_the_install_guard_and_preserve_record_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let (minecraft, manifest_path, record) = managed_mod(temp.path());
    let old = minecraft.join(&record.relative_path);
    let mut entry = super::super::mods::scan_one_mod(&old, "example", true);
    entry.provider_project = record.resolved_project().cloned();
    let lock = ContentLock::acquire(&manifest_path).unwrap();
    assert!(
        toggle(&entry)
            .unwrap_err()
            .to_string()
            .contains("another operation")
    );
    assert!(
        remove(
            &manifest_path,
            &minecraft,
            &old,
            std::slice::from_ref(&record),
            false
        )
        .unwrap_err()
        .to_string()
        .contains("another operation")
    );
    assert_eq!(std::fs::read(&old).unwrap(), b"original");
    drop(lock);

    let disabled = toggle(&entry).unwrap().unwrap();
    let mut expected = record;
    expected.relative_path = "mods/example.jar.disabled".into();
    expected.enabled = false;
    assert_eq!(
        ContentManifest::load(&manifest_path).unwrap().files,
        vec![expected.clone()]
    );
    assert_eq!(std::fs::read(&disabled).unwrap(), b"original");
    assert!(!old.exists());
    let (manifest, _) = remove(&manifest_path, &minecraft, &disabled, &[expected], true).unwrap();
    assert!(manifest.files.is_empty());
    assert!(!disabled.exists());
}

#[test]
fn content_mutations_coordinate_with_the_instance_lifetime_guard() {
    let temp = tempfile::tempdir().unwrap();
    let (minecraft, manifest_path, record) = managed_mod(temp.path());
    let instance_lock = crate::instance::runtime::lock_instance(temp.path(), "instance").unwrap();
    assert!(matches!(
        ContentLock::acquire(&manifest_path),
        Err(super::super::manifest::ManifestError::Busy(_))
    ));
    let path = minecraft.join(&record.relative_path);
    assert!(remove(&manifest_path, &minecraft, &path, &[record], false).is_err());
    drop(instance_lock);
    let content_lock = ContentLock::acquire(&manifest_path).unwrap();
    assert!(crate::instance::runtime::lock_instance(temp.path(), "instance").is_err());
    drop(content_lock);
    crate::instance::runtime::lock_instance(temp.path(), "instance").unwrap();
    assert_eq!(std::fs::read(path).unwrap(), b"original");
}

#[test]
fn running_instances_allow_content_toggles_and_removal_but_still_prevent_duplicate_launches() {
    let temp = tempfile::tempdir().unwrap();
    let (minecraft, manifest_path, record) = managed_mod(temp.path());
    let path = minecraft.join(&record.relative_path);
    let mut entry = super::super::mods::scan_one_mod(&path, "example", true);
    entry.provider_project = record.resolved_project().cloned();
    let mut running = crate::instance::runtime::lock_instance(temp.path(), "instance").unwrap();
    assert!(
        toggle(&entry).is_err(),
        "Content must remain protected during launch preparation"
    );
    running.allow_content_changes().unwrap();
    let disabled = toggle(&entry).unwrap().unwrap();
    assert_eq!(std::fs::read(&disabled).unwrap(), b"original");
    assert!(!path.exists());
    let expected = ContentManifest::load(&manifest_path).unwrap().files;
    let (manifest, _) = remove(&manifest_path, &minecraft, &disabled, &expected, true).unwrap();
    assert!(manifest.files.is_empty());
    assert!(!disabled.exists());
    assert!(crate::instance::runtime::lock_instance(temp.path(), "instance").is_err());
    drop(running);
    crate::instance::runtime::lock_instance(temp.path(), "instance").unwrap();
}

#[test]
fn stale_content_actions_reject_new_ownership_and_changed_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let (minecraft, manifest_path, record) = managed_mod(temp.path());
    let path = minecraft.join(&record.relative_path);
    let mut entry = super::super::mods::scan_one_mod(&path, "example", true);
    entry.provider_project = record.resolved_project().cloned();
    let mut current = record.clone();
    current.resolution = crate::instance::Resolution::Pending;
    ContentManifest {
        version: 1,
        files: vec![current],
    }
    .save(&manifest_path)
    .unwrap();
    assert!(
        toggle(&entry)
            .unwrap_err()
            .to_string()
            .contains("Ownership")
    );
    assert!(
        remove(
            &manifest_path,
            &minecraft,
            &path,
            std::slice::from_ref(&record),
            false
        )
        .unwrap_err()
        .to_string()
        .contains("Ownership")
    );
    ContentManifest {
        version: 1,
        files: vec![record.clone()],
    }
    .save(&manifest_path)
    .unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    std::fs::write(&path, b"external").unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert!(toggle(&entry).unwrap_err().to_string().contains("changed"));
    assert!(
        remove(&manifest_path, &minecraft, &path, &[record], false)
            .unwrap_err()
            .to_string()
            .contains("changed")
    );
    assert_eq!(std::fs::read(path).unwrap(), b"external");
}

#[test]
fn deleting_a_world_validates_and_removes_its_descendant_records() {
    let temp = tempfile::tempdir().unwrap();
    let (minecraft, manifest_path, mut record) = managed_mod(temp.path());
    let path = minecraft.join("saves/world/datapacks/example.zip");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"pack").unwrap();
    record.relative_path = "saves/world/datapacks/example.zip".into();
    record.kind = super::super::manifest::ContentKind::DataPack;
    record.fingerprint = fingerprint(&path).unwrap();
    ContentManifest {
        version: 1,
        files: vec![record.clone()],
    }
    .save(&manifest_path)
    .unwrap();
    let world = minecraft.join("saves/world");
    assert!(
        remove(&manifest_path, &minecraft, &world, &[], false)
            .unwrap_err()
            .to_string()
            .contains("Ownership")
    );
    let mut running = crate::instance::runtime::lock_instance(temp.path(), "instance").unwrap();
    running.allow_content_changes().unwrap();
    assert!(
        remove(
            &manifest_path,
            &minecraft,
            &world,
            std::slice::from_ref(&record),
            false
        )
        .is_err()
    );
    let entry = super::super::worlds::scan_one_world(&world, "world", true);
    assert!(
        toggle(&entry).is_err(),
        "Renaming a live save must remain blocked"
    );
    assert!(world.exists(), "Deleting a live save must remain blocked");
    drop(running);
    let (manifest, _) = remove(&manifest_path, &minecraft, &world, &[record], false).unwrap();
    assert!(manifest.files.is_empty());
    assert!(!world.exists());
}

#[test]
fn orphan_cleanup_rechecks_new_dependents_before_removing_a_library() {
    let temp = tempfile::tempdir().unwrap();
    let (minecraft, manifest_path, record) = managed_mod(temp.path());
    let path = minecraft.join(&record.relative_path);
    let mut dependent = record.clone();
    dependent.relative_path = "mods/dependent.jar".into();
    dependent.automatic_dependency = false;
    dependent.cleanup_eligible = false;
    dependent.required_dependencies = vec![record.resolved_project().unwrap().clone()];
    ContentManifest {
        version: 1,
        files: vec![dependent, record.clone()],
    }
    .save(&manifest_path)
    .unwrap();
    assert!(
        remove(&manifest_path, &minecraft, &path, &[record], true)
            .unwrap_err()
            .to_string()
            .contains("no longer an unused dependency")
    );
    assert_eq!(std::fs::read(path).unwrap(), b"original");
}

#[test]
fn content_named_minecraft_uses_the_instance_manifest_and_lock() {
    let temp = tempfile::tempdir().unwrap();
    let (minecraft, manifest_path, original) = managed_mod(temp.path());
    for (relative, kind, directory) in [
        (
            "saves/minecraft/datapacks/example.zip",
            super::super::manifest::ContentKind::DataPack,
            false,
        ),
        (
            "resourcepacks/minecraft",
            super::super::manifest::ContentKind::ResourcePack,
            true,
        ),
    ] {
        let path = minecraft.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        if directory {
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("pack.mcmeta"), b"{}").unwrap();
        } else {
            std::fs::write(&path, b"pack").unwrap();
        }
        let mut record = original.clone();
        record.relative_path = relative.into();
        record.kind = kind;
        record.fingerprint = if directory {
            directory_fingerprint_metadata(&path).unwrap()
        } else {
            fingerprint(&path).unwrap()
        };
        ContentManifest {
            version: 1,
            files: vec![record.clone()],
        }
        .save(&manifest_path)
        .unwrap();
        let mut entry = super::super::mods::scan_one_mod(&path, "example", true);
        entry.provider_project = record.resolved_project().cloned();
        let lock = ContentLock::acquire(&manifest_path).unwrap();
        assert!(
            toggle(&entry)
                .unwrap_err()
                .to_string()
                .contains("another operation")
        );
        drop(lock);
        let new_path = toggle(&entry).unwrap().unwrap();
        let updated = ContentManifest::load(&manifest_path).unwrap();
        assert_eq!(minecraft.join(&updated.files[0].relative_path), new_path);
        assert!(!updated.files[0].enabled);
    }
}

#[test]
fn failed_manifest_publication_rolls_back_live_file_moves() {
    let temp = tempfile::tempdir().unwrap();
    let (minecraft, manifest_path, record) = managed_mod(temp.path());
    let old = minecraft.join(&record.relative_path);
    let target = minecraft.join("mods/new.jar");
    let lock = ContentLock::acquire(&manifest_path).unwrap();
    let previous = lock.load().unwrap();
    let mut transaction = FileTransaction::new(StagingDirectory::new(&minecraft).unwrap());
    let backup = transaction.staging().join("backup");
    let download = transaction.staging().join("download");
    std::fs::write(&download, b"new").unwrap();
    transaction.move_path(&old, &backup).unwrap();
    transaction.move_path(&download, &target).unwrap();
    std::fs::remove_file(&manifest_path).unwrap();
    std::fs::create_dir(&manifest_path).unwrap();
    let result = lock.save_if_unchanged(&previous, &previous);
    assert!(transaction.finish(result).is_err());
    assert_eq!(std::fs::read(old).unwrap(), b"original");
    assert!(!target.exists());
}

#[tokio::test]
async fn cancelling_the_await_keeps_the_blocking_transaction_guard_until_completion() {
    let temp = tempfile::tempdir().unwrap();
    let (minecraft, manifest_path, record) = managed_mod(temp.path());
    let job_manifest = manifest_path.clone();
    let job_minecraft = minecraft.clone();
    let old = minecraft.join(&record.relative_path);
    let disabled = old.with_file_name("example.jar.disabled");
    let new_path = disabled.clone();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    let caller = tokio::spawn(async move {
        tokio::task::spawn_blocking(move || {
            let lock = ContentLock::acquire(&job_manifest).unwrap();
            let previous = lock.load().unwrap();
            let mut transaction =
                FileTransaction::new(StagingDirectory::new(&job_minecraft).unwrap());
            transaction.move_path(&old, &new_path).unwrap();
            started_tx.send(()).unwrap();
            release_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            let mut manifest = previous.clone();
            assert!(manifest.rename_record(
                Path::new("mods/example.jar"),
                Path::new("mods/example.jar.disabled"),
                false
            ));
            let result = lock.save_if_unchanged(&manifest, &previous);
            transaction.finish(result).unwrap();
            drop(transaction);
            drop(lock);
            let _ = finished_tx.send(());
        })
        .await
        .unwrap();
    });
    started_rx.await.unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert!(ContentLock::acquire(&manifest_path).is_err());
    release_tx.send(()).unwrap();
    finished_rx.await.unwrap();
    ContentLock::acquire(&manifest_path).unwrap();
    assert_eq!(std::fs::read(disabled).unwrap(), b"original");
    assert!(!ContentManifest::load(&manifest_path).unwrap().files[0].enabled);
}
