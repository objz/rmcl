use super::*;

fn options() -> ExportOptions {
    ExportOptions {
        format: PackFormat::CurseForge,
        name: "Exported Pack".to_owned(),
        version: "1.2.3".to_owned(),
        author: "Pack Author".to_owned(),
        game_version: "1.20.1".to_owned(),
        loader: ModLoader::Forge,
        loader_version: Some("47.2.0".to_owned()),
        include: vec![PathBuf::from("options.txt")],
        bundle_unmatched: false,
    }
}

fn game_folder(parent: &Path) -> PathBuf {
    let root = parent.join("game");
    std::fs::create_dir_all(root.join("config/nested")).unwrap();
    std::fs::write(
        root.join("config/nested/settings.json"),
        b"{\"enabled\":true}",
    )
    .unwrap();
    std::fs::write(root.join("options.txt"), b"music:0.5").unwrap();
    root
}

#[tokio::test]
async fn exported_overrides_round_trip_through_curseforge_import() {
    for loader in [ModLoader::Vanilla, ModLoader::Forge] {
        let temp = tempfile::tempdir().unwrap();
        let root = game_folder(temp.path());
        let output = temp.path().join("pack.zip");
        let mut options = options();
        options.loader = loader;
        if loader == ModLoader::Vanilla {
            options.loader_version = None;
        }
        export_pack(&root, &output, &options).await.unwrap();

        let summary = super::super::import::build_summary(&output).unwrap();
        assert_eq!(summary.name, options.name);
        assert_eq!(summary.pack_version, options.version);
        assert_eq!(summary.game_version, options.game_version);
        assert_eq!(summary.loader, options.loader);
        assert_eq!(summary.loader_version, options.loader_version);
        assert_eq!(summary.override_count, 2);
        assert_eq!(summary.mod_count, 0);
        let manager = super::super::InstanceManager::new(
            temp.path().join("instances"),
            temp.path().join("meta"),
        );
        let config = serde_json::from_value(serde_json::json!({
            "name": summary.name,
            "game_version": summary.game_version,
            "loader": summary.loader,
            "loader_version": summary.loader_version,
            "created": "2026-01-01T00:00:00Z"
        }))
        .unwrap();
        super::super::import::curseforge::execute_import(&summary, &manager, &config)
            .await
            .unwrap();
        let imported = crate::storage::InstancePaths::new(manager.instances_dir.join(&config.name))
            .minecraft();
        for path in ["config/nested/settings.json", "options.txt"] {
            assert_eq!(
                std::fs::read(imported.join(path)).unwrap(),
                std::fs::read(root.join(path)).unwrap()
            );
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn export_rejects_nonportable_filenames_before_writing_archive() {
    for name in [
        "bad?.json",
        "bad*.json",
        "NUL.json",
        "trailing.",
        "trailing ",
        r"bad\name.json",
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = game_folder(temp.path());
        std::fs::write(root.join("config").join(name), b"invalid").unwrap();
        let output = temp.path().join("pack.zip");
        let error = export_pack(&root, &output, &options()).await.unwrap_err();
        assert!(
            error.to_string().contains("Invalid export path"),
            "{name}: {error}"
        );
        assert!(!output.exists(), "{name}");
    }
}

#[tokio::test]
async fn export_preserves_existing_output() {
    let temp = tempfile::tempdir().unwrap();
    let root = game_folder(temp.path());
    let output = temp.path().join("pack.zip");
    std::fs::write(&output, b"existing pack").unwrap();
    let error = export_pack(&root, &output, &options()).await.unwrap_err();
    assert!(
        error.to_string().contains("Output already exists"),
        "{error}"
    );
    assert_eq!(std::fs::read(&output).unwrap(), b"existing pack");
}

#[tokio::test]
async fn approved_overwrites_replace_only_completed_archives() {
    for format in PackFormat::ALL {
        let temp = tempfile::tempdir().unwrap();
        let root = game_folder(temp.path());
        let output = temp.path().join(format!("pack.{}", format.extension()));
        let mut options = options();
        options.format = format;
        std::fs::write(&output, b"existing pack").unwrap();
        validate_output_path(&root, &output).unwrap();
        let plan = prepare_export(&root, &options).await.unwrap();
        std::fs::write(root.join("options.txt"), b"changed after preparation").unwrap();
        assert!(
            write_export_with_overwrite(plan, &output, true)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"existing pack");
        let plan = prepare_export(&root, &options).await.unwrap();
        write_export_with_overwrite(plan, &output, true)
            .await
            .unwrap();
        assert_eq!(
            super::super::import::build_summary(&output).unwrap().format,
            format
        );
    }
}

#[tokio::test]
async fn overwrite_rejects_non_files_and_paths_inside_the_game() {
    let temp = tempfile::tempdir().unwrap();
    let root = game_folder(temp.path());
    let output = temp.path().join("directory.zip");
    std::fs::create_dir(&output).unwrap();
    let plan = prepare_export(&root, &options()).await.unwrap();
    assert!(
        write_export_with_overwrite(plan, &output, true)
            .await
            .is_err()
    );
    assert!(output.is_dir());
    let inside = root.join("pack.zip");
    std::fs::write(&inside, b"keep").unwrap();
    let plan = prepare_export(&root, &options()).await.unwrap();
    assert!(
        write_export_with_overwrite(plan, &inside, true)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(inside).unwrap(), b"keep");
}

#[cfg(unix)]
#[tokio::test]
async fn overwrite_rejects_symlinks_without_modifying_the_target() {
    let temp = tempfile::tempdir().unwrap();
    let root = game_folder(temp.path());
    let target = temp.path().join("keep.zip");
    let output = temp.path().join("link.zip");
    std::fs::write(&target, b"keep").unwrap();
    std::os::unix::fs::symlink(&target, &output).unwrap();
    let plan = prepare_export(&root, &options()).await.unwrap();
    assert!(
        write_export_with_overwrite(plan, &output, true)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(target).unwrap(), b"keep");
}

#[tokio::test]
async fn export_rejects_nonportable_include_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = game_folder(temp.path());
    let output = temp.path().join("pack.zip");
    for path in ["../outside", "config//nested", "config/./nested"] {
        let mut options = options();
        options.include = vec![PathBuf::from(path)];
        let error = export_pack(&root, &output, &options).await.unwrap_err();
        assert!(
            error.to_string().contains("Invalid export path"),
            "{path}: {error}"
        );
        assert!(!output.exists());
    }
}

#[tokio::test]
async fn archive_writer_preserves_output_created_after_preflight() {
    let temp = tempfile::tempdir().unwrap();
    let root = game_folder(temp.path());
    let output = temp.path().join("pack.zip");
    assert!(!output.exists());
    let options = options();
    let plan = prepare_export(&root, &options).await.unwrap();
    std::fs::write(&output, b"another export").unwrap();
    assert!(write_export(plan, &output).await.is_err());
    assert_eq!(std::fs::read(&output).unwrap(), b"another export");
}

#[tokio::test]
async fn all_formats_round_trip_metadata_and_bundled_files() {
    for format in PackFormat::ALL {
        for loader in [
            ModLoader::Vanilla,
            ModLoader::Forge,
            ModLoader::Fabric,
            ModLoader::NeoForge,
            ModLoader::Quilt,
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = game_folder(temp.path());
            let output = temp.path().join(format!("pack.{}", format.extension()));
            let mut options = options();
            options.format = format;
            options.loader = loader;
            options.loader_version = (loader != ModLoader::Vanilla).then(|| "1.2.3".to_owned());
            let report = export_pack(&root, &output, &options).await.unwrap();
            assert_eq!(report.bundled_count, 2);
            assert_eq!(report.referenced_count, 0);
            let summary = super::super::import::build_summary(&output).unwrap();
            assert_eq!(summary.format, format);
            assert_eq!(summary.name, options.name);
            assert_eq!(summary.loader, loader);
            assert_eq!(summary.loader_version, options.loader_version);
            assert_eq!(summary.game_version, options.game_version);
            let manager =
                InstanceManager::new(temp.path().join("instances"), temp.path().join("meta"));
            let config = test_config(&options);
            match format {
                PackFormat::CurseForge => {
                    super::super::import::curseforge::execute_import(&summary, &manager, &config)
                        .await
                }
                PackFormat::Mrpack => {
                    super::super::import::mrpack::execute_import(&summary, &manager, &config).await
                }
                PackFormat::Mmc => {
                    super::super::import::mmc::execute_import(&summary, &manager, &config).await
                }
            }
            .unwrap();
            let imported =
                crate::storage::InstancePaths::new(manager.instances_dir.join(&config.name))
                    .minecraft();
            for path in ["config/nested/settings.json", "options.txt"] {
                assert_eq!(
                    std::fs::read(imported.join(path)).unwrap(),
                    std::fs::read(root.join(path)).unwrap()
                );
            }
        }
    }
}

fn test_config(options: &ExportOptions) -> InstanceConfig {
    serde_json::from_value(serde_json::json!({
        "name": options.name, "game_version": options.game_version, "loader": options.loader,
        "loader_version": options.loader_version, "created": "2026-01-01T00:00:00Z"
    }))
    .unwrap()
}

#[tokio::test]
async fn multimc_bundles_mods_without_provider_access_and_excludes_personal_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = game_folder(temp.path());
    for (path, data) in [
        ("mods/custom.jar", "custom"),
        ("mods/old.jar.disabled", "disabled"),
        ("logs/latest.log", "private"),
        ("saves/World/level.dat", "world"),
    ] {
        std::fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
        std::fs::write(root.join(path), data).unwrap();
    }
    let mut options = options();
    options.format = PackFormat::Mmc;
    let output = temp.path().join("pack.zip");
    let report = export_pack(&root, &output, &options).await.unwrap();
    assert_eq!(report.bundled_count, 3);
    let mut archive = zip::ZipArchive::new(std::fs::File::open(output).unwrap()).unwrap();
    assert!(archive.by_name(".minecraft/mods/custom.jar").is_ok());
    for path in [
        ".minecraft/mods/old.jar.disabled",
        ".minecraft/logs/latest.log",
        ".minecraft/saves/World/level.dat",
    ] {
        assert!(archive.by_name(path).is_err());
    }
}

#[tokio::test]
async fn managed_export_holds_lock_and_reloads_loader_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let manager = InstanceManager::new(temp.path().join("instances"), temp.path().join("meta"));
    let mut options = options();
    let config = test_config(&options);
    std::fs::create_dir_all(
        crate::storage::InstancePaths::new(manager.instances_dir.join(&config.name)).minecraft(),
    )
    .unwrap();
    manager.save(&config).unwrap();
    std::fs::write(
        crate::storage::InstancePaths::new(manager.instances_dir.join(&config.name))
            .minecraft()
            .join("options.txt"),
        b"settings",
    )
    .unwrap();
    options.loader = ModLoader::Vanilla;
    options.loader_version = None;
    let plan = prepare_instance_export(&manager, &config.name, &options)
        .await
        .unwrap();
    assert_eq!(plan.options.loader, config.loader);
    assert_eq!(plan.options.loader_version, config.loader_version);
    assert!(super::super::runtime::lock_instance(&manager.instances_dir, &config.name).is_err());
    drop(plan);
    assert!(super::super::runtime::lock_instance(&manager.instances_dir, &config.name).is_ok());
}

