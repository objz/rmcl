// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn content_lock_excludes_independent_handles_until_drop() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("manifest.json");
    let lock = ContentLock::acquire(&path).unwrap();
    assert!(matches!(
        ContentLock::acquire(&path),
        Err(ManifestError::Busy(_))
    ));
    assert!(matches!(
        ContentManifest::update(&path, |_| Ok(())),
        Err(ManifestError::Busy(_))
    ));
    drop(lock);
    assert!(path.with_extension("lock").exists());
    ContentLock::acquire(&path).unwrap();
}

#[test]
fn content_lock_with_held_instance_guard_still_excludes_other_manifest_handles() {
    let temp = tempfile::tempdir().unwrap();
    let guard = crate::instance::runtime::lock_instance(temp.path(), "Example").unwrap();
    let path = temp.path().join("Example/rmcl/content/manifest.json");
    let lock = ContentLock::acquire_with_held_instance_lock(&path, &guard).unwrap();
    assert!(matches!(
        ContentLock::acquire_with_held_instance_lock(&path, &guard),
        Err(ManifestError::Busy(_))
    ));
    drop(lock);
    ContentLock::acquire_with_held_instance_lock(&path, &guard).unwrap();
    assert!(ContentLock::acquire(&path).is_err());
    drop(guard);
    ContentLock::acquire(&path).unwrap();
}

#[test]
fn dropping_guard_releases_the_content_lock_with_a_duplicated_handle() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("instance/rmcl/content/manifest.json");
    let lock = ContentLock::acquire(&path).unwrap();
    let _manifest_handle = lock.file.try_clone().unwrap();
    drop(lock);
    ContentLock::acquire(&path).unwrap();
}

#[test]
fn content_lock_excludes_other_processes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("manifest.json");
    let lock = ContentLock::acquire(&path).unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "instance::content::manifest::tests::content_lock_probe",
        ])
        .env("RMCL_TEST_CONTENT_MANIFEST", &path)
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{}{}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    assert!(String::from_utf8_lossy(&child.stdout).contains("1 passed"));
    drop(lock);
    ContentLock::acquire(&path).unwrap();
}

#[test]
fn content_lock_probe() {
    if let Some(path) = std::env::var_os("RMCL_TEST_CONTENT_MANIFEST") {
        assert!(matches!(
            ContentLock::acquire(Path::new(&path)),
            Err(ManifestError::Busy(_))
        ));
    }
}

#[test]
fn guarded_publication_rejects_an_uncooperative_manifest_change() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("manifest.json");
    let previous = ContentManifest::default();
    previous.save(&path).unwrap();
    let lock = ContentLock::acquire(&path).unwrap();
    std::fs::write(&path, br#"{"version":1,"files":[{"relative_path":"mods/foreign.jar","kind":"mod","enabled":true,"fingerprint":{"size":1,"modified_ns":0,"hashes":{}}}]}"#).unwrap();
    assert!(matches!(
        lock.save_if_unchanged(&previous, &previous),
        Err(ManifestError::Changed(_))
    ));
    assert_eq!(
        ContentManifest::load(&path).unwrap().files[0].relative_path,
        Path::new("mods/foreign.jar")
    );
}

#[test]
fn loading_rejects_duplicate_and_escaping_ownership_paths() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("manifest.json");
    for (relative, copies) in [("../escape.jar", 1), ("mods/a.jar", 2)] {
        let record = serde_json::json!({
            "relative_path": relative, "kind": "mod", "enabled": true,
            "fingerprint": { "size": 0, "modified_ns": 0, "hashes": {} },
        });
        let files = vec![record; copies];
        let manifest = serde_json::json!({ "version": 1, "files": files });
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        assert!(matches!(
            ContentManifest::load(&path),
            Err(ManifestError::InvalidPath(_))
        ));
    }
}

#[test]
fn unchanged_manifest_updates_do_not_rewrite_the_saved_index() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("manifest.json");
    ContentManifest::default().save(&path).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(UNIX_EPOCH))
        .unwrap();
    let before = std::fs::metadata(&path).unwrap().modified().unwrap();
    ContentManifest::update(&path, |_| Ok(())).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        before
    );
}

