// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn instance_config_roundtrips_through_json() {
    let config = InstanceConfig {
        name: "test".to_string(),
        game_version: "1.20.1".to_string(),
        loader: ModLoader::Fabric,
        loader_version: Some("0.15.0".to_string()),
        created: Utc::now(),
        last_played: None,
        java_path: None,
        memory_max: Some("4G".to_string()),
        memory_min: Some("512M".to_string()),
        jvm_args: vec![],
        environment: Default::default(),
        window_mode: Default::default(),
        inherit_window_mode: false,
        resolution: Some((1920, 1080)),
        inherit_resolution: false,
        preferred_account: None,
        pre_launch_command: Default::default(),
        post_exit_command: Default::default(),
        glfw_path: None,
        config_sync_profile: None,
        modpack_source: Some(crate::instance::ProviderProject {
            provider: "modrinth".to_owned(),
            project_id: "pack".to_owned(),
            version_id: "version".to_owned(),
        }),
    };
    let json = serde_json::to_string_pretty(&config).expect("serialize");
    let parsed: InstanceConfig = serde_json::from_str(&json).expect("deserialize");
    // full-struct comparison: a serde skip/rename regression on any field
    // (loader_version, created, memory, jvm_args, ...) must fail this test.
    assert_eq!(parsed, config);
}

#[test]
fn instance_config_accepts_numeric_memory() {
    let json = r#"
        {
          "name": "test",
          "game_version": "1.7.10",
          "loader": "forge",
          "loader_version": "10.13.4.1614",
          "created": "2026-04-20T18:04:25.567993893Z",
          "memory_max": 8,
          "memory_min": 512
        }
        "#;
    let parsed: InstanceConfig = serde_json::from_str(json).expect("deserialize");
    assert_eq!(parsed.memory_max.as_deref(), Some("8G"));
    assert_eq!(parsed.memory_min.as_deref(), Some("512M"));
    assert!(parsed.environment.is_empty());
    assert_eq!(parsed.window_mode, WindowMode::Windowed);
    assert!(!parsed.inherit_window_mode);
    assert!(!parsed.inherit_resolution);
    assert_eq!(parsed.preferred_account, None);
    assert_eq!(parsed.pre_launch_command, LaunchCommand::default());
    assert_eq!(parsed.post_exit_command, LaunchCommand::default());
    assert_eq!(parsed.glfw_path, None);
}

#[test]
fn window_defaults_are_inherited_only_when_explicitly_enabled() {
    let mut config = InstanceConfig {
        name: "test".to_owned(),
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
        window_mode: WindowMode::Windowed,
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
    assert_eq!(
        config.effective_window_mode(WindowMode::Fullscreen),
        WindowMode::Windowed
    );
    assert_eq!(config.effective_resolution(Some((1920, 1080))), None);

    config.inherit_window_mode = true;
    config.inherit_resolution = true;
    assert_eq!(
        config.effective_window_mode(WindowMode::Fullscreen),
        WindowMode::Fullscreen
    );
    assert_eq!(
        config.effective_resolution(Some((1920, 1080))),
        Some((1920, 1080))
    );
}

#[test]
fn normalize_memory_value_handles_bare_numbers() {
    assert_eq!(normalize_memory_value("8").as_deref(), Some("8G"));
    assert_eq!(normalize_memory_value("4096").as_deref(), Some("4096M"));
    assert_eq!(normalize_memory_value("8G").as_deref(), Some("8G"));
    assert_eq!(normalize_memory_value("2048m").as_deref(), Some("2048M"));
    assert_eq!(normalize_memory_value(""), None);
}

#[test]
fn instance_config_ignores_invalid_memory_values() {
    let json = r#"
        {
          "name": "test",
          "game_version": "1.7.10",
          "loader": "forge",
          "loader_version": "10.13.4.1614",
          "created": "2026-04-20T18:04:25.567993893Z",
          "memory_max": ["8G"],
          "memory_min": "8GB"
        }
        "#;
    let parsed: InstanceConfig = serde_json::from_str(json).expect("deserialize");
    assert_eq!(parsed.memory_max, None);
    assert_eq!(parsed.memory_min, None);
}

#[test]
fn instance_config_normalizes_optional_paths_and_resolution() {
    let json = r#"
        {
          "name": "test",
          "game_version": "1.21.1",
          "loader": "vanilla",
          "loader_version": null,
          "created": "2026-04-20T18:04:25.567993893Z",
          "java_path": "  ",
          "glfw_path": " /usr/lib/libglfw.so ",
          "preferred_account": "",
          "resolution": [0, 1080]
        }
        "#;

    let parsed: InstanceConfig = serde_json::from_str(json).expect("deserialize");

    assert!(parsed.java_path.is_none());
    assert_eq!(parsed.glfw_path.as_deref(), Some("/usr/lib/libglfw.so"));
    assert!(parsed.preferred_account.is_none());
    assert!(parsed.resolution.is_none());
}

#[test]
fn instance_config_rejects_zero_resolution_height() {
    let json = r#"
        {
          "name": "test",
          "game_version": "1.21.1",
          "loader": "vanilla",
          "loader_version": null,
          "created": "2026-04-20T18:04:25.567993893Z",
          "resolution": [1920, 0]
        }
        "#;

    let parsed: InstanceConfig = serde_json::from_str(json).expect("deserialize");

    assert!(parsed.resolution.is_none());
}

#[test]
fn normalize_memory_value_rejects_invalid_values() {
    assert_eq!(normalize_memory_value("0"), None);
    assert_eq!(normalize_memory_value("-1"), None);
    assert_eq!(normalize_memory_value("1.5G"), None);
    assert_eq!(normalize_memory_value("8GB"), None);
    assert_eq!(normalize_memory_value("banana"), None);
}

#[test]
fn parse_resolution_accepts_common_separators() {
    assert_eq!(parse_resolution("1920x1080"), Ok((1920, 1080)));
    assert_eq!(parse_resolution(" 1280X720 "), Ok((1280, 720)));
}

#[test]
fn parse_resolution_rejects_invalid_values() {
    assert!(parse_resolution("1920").is_err());
    assert!(parse_resolution("0x1080").is_err());
    assert!(parse_resolution("wide x tall").is_err());
}
