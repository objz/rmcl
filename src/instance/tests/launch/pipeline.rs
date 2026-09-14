// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[rstest::rstest]
#[case("openjdk version \"25.0.3\" 2026-04-21", Some(25))]
#[case("openjdk version \"21.0.11\" 2026-04-21", Some(21))]
#[case("java version \"1.8.0_402\"", Some(8))]
#[case("garbage", None)]
fn parse_java_major_version_handles_common_outputs(
    #[case] output: &str,
    #[case] expected: Option<u32>,
) {
    assert_eq!(
        crate::instance::java::parse_java_major_version(output),
        expected
    );
}

#[test]
fn quick_play_world_must_be_a_direct_save_directory() {
    let temp = tempfile::tempdir().unwrap();
    let minecraft = temp.path().join("minecraft");
    std::fs::create_dir_all(minecraft.join("saves/World")).unwrap();

    assert!(validate_quick_play_world(&minecraft, "World").is_ok());
    assert!(validate_quick_play_world(&minecraft, "../World").is_err());
    assert!(validate_quick_play_world(&minecraft, "Missing").is_err());
}

#[test]
fn build_game_args_renders_upstream_arguments() {
    use crate::launch_profile::model::{Argument, Arguments, LaunchProfile};
    use crate::launch_profile::rules::{FeatureSet, RuleContext};
    use TemplateContext;
    use std::path::PathBuf;

    let lib = PathBuf::from("/m/libraries");
    let nat = PathBuf::from("/m/natives");
    let game_dir = PathBuf::from("/i/.minecraft");
    let assets = PathBuf::from("/m/assets");

    let template_ctx = TemplateContext {
        library_directory: &lib,
        classpath_separator: ":",
        version_name: "1.20.1",
        natives_directory: &nat,
        classpath: "a.jar:b.jar",
        game_directory: &game_dir,
        assets_root: &assets,
        assets_index_name: "5",
        auth_player_name: "Player",
        auth_uuid: "00000000-0000-0000-0000-000000000000",
        auth_access_token: "token",
        auth_xuid: "0",
        user_type: "msa",
        user_properties: "{}",
        launcher_name: "rmcl",
        launcher_version: "test",
        clientid: "0",
        quick_play_singleplayer: None,
        resolution_width: None,
        resolution_height: None,
        version_type: "release",
    };
    let features = FeatureSet::default();
    let rule_ctx = RuleContext {
        os_name: "linux",
        os_version: "6.0",
        arch: "x86_64",
        features: &features,
    };

    let profile = LaunchProfile {
        id: "1.20.1".into(),
        inherits_from: None,
        main_class: Some("net.minecraft.client.main.Main".into()),
        libraries: Vec::new(),
        arguments: Some(Arguments {
            game: vec![
                Argument::Literal("--username".into()),
                Argument::Literal("${auth_player_name}".into()),
            ],
            jvm: vec![Argument::Literal(
                "-Djava.library.path=${natives_directory}".into(),
            )],
        }),
        ..Default::default()
    };

    let (jvm, game_args) = build_game_args(&profile, &rule_ctx, &template_ctx).unwrap();
    assert_eq!(jvm, vec!["-Djava.library.path=/m/natives"]);
    assert_eq!(game_args, vec!["--username", "Player"]);
}

#[test]
fn custom_resolution_is_added_once() {
    let mut args = vec!["--username".to_owned(), "Player".to_owned()];
    apply_custom_resolution(&mut args, Some((1920, 1080)));
    assert_eq!(
        args,
        [
            "--username",
            "Player",
            "--width",
            "1920",
            "--height",
            "1080"
        ]
    );

    apply_custom_resolution(&mut args, Some((1280, 720)));
    assert_eq!(args.iter().filter(|arg| *arg == "--width").count(), 1);
    assert_eq!(args.iter().filter(|arg| *arg == "--height").count(), 1);
}