#[test]
fn curseforge_fingerprint_ignores_whitespace() {
    let temp = tempfile::tempdir().unwrap();
    let compact = temp.path().join("compact.jar");
    let spaced = temp.path().join("spaced.jar");
    std::fs::write(&compact, b"abc").unwrap();
    std::fs::write(&spaced, b"a b\nc\r\t").unwrap();
    let compact = fingerprint(&compact).unwrap();
    assert_eq!(compact.hash("curseforge"), Some("1621425345"));
    assert_eq!(
        compact.hash("curseforge"),
        fingerprint(&spaced).unwrap().hash("curseforge")
    );
}

#[test]
fn manifest_round_trip_and_lookup_are_exact() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("instance/rmcl/content/manifest.json");
    let record = ContentFileRecord {
        relative_path: PathBuf::from("mods/fabric-api.jar"),
        kind: ContentKind::Mod,
        enabled: true,
        fingerprint: FileFingerprint {
            size: 3,
            modified_ns: 4,
            hashes: BTreeMap::from([("sha1".to_owned(), "abc".to_owned())]),
        },
        resolution: Resolution::Resolved {
            project: ProviderProject {
                provider: "modrinth".to_owned(),
                project_id: "P7dR8mSH".to_owned(),
                version_id: "version".to_owned(),
            },
        },
        provider_aliases: Vec::new(),
        provider_checks: Vec::new(),
        required_dependencies: Vec::new(),
        automatic_dependency: false,
        cleanup_eligible: false,
    };
    let mut manifest = ContentManifest::default();
    manifest.upsert(record);
    manifest.save(&path).unwrap();

    let loaded = ContentManifest::load(&path).unwrap();
    assert_eq!(
        loaded.resolved_project_path("modrinth", "P7dR8mSH", Path::new("/instance/minecraft")),
        Some(PathBuf::from("/instance/minecraft/mods/fabric-api.jar"))
    );
    assert!(
        loaded
            .resolved_project_path("modrinth", "fabric-api", Path::new("/minecraft"))
            .is_none()
    );
}

#[test]
fn fingerprint_contains_both_modrinth_hashes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("example.jar");
    std::fs::write(&path, b"abc").unwrap();
    let fingerprint = fingerprint(&path).unwrap();
    assert_eq!(
        fingerprint.hash("sha1"),
        Some("a9993e364706816aba3e25717850c26c9cd0d89d")
    );
    assert_eq!(fingerprint.hash("sha512").unwrap().len(), 128);
}

#[test]
fn fingerprint_checks_cancellation_during_both_hashing_passes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("example.jar");
    std::fs::write(&path, [b'x'; 64 * 1024 + 1]).unwrap();
    for cancel_at in [3, 6] {
        let calls = std::cell::Cell::new(0);
        let error = fingerprint_with_check(&path, || {
            calls.set(calls.get() + 1);
            if calls.get() == cancel_at {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "cancelled",
                ))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(calls.get(), cancel_at);
    }
}

