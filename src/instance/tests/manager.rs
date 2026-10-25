// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;
use crate::instance::models::ModLoader;
use tempfile::TempDir;

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
fn instance_names_are_portable_components() {
    for name in [
        "C:victim",
        "C:",
        "a:b",
        "CON",
        "con.txt",
        "CON .txt",
        "AUX.jar",
        "NUL",
        "PRN",
        "COM1",
        "lpt9.log",
        "COM¹",
        "LPT².txt",
        "CLOCK$",
        "CONIN$",
        "CONOUT$",
        "trailing.",
        "trailing ",
        "bad?name",
        "bad*name",
        "bad\"name",
        "bad<name",
        "bad>name",
        "bad|name",
        r"path\name",
        ".",
        "..",
    ] {
        assert!(validate_name(name).is_err(), "{name}");
    }
    for name in ["CONifer", "COM0", "COM10", "世界", "%USERNAME%", "My Pack"] {
        assert!(validate_name(name).is_ok(), "{name}");
    }
}

#[tokio::test]
async fn all_manager_entry_points_reject_nonportable_names_before_io() {
    let temp = tempfile::tempdir().unwrap();
    let manager = InstanceManager::new(temp.path().join("instances"), temp.path().join("meta"));
    for name in ["C:victim", "CON.txt", "trailing.", "bad?name"] {
        let config = dummy_config(name);
        assert!(matches!(
            manager.create(name, "1.21", ModLoader::Vanilla, None).await,
            Err(InstanceError::InvalidName(_))
        ));
        assert!(matches!(
            manager.repair_runtime_cache(&config).await,
            Err(InstanceError::InvalidName(_))
        ));
        assert!(matches!(
            manager.delete(name),
            Err(InstanceError::InvalidName(_))
        ));
        assert!(matches!(
            manager.rename(name, "safe"),
            Err(InstanceError::InvalidName(_))
        ));
        assert!(matches!(
            manager.rename("safe", name),
            Err(InstanceError::InvalidName(_))
        ));
        assert!(matches!(
            manager.load_one(name),
            Err(InstanceError::InvalidName(_))
        ));
        assert!(matches!(
            manager.save(&config),
            Err(InstanceError::InvalidName(_))
        ));
        assert!(matches!(
            manager.save_config_sync_profile(name, Some("shared".to_owned())),
            Err(InstanceError::InvalidName(_))
        ));
        assert!(matches!(
            manager.touch_last_played(name),
            Err(InstanceError::InvalidName(_))
        ));
        assert!(matches!(
            crate::instance::runtime::lock_instance(&manager.instances_dir, name),
            Err(InstanceError::InvalidName(_))
        ));
    }
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
}

#[cfg(windows)]
#[test]
fn drive_relative_deletion_preserves_an_external_directory() {
    const ROOT: &str = "RMCL_NAME_BOUNDARY_ROOT";
    if let Some(root) = std::env::var_os(ROOT) {
        let cwd = std::env::current_dir().unwrap();
        let std::path::Component::Prefix(prefix) = cwd.components().next().unwrap() else {
            panic!("Expected a Windows drive");
        };
        let drive = match prefix.kind() {
            std::path::Prefix::Disk(drive) | std::path::Prefix::VerbatimDisk(drive) => drive,
            other => panic!("Expected a local drive, got {other:?}"),
        };
        let name = format!("{}:victim", char::from(drive));
        let manager = InstanceManager::new(&root, cwd.join("meta"));
        assert!(matches!(
            manager.delete(&name),
            Err(InstanceError::InvalidName(_))
        ));
        assert_eq!(std::fs::read(cwd.join("victim/keep")).unwrap(), b"keep");
        assert!(!cwd.join("victim.lock").exists());
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("outside");
    std::fs::create_dir_all(cwd.join("victim")).unwrap();
    std::fs::write(cwd.join("victim/keep"), b"keep").unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "instance::manager::tests::drive_relative_deletion_preserves_an_external_directory",
            "--nocapture",
        ])
        .env(ROOT, temp.path().join("instances"))
        .current_dir(&cwd)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(std::fs::read(cwd.join("victim/keep")).unwrap(), b"keep");
}

#[test]
fn delete_missing_instance_returns_not_found() {
    let (manager, _tmp) = test_manager();
    let result = manager.delete("ghost-instance");
    assert!(matches!(result, Err(InstanceError::NotFound(_))));
}

