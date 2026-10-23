use super::*;

fn options() -> ExportOptions {
    ExportOptions {
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
        export_curseforge(&root, &output, &options).await.unwrap();

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
        let error = export_curseforge(&root, &output, &options())
            .await
            .unwrap_err();
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
    let error = export_curseforge(&root, &output, &options())
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("Output already exists"),
        "{error}"
    );
    assert_eq!(std::fs::read(&output).unwrap(), b"existing pack");
}

#[tokio::test]
async fn export_rejects_nonportable_include_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = game_folder(temp.path());
    let output = temp.path().join("pack.zip");
    for path in ["../outside", "config//nested", "config/./nested"] {
        let mut options = options();
        options.include = vec![PathBuf::from(path)];
        let error = export_curseforge(&root, &output, &options)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("Invalid export path"),
            "{path}: {error}"
        );
        assert!(!output.exists());
    }
}

#[test]
fn archive_writer_preserves_output_created_after_preflight() {
    let temp = tempfile::tempdir().unwrap();
    let root = game_folder(temp.path());
    let output = temp.path().join("pack.zip");
    assert!(!output.exists());
    let options = options();
    let files = selected_files(&root, &options.include).unwrap();
    std::fs::write(&output, b"another export").unwrap();
    assert!(write_archive(&root, &output, &options, &files, &BTreeMap::new()).is_err());
    assert_eq!(std::fs::read(&output).unwrap(), b"another export");
}