#[tokio::test]
async fn archive_rejects_changed_files_and_output_inside_game_folder() {
    for format in PackFormat::ALL {
        let temp = tempfile::tempdir().unwrap();
        let root = game_folder(temp.path());
        let mut options = options();
        options.format = format;
        assert!(
            export_pack(&root, &root.join("config/pack.zip"), &options)
                .await
                .is_err()
        );
        let plan = prepare_export(&root, &options).await.unwrap();
        std::fs::write(root.join("options.txt"), b"changed settings").unwrap();
        let output = temp.path().join("pack.zip");
        assert!(
            write_export(plan, &output)
                .await
                .unwrap_err()
                .to_string()
                .contains("changed after preparation")
        );
        assert!(!output.exists());
    }
}

fn content_plan(root: &Path, format: PackFormat) -> ExportPlan {
    let mut options = options();
    options.format = format;
    options.include.clear();
    let progress = ProgressTask::start("Export test");
    inventory(root.to_owned(), options, None, &progress.handle()).unwrap()
}

fn version(plan: &ExportPlan, path: &str) -> crate::net::modrinth::VersionInfo {
    let hash = &plan.files[Path::new(path)];
    serde_json::from_value(serde_json::json!({
        "id": "9", "project_id": "7", "name": "Example", "version_number": "1", "game_versions": [], "loaders": [],
        "files": [{"url": "https://cdn.modrinth.com/data/7/versions/9/example.jar", "filename": "example.jar", "size": hash.size, "primary": true,
            "hashes": {"sha1": hash.hash("sha1"), "sha512": hash.hash("sha512")}}]
    })).unwrap()
}