#[test]
fn running_instance_cannot_be_deleted_or_renamed() {
    let (manager, tmp) = test_manager();
    let name = format!("running-{}", std::process::id());
    std::fs::create_dir_all(tmp.path().join(&name)).unwrap();
    manager.save(&dummy_config(&name)).unwrap();
    crate::instance::runtime::set_state(&name, crate::instance::runtime::RunState::Running);

    let deletion = manager.delete(&name);
    let rename = manager.rename(&name, "changed");
    crate::instance::runtime::remove(&name);

    assert!(matches!(deletion, Err(InstanceError::InstanceRunning(_))));
    assert!(matches!(rename, Err(InstanceError::InstanceRunning(_))));
    assert!(tmp.path().join(&name).exists());
    assert!(!tmp.path().join("changed").exists());
}

#[test]
fn instance_lock_child() {
    use std::io::Write;
    let Some(root) = std::env::var_os("RMCL_INSTANCE_LOCK_TEST") else {
        return;
    };
    let mut lock =
        crate::instance::runtime::lock_instance(std::path::Path::new(&root), "locked").unwrap();
    if std::env::var("RMCL_ALLOW_CONTENT_TEST").as_deref() == Ok("1") {
        lock.allow_content_changes().unwrap();
    }
    println!("LOCKED");
    std::io::stdout().flush().unwrap();
    std::thread::sleep(std::time::Duration::from_secs(10));
}

#[test]
fn another_process_protects_the_instance_from_mutation_and_duplicate_launch() {
    use std::io::BufRead;
    let (manager, temp) = test_manager();
    std::fs::create_dir_all(temp.path().join("locked")).unwrap();
    manager.save(&dummy_config("locked")).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "instance::manager::tests::instance_lock_child",
            "--nocapture",
        ])
        .env("RMCL_INSTANCE_LOCK_TEST", temp.path())
        .env("RMCL_ALLOW_CONTENT_TEST", "1")
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let ready = std::io::BufReader::new(child.stdout.take().unwrap())
        .lines()
        .any(|line| line.unwrap() == "LOCKED");
    let deletion = manager.delete("locked");
    let rename = manager.rename("locked", "new");
    let launch = crate::instance::runtime::lock_instance(&manager.instances_dir, "locked");
    let content_allowed =
        crate::instance::runtime::lock_content(&manager.instances_dir, "locked").is_ok();
    let _ = child.kill();
    child.wait().unwrap();
    assert!(ready);
    assert!(content_allowed);
    assert!(matches!(deletion, Err(InstanceError::InstanceRunning(_))));
    assert!(matches!(rename, Err(InstanceError::InstanceRunning(_))));
    assert!(matches!(launch, Err(InstanceError::InstanceRunning(_))));
    assert!(temp.path().join("locked").is_dir());
    manager.delete("locked").unwrap();
}

#[test]
fn copied_instance_uses_its_directory_as_identity() {
    let (manager, tmp) = test_manager();
    std::fs::create_dir_all(tmp.path().join("original")).unwrap();
    manager.save(&dummy_config("original")).unwrap();
    std::fs::create_dir_all(tmp.path().join("copy")).unwrap();
    std::fs::copy(
        tmp.path().join("original/instance.json"),
        tmp.path().join("copy/instance.json"),
    )
    .unwrap();

    assert_eq!(manager.load_one("copy").unwrap().name, "copy");
    assert!(
        manager
            .load_all()
            .iter()
            .any(|config| config.name == "copy")
    );
}

#[test]
fn configuration_locks_survive_instance_directory_replacement() {
    let (manager, temp) = test_manager();
    let name = "replaced";
    std::fs::create_dir(temp.path().join(name)).unwrap();
    manager.save(&dummy_config(name)).unwrap();
    let held = manager.config_lock(name).unwrap();
    std::fs::rename(temp.path().join(name), temp.path().join("backup")).unwrap();
    std::fs::create_dir(temp.path().join(name)).unwrap();
    let root = manager.instances_dir.clone();
    let meta = manager.meta_dir.clone();
    let (sender, receiver) = std::sync::mpsc::channel();
    let contender = std::thread::spawn(move || {
        let manager = InstanceManager::new(root, meta);
        sender.send("attempting").unwrap();
        let _held = manager.config_lock(name).unwrap();
        sender.send("acquired").unwrap();
    });
    assert_eq!(
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap(),
        "attempting"
    );
    let while_locked = receiver.recv_timeout(std::time::Duration::from_millis(100));
    drop(held);
    assert!(matches!(
        while_locked,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout)
    ));
    assert_eq!(
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap(),
        "acquired"
    );
    contender.join().unwrap();
}

