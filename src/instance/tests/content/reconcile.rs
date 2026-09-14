// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

struct NoopProgress;

impl InventoryProgress for NoopProgress {
    fn set_sub_action(&self, _text: &str) {}

    fn set_progress(&self, _current: u64, _total: u64) {}
}

fn resolved_record() -> ContentFileRecord {
    ContentFileRecord {
        relative_path: PathBuf::from("mods/example.jar"),
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
                project_id: "example".to_owned(),
                version_id: "1".to_owned(),
            },
        },
        provider_aliases: Vec::new(),
        provider_checks: Vec::new(),
        required_dependencies: Vec::new(),
        automatic_dependency: false,
        cleanup_eligible: false,
    }
}

fn job(name: &str) -> ReconcileJob {
    ReconcileJob {
        instance: InstanceConfig {
            name: name.to_owned(),
            game_version: "1.21.1".to_owned(),
            loader: crate::instance::ModLoader::Fabric,
            loader_version: None,
            created: chrono::DateTime::UNIX_EPOCH,
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
        },
        instances_dir: PathBuf::new(),
        client: crate::net::HttpClient::new(),
    }
}

#[test]
fn coordinator_queues_instances_once_and_preserves_order() {
    let mut coordinator = ReconcileCoordinator::default();
    let one = ("one".to_owned(), chrono::DateTime::UNIX_EPOCH);
    assert!(coordinator.enqueue(job("one"), false));
    assert!(!coordinator.enqueue(job("two"), false));
    assert!(!coordinator.enqueue(job("one"), false));
    assert!(!coordinator.rerun.contains(&one));
    assert!(!coordinator.enqueue(job("one"), true));
    assert_eq!(coordinator.queue.len(), 2);
    assert!(coordinator.rerun.contains(&one));
    assert_eq!(coordinator.queue.pop_front().unwrap().instance.name, "one");
    assert_eq!(coordinator.queue.pop_front().unwrap().instance.name, "two");
}

#[test]
fn coordinator_does_not_merge_recreated_instances_with_the_same_name() {
    let mut coordinator = ReconcileCoordinator::default();
    let first = job("same");
    let mut recreated = job("same");
    recreated.instance.created += chrono::TimeDelta::seconds(1);

    assert!(coordinator.enqueue(first, false));
    assert!(!coordinator.enqueue(recreated, false));
    assert_eq!(coordinator.queue.len(), 2);
}

#[test]
fn oversized_content_is_kept_without_hashing_or_provider_query() {
    let temp = tempfile::tempdir().unwrap();
    let minecraft = temp.path().join("minecraft");
    let resource_packs = minecraft.join("resourcepacks");
    std::fs::create_dir_all(&resource_packs).unwrap();
    let pack = resource_packs.join("large.zip");
    let file = std::fs::File::create(&pack).unwrap();
    file.set_len(2 * 1024 * 1024).unwrap();
    let manifest_path = temp.path().join("manifest.json");

    let inventory = reconcile_inventory(&manifest_path, &minecraft, 24, 1, &NoopProgress).unwrap();

    assert!(inventory.queries.is_empty());
    assert_eq!(inventory.manifest.files.len(), 1);
    assert!(inventory.manifest.files[0].fingerprint.hashes.is_empty());
    assert!(matches!(
        inventory.manifest.files[0].resolution,
        Resolution::Unmatched { .. }
    ));
}

#[test]
fn unchanged_saved_index_reuses_fingerprint_and_skips_provider_query() {
    let temp = tempfile::tempdir().unwrap();
    let minecraft = temp.path().join("minecraft");
    let mods = minecraft.join("mods");
    std::fs::create_dir_all(&mods).unwrap();
    std::fs::write(mods.join("example.jar"), b"example").unwrap();
    let manifest_path = temp.path().join("manifest.json");

    let mut first =
        reconcile_inventory(&manifest_path, &minecraft, 24, 512, &NoopProgress).unwrap();
    assert_eq!(first.queries.len(), 1);
    let fingerprint = first.manifest.files[0].fingerprint.clone();
    first.manifest.files[0].resolution = Resolution::Unmatched {
        checked_at: chrono::Utc::now().timestamp(),
        providers: vec!["modrinth".to_owned(), "curseforge".to_owned()],
    };
    first.manifest.save(&manifest_path).unwrap();

    let second = reconcile_inventory(&manifest_path, &minecraft, 24, 512, &NoopProgress).unwrap();
    assert!(second.queries.is_empty());
    assert_eq!(second.manifest.files[0].fingerprint, fingerprint);
}

#[test]
fn newly_configured_provider_retries_an_unmatched_record() {
    let mut record = resolved_record();

    assert!(provider_was_not_checked(&record, "curseforge"));
    assert!(!provider_was_not_checked(&record, "modrinth"));
    record.provider_checks.push("curseforge".to_owned());
    assert!(!provider_was_not_checked(&record, "curseforge"));
}

#[test]
fn reconciliation_saves_provider_aliases_for_an_existing_resolution() {
    let temp = tempfile::tempdir().unwrap();
    let manifest_path = temp.path().join("manifest.json");
    let current = ContentManifest {
        version: 1,
        files: vec![resolved_record()],
    };
    current.save(&manifest_path).unwrap();

    let mut enriched = resolved_record();
    enriched.provider_aliases.push(ProviderProject {
        provider: "curseforge".to_owned(),
        project_id: "42".to_owned(),
        version_id: "84".to_owned(),
    });
    enriched.provider_checks = vec!["modrinth".to_owned(), "curseforge".to_owned()];
    let saved = save_reconciled_manifest(
        &manifest_path,
        &temp.path().join("minecraft"),
        ContentManifest {
            version: 1,
            files: vec![enriched.clone()],
        },
    )
    .unwrap();

    assert_eq!(saved.files, vec![enriched]);
}

#[test]
fn directory_packs_are_indexed_without_provider_queries() {
    let temp = tempfile::tempdir().unwrap();
    let minecraft = temp.path().join("minecraft");
    let pack = minecraft.join("resourcepacks/example");
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::write(pack.join("pack.mcmeta"), b"{}").unwrap();

    let inventory = reconcile_inventory(
        &temp.path().join("manifest.json"),
        &minecraft,
        24,
        512,
        &NoopProgress,
    )
    .unwrap();

    assert!(inventory.queries.is_empty());
    assert_eq!(
        inventory.manifest.files[0].relative_path,
        PathBuf::from("resourcepacks/example")
    );
}

#[test]
fn datapacks_are_indexed_under_their_world() {
    let temp = tempfile::tempdir().unwrap();
    let minecraft = temp.path().join("minecraft");
    let datapacks = minecraft.join("saves/world/datapacks");
    std::fs::create_dir_all(datapacks.join("folder-pack")).unwrap();
    std::fs::write(datapacks.join("zipped-pack.zip"), b"zip").unwrap();

    let inventory = reconcile_inventory(
        &temp.path().join("manifest.json"),
        &minecraft,
        24,
        512,
        &NoopProgress,
    )
    .unwrap();

    assert_eq!(inventory.manifest.files.len(), 2);
    assert!(inventory.manifest.files.iter().all(|record| {
        record.kind == ContentKind::DataPack
            && record.relative_path.starts_with("saves/world/datapacks")
    }));
    assert_eq!(inventory.queries.len(), 1);
}