#[tokio::test]
async fn archive_rejects_referenced_mod_changes_even_when_size_and_timestamp_are_preserved() {
    for format in [PackFormat::CurseForge, PackFormat::Mrpack] {
        let temp = tempfile::tempdir().unwrap();
        let root = game_folder(temp.path());
        std::fs::create_dir(root.join("mods")).unwrap();
        let path = root.join("mods/example.jar");
        std::fs::write(&path, b"old").unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let mut plan = content_plan(&root, format);
        let matched = version(&plan, "mods/example.jar");
        plan.archive = match format {
            PackFormat::CurseForge => curseforge::manifest(
                &plan,
                &BTreeMap::from([(
                    plan.files[Path::new("mods/example.jar")]
                        .hash("curseforge")
                        .unwrap()
                        .parse()
                        .unwrap(),
                    matched,
                )]),
            )
            .unwrap(),
            PackFormat::Mrpack => mrpack::manifest(
                &plan,
                &HashMap::from([(
                    plan.files[Path::new("mods/example.jar")]
                        .hash("sha512")
                        .unwrap()
                        .to_owned(),
                    matched,
                )]),
            )
            .unwrap(),
            PackFormat::Mmc => unreachable!(),
        };
        assert_eq!(plan.referenced_count(), 1);
        std::fs::write(&path, b"new").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        let current = fingerprint_metadata(&path).unwrap();
        let expected = &plan.files[Path::new("mods/example.jar")];
        assert_eq!(current.size, expected.size);
        assert_eq!(current.modified_ns, expected.modified_ns);
        let output = temp.path().join(format!("pack.{}", format.extension()));
        assert!(
            write_export(plan, &output)
                .await
                .unwrap_err()
                .to_string()
                .contains("changed after preparation")
        );
        assert!(!output.exists());
    }
}