#[tokio::test]
async fn create_preserves_directory_without_instance_config() {
    let (manager, tmp) = test_manager();
    let dir = tmp.path().join("existing");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("world.zip"), b"keep").unwrap();

    let result = manager
        .create("existing", "1.20.1", ModLoader::Vanilla, None)
        .await;

    assert!(matches!(result, Err(InstanceError::AlreadyExists(_))));
    assert_eq!(std::fs::read(dir.join("world.zip")).unwrap(), b"keep");
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
fn case_only_rename_uses_the_actual_volume_and_updates_directory_spelling() {
    let (manager, temp) = test_manager();
    let old = format!(
        "CasePack-{}",
        temp.path().file_name().unwrap().to_str().unwrap()
    );
    let new = old.to_lowercase();
    std::fs::create_dir(temp.path().join(&old)).unwrap();
    manager.save(&dummy_config(&old)).unwrap();
    let aliases = temp.path().join(&new).exists();
    assert_eq!(
        paths_alias(&temp.path().join(&old), &temp.path().join(&new)).unwrap(),
        aliases
    );
    manager.rename(&old, &new).unwrap();
    let names: Vec<_> = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert!(names.iter().any(|name| name.to_str() == Some(new.as_str())));
    assert!(!names.iter().any(|name| name.to_str() == Some(old.as_str())));
    let raw: InstanceConfig = serde_json::from_slice(
        &std::fs::read(temp.path().join(&new).join("instance.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(raw.name, new);
    if !aliases {
        std::fs::create_dir(temp.path().join(&old)).unwrap();
        manager.save(&dummy_config(&old)).unwrap();
        assert!(matches!(
            manager.rename(&new, &old),
            Err(InstanceError::AlreadyExists(_))
        ));
    }
}

#[test]
fn temporary_rename_rolls_back_when_the_second_step_fails() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("Pack");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("keep"), b"keep").unwrap();
    assert!(rename_path(&source, &temp.path().join("missing/pack"), true).is_err());
    assert_eq!(std::fs::read(source.join("keep")).unwrap(), b"keep");
    assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
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

#[test]
fn ordinary_settings_saves_preserve_profile_selection_and_malformed_config() {
    let (manager, temp) = test_manager();
    let name = "settings";
    std::fs::create_dir(temp.path().join(name)).unwrap();
    let mut stale = dummy_config(name);
    manager.save(&stale).unwrap();
    manager
        .save_config_sync_profile(name, Some("shared".to_owned()))
        .unwrap();
    stale.memory_max = Some("8G".to_owned());
    manager.save(&stale).unwrap();
    let current = manager.load_one(name).unwrap();
    assert_eq!(current.config_sync_profile.as_deref(), Some("shared"));
    assert_eq!(current.memory_max.as_deref(), Some("8G"));
    let path = temp.path().join(name).join("instance.json");
    std::fs::write(&path, b"invalid json").unwrap();
    assert!(matches!(manager.save(&stale), Err(InstanceError::Json(_))));
    assert_eq!(std::fs::read(path).unwrap(), b"invalid json");
}

#[test]
fn config_sync_profile_save_preserves_authoritative_settings_and_last_played() {
    let (manager, temp) = test_manager();
    let name = "sync-profile";
    let directory = temp.path().join(name);
    std::fs::create_dir(&directory).unwrap();
    manager.save(&dummy_config(name)).unwrap();
    let staged = manager.load_one(name).unwrap();

    let mut latest = staged.clone();
    latest.config_sync_profile = Some("previous".to_owned());
    latest.memory_max = Some("8G".to_owned());
    latest.jvm_args.push("-Dlocal.setting=new".to_owned());
    latest
        .environment
        .insert("LOCAL_SETTING".to_owned(), "new".to_owned());
    manager.save(&latest).unwrap();
    manager
        .save_config_sync_profile(name, latest.config_sync_profile.clone())
        .unwrap();
    manager.touch_last_played(name).unwrap();
    let mut expected = manager.load_one(name).unwrap();
    let before: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.join("instance.json")).unwrap()).unwrap();

    for profile in [Some("shared".to_owned()), None] {
        expected.config_sync_profile = profile.clone();
        let saved = manager
            .save_config_sync_profile(&staged.name, profile)
            .unwrap();
        assert_eq!(saved, expected);
        assert_eq!(manager.load_one(name).unwrap(), expected);
        let after: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join("instance.json")).unwrap())
                .unwrap();
        assert_eq!(after["last_played"], before["last_played"]);
    }

    std::fs::write(directory.join("instance.json"), b"invalid json").unwrap();
    assert!(matches!(
        manager.save_config_sync_profile(name, Some("shared".to_owned())),
        Err(InstanceError::Json(_))
    ));
    assert_eq!(
        std::fs::read(directory.join("instance.json")).unwrap(),
        b"invalid json"
    );
}