#[test]
fn renaming_a_record_preserves_its_resolution() {
    let mut manifest = ContentManifest::default();
    manifest.upsert(ContentFileRecord {
        relative_path: PathBuf::from("mods/example.jar"),
        kind: ContentKind::Mod,
        enabled: true,
        fingerprint: FileFingerprint {
            size: 3,
            modified_ns: 4,
            hashes: BTreeMap::new(),
        },
        resolution: Resolution::Resolved {
            project: ProviderProject {
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

    assert!(manifest.rename_record(
        Path::new("mods/example.jar"),
        Path::new("mods/example.jar.disabled"),
        false,
    ));
    let record = manifest
        .record(Path::new("mods/example.jar.disabled"))
        .unwrap();
    assert!(!record.enabled);
    assert!(matches!(record.resolution, Resolution::Resolved { .. }));
}

fn managed_record(
    path: &str,
    project_id: &str,
    automatic_dependency: bool,
    required_dependencies: Vec<ProviderProject>,
) -> ContentFileRecord {
    ContentFileRecord {
        relative_path: PathBuf::from(path),
        kind: ContentKind::Mod,
        enabled: true,
        fingerprint: FileFingerprint {
            size: 1,
            modified_ns: 1,
            hashes: BTreeMap::new(),
        },
        resolution: Resolution::Resolved {
            project: ProviderProject {
                provider: "modrinth".to_owned(),
                project_id: project_id.to_owned(),
                version_id: format!("{project_id}-version"),
            },
        },
        provider_aliases: Vec::new(),
        provider_checks: Vec::new(),
        required_dependencies,
        automatic_dependency,
        cleanup_eligible: automatic_dependency,
    }
}

fn dependency(project_id: &str) -> ProviderProject {
    ProviderProject {
        provider: "modrinth".to_owned(),
        project_id: project_id.to_owned(),
        version_id: format!("{project_id}-version"),
    }
}

#[test]
fn provider_aliases_match_the_same_installed_record() {
    let mut record = managed_record("mods/library.jar", "curseforge-library", false, Vec::new());
    record.provider_aliases.push(ProviderProject {
        provider: "modrinth".to_owned(),
        project_id: "modrinth-library".to_owned(),
        version_id: "modrinth-version".to_owned(),
    });
    let manifest = ContentManifest {
        version: 1,
        files: vec![record],
    };

    assert!(
        manifest
            .resolved_project_record("modrinth", "modrinth-library")
            .is_some()
    );
}

#[test]
fn orphan_cleanup_keeps_shared_and_user_managed_libraries() {
    let manifest = ContentManifest {
        version: 1,
        files: vec![
            managed_record("mods/first.jar", "first", false, vec![dependency("shared")]),
            managed_record(
                "mods/second.jar",
                "second",
                false,
                vec![dependency("shared"), dependency("explicit")],
            ),
            managed_record("mods/shared.jar", "shared", true, Vec::new()),
            managed_record("mods/explicit.jar", "explicit", false, Vec::new()),
        ],
    };

    assert_eq!(
        manifest.orphaned_dependencies_after_removing(Path::new("mods/first.jar")),
        Vec::<PathBuf>::new()
    );
    assert_eq!(
        manifest.orphaned_dependencies_after_removing(Path::new("mods/second.jar")),
        Vec::<PathBuf>::new()
    );
}

#[test]
fn orphan_cleanup_follows_automatic_dependency_chains() {
    let manifest = ContentManifest {
        version: 1,
        files: vec![
            managed_record("mods/root.jar", "root", false, vec![dependency("library")]),
            managed_record(
                "mods/library.jar",
                "library",
                true,
                vec![dependency("nested")],
            ),
            managed_record("mods/nested.jar", "nested", true, Vec::new()),
        ],
    };

    assert_eq!(
        manifest.orphaned_dependencies_after_removing(Path::new("mods/root.jar")),
        vec![
            PathBuf::from("mods/library.jar"),
            PathBuf::from("mods/nested.jar")
        ]
    );
}

#[test]
fn orphan_cleanup_keeps_automatic_non_library_dependencies() {
    let mut dependency_record = managed_record("mods/sodium.jar", "sodium", true, Vec::new());
    dependency_record.cleanup_eligible = false;
    let manifest = ContentManifest {
        version: 1,
        files: vec![
            managed_record("mods/root.jar", "root", false, vec![dependency("sodium")]),
            dependency_record,
        ],
    };

    assert!(
        manifest
            .orphaned_dependencies_after_removing(Path::new("mods/root.jar"))
            .is_empty()
    );
}

#[test]
fn datapack_dependencies_are_scoped_to_their_world() {
    let mut first_root = managed_record(
        "saves/first/datapacks/root.zip",
        "root",
        false,
        vec![dependency("library")],
    );
    first_root.kind = ContentKind::DataPack;
    let mut second_root = managed_record(
        "saves/second/datapacks/root.zip",
        "root",
        false,
        vec![dependency("library")],
    );
    second_root.kind = ContentKind::DataPack;
    let mut first_library = managed_record(
        "saves/first/datapacks/library.zip",
        "library",
        true,
        Vec::new(),
    );
    first_library.kind = ContentKind::DataPack;
    let mut second_library = managed_record(
        "saves/second/datapacks/library.zip",
        "library",
        true,
        Vec::new(),
    );
    second_library.kind = ContentKind::DataPack;
    let manifest = ContentManifest {
        version: 1,
        files: vec![first_root, second_root, first_library, second_library],
    };

    assert_eq!(
        manifest.orphaned_dependencies_after_removing(Path::new("saves/first/datapacks/root.zip")),
        vec![PathBuf::from("saves/first/datapacks/library.zip")]
    );
}