#[tokio::test]
async fn multimc_names_are_quoted_for_qt_and_round_trip_through_import() {
    for (name, encoded) in [
        ("Plain Pack", r#""Plain Pack""#),
        ("Pack, with commas", r#""Pack, with commas""#),
        ("Pack; semicolon", r#""Pack; semicolon""#),
        (r#"A "quoted" pack"#, r#""A \"quoted\" pack""#),
        (r"C:\new\folder", r#""C:\\new\\folder""#),
        ("@String(hello)", r#""@@String(hello)""#),
        ("@@Pack", r#""@@@Pack""#),
        (" 雪 ❄ ", "\" 雪 ❄ \""),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let root = game_folder(temp.path());
        let mut options = options();
        options.name = name.to_owned();
        options.format = PackFormat::Mmc;
        let output = temp.path().join("pack.zip");
        export_pack(&root, &output, &options).await.unwrap();
        let mut archive = zip::ZipArchive::new(std::fs::File::open(&output).unwrap()).unwrap();
        let mut cfg = String::new();
        std::io::Read::read_to_string(&mut archive.by_name("instance.cfg").unwrap(), &mut cfg)
            .unwrap();
        assert!(
            cfg.lines().any(|line| line == format!("name={encoded}")),
            "{cfg}"
        );
        assert_eq!(
            super::super::import::build_summary(&output).unwrap().name,
            name
        );
    }
}

#[test]
fn export_scan_checks_cancellation_between_directory_entries() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("one.txt"), b"one").unwrap();
    std::fs::write(temp.path().join("two.txt"), b"two").unwrap();
    let progress = ProgressTask::start("Export cancellation test");
    let calls = std::cell::Cell::new(0);
    let check = || {
        calls.set(calls.get() + 1);
        if calls.get() == 3 {
            Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled",
            ))
        } else {
            Ok(())
        }
    };
    let mut files = BTreeMap::new();
    let error = scan_directory(
        temp.path(),
        Path::new(""),
        &mut files,
        &mut Vec::new(),
        &ContentManifest::default(),
        &progress,
        &check,
    )
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::Interrupted
    );
    assert_eq!(files.len(), 1);
}

#[test]
fn export_fingerprints_reuse_only_complete_unchanged_cache_entries() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("mod.jar");
    std::fs::write(&path, b"cached content").unwrap();
    let expected = super::super::content::manifest::fingerprint(&path).unwrap();
    let calls = std::cell::Cell::new(0);
    let check = || {
        calls.set(calls.get() + 1);
        Ok(())
    };
    assert_eq!(
        export_fingerprint(&path, Some(&expected), check).unwrap(),
        expected
    );
    assert_eq!(
        calls.get(),
        1,
        "A cache hit must not enter either hashing pass"
    );
    let mut wrong_size = expected.clone();
    wrong_size.size += 1;
    let mut wrong_time = expected.clone();
    wrong_time.modified_ns += 1;
    let mut incomplete = expected.clone();
    incomplete.hashes.remove("sha512");
    for cached in [
        None,
        Some(&wrong_size),
        Some(&wrong_time),
        Some(&incomplete),
    ] {
        calls.set(0);
        assert_eq!(export_fingerprint(&path, cached, check).unwrap(), expected);
        assert!(calls.get() > 1);
    }
    assert!(
        export_fingerprint(&path, Some(&expected), || Err(std::io::Error::from(
            std::io::ErrorKind::Interrupted
        )))
        .is_err()
    );
}

#[tokio::test]
async fn export_scan_rechecks_cached_hashes_and_recovers_on_retry() {
    for managed in [true, false] {
        for format in PackFormat::ALL {
            if !managed && format == PackFormat::Mmc {
                continue;
            }
            let temp = tempfile::tempdir().unwrap();
            let manager =
                InstanceManager::new(temp.path().join("instances"), temp.path().join("meta"));
            let mut options = options();
            options.format = format;
            options.bundle_unmatched = true;
            options.include.clear();
            let config = test_config(&options);
            let paths =
                crate::storage::InstancePaths::new(manager.instances_dir.join(&config.name));
            std::fs::create_dir_all(paths.minecraft().join("mods")).unwrap();
            let path = paths.minecraft().join("mods/cached.jar");
            std::fs::write(&path, b"original").unwrap();
            let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
            let expected = super::super::content::manifest::fingerprint(&path).unwrap();
            manager.save(&config).unwrap();
            let manifest: ContentManifest = serde_json::from_value(serde_json::json!({
                "version": 1,
                "files": [{
                    "relative_path": "mods/cached.jar",
                    "kind": "mod",
                    "enabled": true,
                    "fingerprint": expected,
                    "resolution": {"status": "resolved", "project": {
                        "provider": "modrinth",
                        "project_id": "old-project",
                        "version_id": "old-version"
                    }},
                    "automatic_dependency": true
                }]
            }))
            .unwrap();
            manifest.save(&paths.content_manifest()).unwrap();
            std::fs::write(&path, b"replaced").unwrap();
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(modified)
                .unwrap();
            let scan = if managed {
                Some(scan_instance_export(&manager, &config.name).await.unwrap())
            } else {
                None
            };
            let references = ExportReferences::empty(format);
            let progress = ProgressTask::start("Cached export inventory");
            let plan = if let Some(scan) = &scan {
                scan.plan(&options, &scan.defaults, &references).unwrap()
            } else {
                inventory(paths.minecraft(), options.clone(), None, &progress.handle()).unwrap()
            };
            assert_eq!(plan.files[Path::new("mods/cached.jar")], expected);
            let output = temp.path().join("pack.zip");
            assert!(
                write_export(plan, &output)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("changed after preparation")
            );
            assert!(!output.exists());
            let mut invalidated = manifest.clone();
            invalidated.files[0].fingerprint.modified_ns = u128::MAX;
            assert_eq!(
                ContentManifest::load(&paths.content_manifest())
                    .unwrap()
                    .files,
                invalidated.files,
                "Invalidating the cache must preserve hashes and ownership"
            );
            assert!(
                super::super::content::local::validate_record(
                    paths.minecraft().as_path(),
                    &invalidated.files[0]
                )
                .is_err()
            );
            if managed {
                assert!(
                    super::super::runtime::lock_content(&manager.instances_dir, &config.name)
                        .is_err()
                );
            }
            drop(scan);

            let progress = ProgressTask::start("Retry export inventory");
            let inventory =
                inventory(paths.minecraft(), options.clone(), None, &progress.handle()).unwrap();
            let actual = super::super::content::manifest::fingerprint(&path).unwrap();
            if format != PackFormat::Mmc {
                assert_eq!(inventory.files[Path::new("mods/cached.jar")], actual);
            }
            validate_inventory(&inventory).unwrap();
            drop(inventory);

            let retry = scan_instance_export(&manager, &config.name).await.unwrap();
            assert_eq!(retry.files[Path::new("mods/cached.jar")], actual);
            let plan = retry.plan(&options, &retry.defaults, &references).unwrap();
            write_export(plan, &output).await.unwrap();
            let mut archive = zip::ZipArchive::new(std::fs::File::open(&output).unwrap()).unwrap();
            let prefix = if format == PackFormat::Mmc {
                ".minecraft"
            } else if format == PackFormat::Mrpack {
                "client-overrides"
            } else {
                "overrides"
            };
            let mut bytes = Vec::new();
            std::io::Read::read_to_end(
                &mut archive
                    .by_name(&format!("{prefix}/mods/cached.jar"))
                    .unwrap(),
                &mut bytes,
            )
            .unwrap();
            assert_eq!(bytes, b"replaced");
        }
    }
}

#[test]
fn curseforge_references_require_exact_sha1_even_when_fingerprints_match() {
    let temp = tempfile::tempdir().unwrap();
    let root = game_folder(temp.path());
    std::fs::create_dir(root.join("mods")).unwrap();
    std::fs::write(root.join("mods/example.jar"), b"mod").unwrap();
    let plan = content_plan(&root, PackFormat::CurseForge);
    let hash = plan.files[Path::new("mods/example.jar")]
        .hash("curseforge")
        .unwrap()
        .parse()
        .unwrap();
    let mut matched = version(&plan, "mods/example.jar");
    let archive = curseforge::manifest(&plan, &BTreeMap::from([(hash, matched.clone())])).unwrap();
    assert!(archive.referenced.contains(Path::new("mods/example.jar")));
    let manifest: serde_json::Value = serde_json::from_slice(&archive.manifests[0].1).unwrap();
    assert_eq!(manifest["files"][0]["projectID"], 7);
    assert_eq!(manifest["files"][0]["fileID"], 9);
    matched.files[0]
        .hashes
        .insert("sha1".to_owned(), "wrong".to_owned());
    assert!(
        curseforge::manifest(&plan, &BTreeMap::from([(hash, matched)]))
            .unwrap()
            .referenced
            .is_empty()
    );
}

#[test]
fn mrpack_references_preserve_paths_hashes_sizes_and_exact_download_artifact() {
    let temp = tempfile::tempdir().unwrap();
    let root = game_folder(temp.path());
    std::fs::create_dir(root.join("resourcepacks")).unwrap();
    std::fs::write(root.join("resourcepacks/renamed.zip"), b"resource pack").unwrap();
    let plan = content_plan(&root, PackFormat::Mrpack);
    let path = "resourcepacks/renamed.zip";
    let sha512 = plan.files[Path::new(path)]
        .hash("sha512")
        .unwrap()
        .to_owned();
    let mut matched = version(&plan, path);
    let mut wrong_primary = matched.files[0].clone();
    wrong_primary
        .hashes
        .insert("sha512".to_owned(), "wrong".to_owned());
    matched.files.insert(0, wrong_primary);
    let archive = mrpack::manifest(
        &plan,
        &std::collections::HashMap::from([(sha512.clone(), matched.clone())]),
    )
    .unwrap();
    let index: super::super::import::mrpack::MrpackIndex =
        serde_json::from_slice(&archive.manifests[0].1).unwrap();
    assert_eq!(index.files[0].path, path);
    assert_eq!(index.files[0].hashes["sha512"], sha512);
    assert_eq!(index.files[0].file_size, 13);
    assert_eq!(index.files[0].env["server"], "unsupported");
    matched.files[1].url = "https://untrusted.example/mod.jar".to_owned();
    assert!(
        mrpack::manifest(&plan, &std::collections::HashMap::from([(sha512, matched)]))
            .unwrap()
            .referenced
            .is_empty()
    );
}

#[tokio::test]
async fn unmatched_content_needs_explicit_bundling_approval() {
    let temp = tempfile::tempdir().unwrap();
    let root = game_folder(temp.path());
    std::fs::create_dir(root.join("mods")).unwrap();
    std::fs::write(root.join("mods/custom.txt"), b"custom").unwrap();
    for format in [PackFormat::CurseForge, PackFormat::Mrpack] {
        let mut options = options();
        options.format = format;
        let output = temp
            .path()
            .join(format!("{}.{}", format.label(), format.extension()));
        let mut plan = prepare_export(&root, &options).await.unwrap();
        assert_eq!(plan.unmatched, [PathBuf::from("mods/custom.txt")]);
        assert!(write_export(plan, &output).await.is_err());
        assert!(!output.exists());
        plan = prepare_export(&root, &options).await.unwrap();
        plan.options.bundle_unmatched = true;
        assert_eq!(
            write_export(plan, &output).await.unwrap().unmatched,
            [PathBuf::from("mods/custom.txt")]
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn export_rejects_symlinks_in_all_formats() {
    let temp = tempfile::tempdir().unwrap();
    let root = game_folder(temp.path());
    std::fs::write(temp.path().join("private"), b"private").unwrap();
    std::os::unix::fs::symlink(temp.path().join("private"), root.join("config/secret")).unwrap();
    for format in PackFormat::ALL {
        let mut options = options();
        options.format = format;
        assert!(
            prepare_export(&root, &options)
                .await
                .err()
                .unwrap()
                .to_string()
                .contains("symlinks")
        );
    }
}

#[tokio::test]
async fn export_scan_defaults_and_explicit_selection_are_independent() {
    let temp = tempfile::tempdir().unwrap();
    let manager = InstanceManager::new(temp.path().join("instances"), temp.path().join("meta"));
    let mut options = options();
    options.format = PackFormat::Mmc;
    let config = test_config(&options);
    let root =
        crate::storage::InstancePaths::new(manager.instances_dir.join(&config.name)).minecraft();
    for (path, data) in [
        ("config/keep.txt", "keep"),
        ("config/omit.txt", "omit"),
        ("options.txt", "options"),
        ("logs/latest.log", "private"),
        ("saves/World/level.dat", "world"),
        ("mods/off.jar.disabled", "off"),
    ] {
        std::fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
        std::fs::write(root.join(path), data).unwrap();
    }
    manager.save(&config).unwrap();
    let scan = scan_instance_export(&manager, &config.name).await.unwrap();
    assert_eq!(
        scan.defaults,
        BTreeSet::from([
            PathBuf::from("config/keep.txt"),
            PathBuf::from("config/omit.txt")
        ])
    );
    assert!(scan.paths().any(|path| path == Path::new("options.txt")));
    assert!(
        scan.paths()
            .any(|path| path == Path::new("saves/World/level.dat"))
    );
    assert!(
        !scan
            .paths()
            .any(|path| path == Path::new("mods/off.jar.disabled"))
    );
    let selected = BTreeSet::from([
        PathBuf::from("config/keep.txt"),
        PathBuf::from("options.txt"),
    ]);
    let plan = scan
        .plan(&options, &selected, &ExportReferences::Bundled)
        .unwrap();
    std::fs::write(root.join("config/omit.txt"), b"changed unselected file").unwrap();
    std::fs::write(root.join("logs/new.log"), b"unselected new log").unwrap();
    let output = temp.path().join("pack.zip");
    assert_eq!(write_export(plan, &output).await.unwrap().bundled_count, 2);
    let mut archive = zip::ZipArchive::new(std::fs::File::open(output).unwrap()).unwrap();
    assert!(archive.by_name(".minecraft/config/keep.txt").is_ok());
    assert!(archive.by_name(".minecraft/options.txt").is_ok());
    assert!(archive.by_name(".minecraft/config/omit.txt").is_err());
    let unknown = BTreeSet::from([PathBuf::from("../secret")]);
    assert!(
        scan.plan(&options, &unknown, &ExportReferences::Bundled)
            .is_err()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn export_scan_skips_unsafe_and_symlinked_entries_without_following_them() {
    let temp = tempfile::tempdir().unwrap();
    let manager = InstanceManager::new(temp.path().join("instances"), temp.path().join("meta"));
    let options = options();
    let config = test_config(&options);
    let root =
        crate::storage::InstancePaths::new(manager.instances_dir.join(&config.name)).minecraft();
    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(root.join("config/safe.txt"), b"safe").unwrap();
    std::fs::write(root.join("config/bad?.txt"), b"unsafe").unwrap();
    std::os::unix::fs::symlink(temp.path().join("missing"), root.join("config/link")).unwrap();
    manager.save(&config).unwrap();
    let scan = scan_instance_export(&manager, &config.name).await.unwrap();
    assert_eq!(
        scan.paths().cloned().collect::<Vec<_>>(),
        [PathBuf::from("config/safe.txt")]
    );
    assert_eq!(scan.skipped.len(), 2);
}

#[test]
fn curseforge_does_not_reference_resource_packs_from_a_shared_scan() {
    let temp = tempfile::tempdir().unwrap();
    let root = game_folder(temp.path());
    std::fs::create_dir(root.join("resourcepacks")).unwrap();
    std::fs::write(root.join("resourcepacks/pack.zip"), b"pack").unwrap();
    let plan = content_plan(&root, PackFormat::Mrpack);
    let path = "resourcepacks/pack.zip";
    let fingerprint = plan.files[Path::new(path)]
        .hash("curseforge")
        .unwrap()
        .parse()
        .unwrap();
    let archive = curseforge::manifest(
        &plan,
        &BTreeMap::from([(fingerprint, version(&plan, path))]),
    )
    .unwrap();
    assert!(archive.referenced.is_empty());
}