#[test]
fn fullscreen_is_added_once() {
    let mut args = vec!["--username".to_owned(), "Player".to_owned()];
    apply_window_mode(&mut args, WindowMode::Fullscreen);
    apply_window_mode(&mut args, WindowMode::Fullscreen);
    assert_eq!(
        args.iter()
            .filter(|argument| *argument == "--fullscreen")
            .count(),
        1
    );

    let mut windowed = vec![
        "--username".to_owned(),
        "Player".to_owned(),
        "--fullscreen".to_owned(),
    ];
    apply_window_mode(&mut windowed, WindowMode::Windowed);
    assert_eq!(windowed, ["--username", "Player"]);
}

#[test]
fn preferred_account_falls_back_to_the_active_account() {
    let accounts = vec![
        crate::auth::Account {
            uuid: "active".to_owned(),
            username: "Active".to_owned(),
            account_type: AccountType::Microsoft,
            active: true,
            refresh_token: None,
            cached_mc_token: None,
            cached_mc_token_expires_at: None,
        },
        crate::auth::Account {
            uuid: "preferred".to_owned(),
            username: "Preferred".to_owned(),
            account_type: AccountType::Offline,
            active: false,
            refresh_token: None,
            cached_mc_token: None,
            cached_mc_token_expires_at: None,
        },
    ];

    assert_eq!(
        select_launch_account(&accounts, Some("preferred")).map(|account| account.uuid.as_str()),
        Some("preferred")
    );
    assert_eq!(
        select_launch_account(&accounts, Some("removed")).map(|account| account.uuid.as_str()),
        Some("active")
    );
    assert_eq!(
        select_launch_account(&accounts, None).map(|account| account.uuid.as_str()),
        Some("active")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn launch_commands_receive_instance_environment() {
    use chrono::Utc;

    let temp = tempfile::tempdir().unwrap();
    let minecraft = temp.path().join("minecraft");
    std::fs::create_dir_all(&minecraft).unwrap();
    let mut config = InstanceConfig {
        name: "Hook Test".to_owned(),
        game_version: "1.21.1".to_owned(),
        loader: ModLoader::Vanilla,
        loader_version: None,
        created: Utc::now(),
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
    };
    config
        .environment
        .insert("CUSTOM_VALUE".to_owned(), "available".to_owned());
    let invocation = LaunchInvocation {
        java: "/usr/bin/java".to_owned(),
        jvm_args: vec!["-Xmx2G".to_owned()],
        classpath: Vec::new(),
        classpath_string: String::new(),
        main_class: String::new(),
        extra_args: Vec::new(),
        game_args: Vec::new(),
        environment: config.environment.clone(),
        working_dir: minecraft.clone(),
    };
    let command = LaunchCommand {
        enabled: true,
        command: "printf '%s|%s' \"$CUSTOM_VALUE\" \"$INST_NAME\" > hook-result".to_owned(),
    };

    run_launch_command("Pre-launch", &command, &config, &invocation, temp.path())
        .await
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(minecraft.join("hook-result")).unwrap(),
        "available|Hook Test"
    );
}

// exercises the early-return branch of migrate_legacy_meta_if_needed.
// a profile with either arguments or minecraftArguments is not legacy
// and must produce Ok(None) without touching the network. covers both
// shapes in one parameterised test so a regression that drops one of
// the two predicate conditions is caught.
#[rstest::rstest]
#[case::modern_arguments(true, false)]
#[case::legacy_minecraft_arguments(false, true)]
#[tokio::test]
async fn migrate_legacy_meta_skips_when_arguments_present(
    #[case] modern: bool,
    #[case] legacy: bool,
) {
    use crate::launch_profile::model::{Arguments, LaunchProfile};
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let meta_path = tmp.path().join("meta.json");
    std::fs::write(&meta_path, b"{}").unwrap();

    let profile = LaunchProfile {
        id: "1.20.1".into(),
        main_class: Some("net.test.Main".into()),
        arguments: modern.then(Arguments::default),
        minecraft_arguments: legacy.then(|| "--username Player".into()),
        ..Default::default()
    };

    let result = migrate_legacy_meta_if_needed(&meta_path, &profile, "1.20.1").await;
    assert!(
        matches!(result, Ok(None)),
        "expected Ok(None) for non-legacy profile, got {result:?}"
    );
}

// each loader maps to a distinct directory-naming branch. one rstest
// exercises every variant so a regression that misorders the match
// arms in installer_version_dir_name is caught.
#[rstest::rstest]
#[case::forge(ModLoader::Forge, "1.20.1", "47.2.0", Some("1.20.1-forge-47.2.0"))]
#[case::neoforge(ModLoader::NeoForge, "1.21.1", "21.1.0", Some("neoforge-21.1.0"))]
#[case::vanilla(ModLoader::Vanilla, "1.20.1", "v", None)]
#[case::fabric(ModLoader::Fabric, "1.20.1", "0.14.21", None)]
#[case::quilt(ModLoader::Quilt, "1.20.1", "0.20.0", None)]
fn installer_version_dir_name_per_loader(
    #[case] loader: ModLoader,
    #[case] game_version: &str,
    #[case] loader_version: &str,
    #[case] expected: Option<&str>,
) {
    assert_eq!(
        installer_version_dir_name(loader, game_version, loader_version),
        expected.map(str::to_owned)
    );
}

// exercises the modern-profile early-return in
// migrate_legacy_loader_profile_if_needed. any of inheritsFrom,
// arguments, minecraftArguments present (or game_arguments absent)
// means "not legacy" and the function must return Ok(None) without
// touching the installer JSON path.
#[tokio::test]
async fn migrate_legacy_loader_profile_skips_modern_with_inherits_from() {
    use LaunchProfile;
    use chrono::Utc;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let instance_dir = tmp.path().join("instance");
    std::fs::create_dir_all(&instance_dir).unwrap();
    let profile_path = tmp.path().join("forge-1.20.1-47.2.0.json");
    std::fs::write(&profile_path, b"{}").unwrap();

    let modern = LaunchProfile {
        id: "1.20.1-forge-47.2.0".into(),
        inherits_from: Some("1.20.1".into()),
        main_class: Some("cpw.mods.bootstraplauncher.BootstrapLauncher".into()),
        ..Default::default()
    };

    let config = InstanceConfig {
        name: "test".into(),
        game_version: "1.20.1".into(),
        loader: ModLoader::Forge,
        loader_version: Some("47.2.0".into()),
        created: Utc::now(),
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
    };

    let result =
        migrate_legacy_loader_profile_if_needed(&profile_path, &modern, &config, &instance_dir)
            .await;
    assert!(
        matches!(result, Ok(None)),
        "expected Ok(None), got {result:?}"
    );
}

#[tokio::test]
async fn migrate_legacy_loader_profile_skips_fabric() {
    // a fresh upstream Fabric profile happens to match the "legacy"
    // shape (no inheritsFrom, no arguments, no minecraftArguments).
    // make sure the migration helper recognises this is Fabric and
    // returns Ok(None) instead of erroring with "reinstall Fabric".
    use LaunchProfile;
    use chrono::Utc;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let instance_dir = tmp.path().join("instance");
    std::fs::create_dir_all(&instance_dir).unwrap();
    let profile_path = tmp.path().join("fabric-1.20.1-0.14.21.json");
    std::fs::write(&profile_path, b"{}").unwrap();

    let upstream_fabric_shape = LaunchProfile {
        id: "fabric-loader-0.14.21-1.20.1".into(),
        inherits_from: None,
        main_class: Some("net.fabricmc.loader.impl.launch.knot.KnotClient".into()),
        libraries: Vec::new(),
        ..Default::default()
    };

    let config = InstanceConfig {
        name: "test".into(),
        game_version: "1.20.1".into(),
        loader: ModLoader::Fabric,
        loader_version: Some("0.14.21".into()),
        created: Utc::now(),
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
    };

    let result = migrate_legacy_loader_profile_if_needed(
        &profile_path,
        &upstream_fabric_shape,
        &config,
        &instance_dir,
    )
    .await;

    assert!(
        matches!(result, Ok(None)),
        "expected Ok(None), got {result:?}"
    );
}