#[cfg(unix)]
#[test]
fn runtime_repair_uses_selected_java_cwd_environment_and_platform() {
    use sha1::{Digest, Sha1};
    use std::collections::BTreeMap;
    use std::io::{Cursor, Write};
    use std::os::unix::fs::PermissionsExt;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const ROOT: &str = "RMCL_RUNTIME_CONTEXT_TEST";
    let Some(root) = std::env::var_os(ROOT) else {
        let temp =
            tempfile::tempdir_in(Path::new(env!("CARGO_MANIFEST_DIR")).join("target")).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "instance::manager::tests::runtime_repair_uses_selected_java_cwd_environment_and_platform",
                "--nocapture",
            ])
            .env(ROOT, temp.path())
            .env("HOME", temp.path())
            .env("XDG_CONFIG_HOME", temp.path().join("config"))
            .current_dir(temp.path())
            .status()
            .unwrap();
        assert!(status.success());
        return;
    };
    let root = PathBuf::from(root);
    let config_dir = crate::config::get_config_path();
    assert!(config_dir.starts_with(&root));
    std::fs::create_dir_all(&config_dir).unwrap();
    let mut settings = crate::config::Config::default();
    settings.paths.java_path = Some("context-java".to_owned());
    settings.defaults.environment = BTreeMap::from([
        ("PATH".to_owned(), "global-bin".to_owned()),
        ("JAVA_HOME".to_owned(), "missing-home".to_owned()),
        ("RUNTIME_GLOBAL".to_owned(), "global-only".to_owned()),
        ("RUNTIME_SHARED".to_owned(), "global".to_owned()),
    ]);
    std::fs::write(
        config_dir.join("config.toml"),
        toml::to_string(&settings).unwrap(),
    )
    .unwrap();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let server = MockServer::start().await;
        let client_jar = b"client";
        let assets = br#"{"objects":{}}"#;
        for (endpoint, body) in [("/client.jar", client_jar.as_slice()), ("/assets.json", assets.as_slice())] {
            Mock::given(method("GET"))
                .and(path(endpoint))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(body.to_vec()))
                .expect(3)
                .mount(&server).await;
        }
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive.start_file("native.bin", zip::write::SimpleFileOptions::default()).unwrap();
        archive.write_all(b"selected-native").unwrap();
        let native = archive.finish().unwrap().into_inner();
        let hash = |bytes: &[u8]| format!("{:x}", Sha1::digest(bytes));
        let manager = InstanceManager::new("instances", "meta");
        let metadata = crate::storage::MetadataPaths::new(&manager.meta_dir);

        for (name, os, java_os, java_arch, arch, bits) in [
            ("global", "linux", "Linux", "aarch64", "arm64", 64),
            ("override", "windows", "Windows 11", "i386", "x86", 32),
            ("relative", "osx", "Mac OS X", "amd64", "x86_64", 64),
        ] {
            let mut config = dummy_config(name);
            config.game_version = name.to_owned();
            let minecraft = root.join("instances").join(name).join(crate::storage::MINECRAFT_DIR_NAME);
            let bin = if name == "global" { "global-bin" } else { "instance-bin" };
            let java = minecraft.join(bin).join("context-java");
            std::fs::create_dir_all(java.parent().unwrap()).unwrap();
            std::fs::write(&java, "#!/bin/sh\nprintf '%s\\n' \"$0\" > probe-java.txt\npwd > probe-cwd.txt\nprintf '%s\\n' \"$RUNTIME_GLOBAL\" \"$RUNTIME_SHARED\" > probe-env.txt\nprintf 'java.version = 25\\nos.name = %s\\nos.arch = %s\\nsun.arch.data.model = %s\\nos.version = fixture-os\\n' \"$RUNTIME_OS\" \"$RUNTIME_ARCH\" \"$RUNTIME_BITS\" >&2\n").unwrap();
            std::fs::set_permissions(&java, std::fs::Permissions::from_mode(0o755)).unwrap();
            config.environment = BTreeMap::from([
                ("RUNTIME_OS".to_owned(), java_os.to_owned()),
                ("RUNTIME_ARCH".to_owned(), java_arch.to_owned()),
                ("RUNTIME_BITS".to_owned(), bits.to_string()),
                ("RUNTIME_SHARED".to_owned(), "instance".to_owned()),
            ]);
            if name != "global" {
                config.environment.insert("PATH".to_owned(), bin.to_owned());
            }
            if name == "relative" {
                config.java_path = Some("./instance-bin/context-java".to_owned());
            }

            let mut libraries = Vec::new();
            for (library_os, library_arch) in [("linux", "arm64"), ("windows", "x86"), ("osx", "x86_64")] {
                let mut classifiers = serde_json::Map::new();
                for native_bits in [32, 64] {
                    let endpoint = format!("/{name}-{library_os}-{native_bits}.jar");
                    Mock::given(method("GET"))
                        .and(path(&endpoint))
                        .respond_with(ResponseTemplate::new(200).set_body_bytes(native.clone()))
                        .expect(u64::from(library_os == os && native_bits == bits))
                        .mount(&server).await;
                    classifiers.insert(format!("natives-{native_bits}"), serde_json::json!({
                        "url": format!("{}{endpoint}", server.uri()),
                        "path": format!("{name}/{library_os}-{native_bits}.jar"),
                        "sha1": hash(&native), "size": native.len()
                    }));
                }
                libraries.push(serde_json::json!({
                    "name": format!("example:{library_os}:1"),
                    "rules": [{ "action": "allow", "os": { "name": library_os, "arch": library_arch, "version": "^fixture-os$" } }],
                    "natives": { (library_os): "natives-${arch}" },
                    "downloads": { "classifiers": classifiers }
                }));
            }
            let version_dir = metadata.versions().join(name);
            std::fs::create_dir_all(&version_dir).unwrap();
            std::fs::write(version_dir.join("meta.json"), serde_json::to_vec(&serde_json::json!({
                "id": name, "mainClass": "fixture.Main", "arguments": { "game": [], "jvm": [] },
                "assetIndex": { "id": name, "url": format!("{}/assets.json", server.uri()), "sha1": hash(assets) },
                "downloads": { "client": { "url": format!("{}/client.jar", server.uri()), "sha1": hash(client_jar), "size": client_jar.len() } },
                "libraries": libraries
            })).unwrap()).unwrap();

            manager.repair_runtime_cache(&config).await.unwrap();
            let selected_java = std::fs::read_to_string(minecraft.join("probe-java.txt")).unwrap();
            assert_eq!(Path::new(selected_java.trim()), java);
            let cwd = std::fs::read_to_string(minecraft.join("probe-cwd.txt")).unwrap();
            assert_eq!(Path::new(cwd.trim()).canonicalize().unwrap(), minecraft.canonicalize().unwrap());
            assert_eq!(std::fs::read_to_string(minecraft.join("probe-env.txt")).unwrap(), "global-only\ninstance\n");
            assert_eq!(std::fs::read(version_dir.join("natives").join(format!("{os}-{arch}-{bits}")).join("native.bin")).unwrap(), b"selected-native");
            for (library_os, _) in [("linux", "arm64"), ("windows", "x86"), ("osx", "x86_64")] {
                for native_bits in [32, 64] {
                    assert_eq!(metadata.libraries().join(name).join(format!("{library_os}-{native_bits}.jar")).exists(), library_os == os && native_bits == bits);
                }
            }
        }
    });
}
