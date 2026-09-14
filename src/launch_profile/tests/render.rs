// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;
use crate::launch_profile::model::Arguments;
use crate::launch_profile::rules::{FeatureSet, OsCondition, Rule, RuleAction};
use std::path::PathBuf;

// owns the path buffers + FeatureSet so each test just calls
// fx.template_ctx() and fx.rule_ctx() instead of declaring four PathBufs
// and a RuleContext inline. all tests use linux/x86_64; if a test needs
// a different OS it sets it explicitly.
struct Fixture {
    lib: PathBuf,
    nat: PathBuf,
    game: PathBuf,
    assets: PathBuf,
    features: FeatureSet,
}

impl Fixture {
    fn new() -> Self {
        Self {
            lib: PathBuf::from("/m/libraries"),
            nat: PathBuf::from("/m/natives"),
            game: PathBuf::from("/i/.minecraft"),
            assets: PathBuf::from("/m/assets"),
            features: FeatureSet::default(),
        }
    }

    fn template_ctx(&self) -> TemplateContext<'_> {
        TemplateContext {
            library_directory: &self.lib,
            classpath_separator: ":",
            version_name: "1.20.1",
            version_type: "release",
            natives_directory: &self.nat,
            classpath: "a.jar:b.jar",
            game_directory: &self.game,
            assets_root: &self.assets,
            assets_index_name: "5",
            auth_player_name: "Player",
            auth_uuid: "00000000-0000-0000-0000-000000000000",
            auth_access_token: "token",
            auth_xuid: "0",
            user_type: "msa",
            user_properties: "{}",
            launcher_name: "rmcl",
            launcher_version: "0.3.0",
            clientid: "0",
            quick_play_singleplayer: None,
            resolution_width: None,
            resolution_height: None,
        }
    }

    fn rule_ctx(&self) -> RuleContext<'_> {
        RuleContext {
            os_name: "linux",
            os_version: "6.0",
            arch: "x86_64",
            features: &self.features,
        }
    }
}

fn minimal_profile() -> LaunchProfile {
    LaunchProfile {
        id: "test".into(),
        main_class: Some("net.test.Main".into()),
        ..Default::default()
    }
}

#[test]
fn legacy_minecraft_arguments_render_into_game() {
    let fx = Fixture::new();
    let mut profile = minimal_profile();
    profile.minecraft_arguments =
        Some("--username ${auth_player_name} --version ${version_name}".into());

    let rendered = render_args(&profile, &fx.rule_ctx(), &fx.template_ctx()).unwrap();
    assert_eq!(rendered.main_class, "net.test.Main");
    assert!(rendered.jvm.is_empty());
    assert_eq!(
        rendered.game,
        vec!["--username", "Player", "--version", "1.20.1"]
    );
}

#[test]
fn modern_arguments_render_with_literals_and_substitutions() {
    let fx = Fixture::new();
    let mut profile = minimal_profile();
    profile.arguments = Some(Arguments {
        game: vec![
            Argument::Literal("--username".into()),
            Argument::Literal("${auth_player_name}".into()),
        ],
        jvm: vec![Argument::Literal(
            "-Djava.library.path=${natives_directory}".into(),
        )],
    });

    let rendered = render_args(&profile, &fx.rule_ctx(), &fx.template_ctx()).unwrap();
    assert_eq!(rendered.game, vec!["--username", "Player"]);
    assert_eq!(rendered.jvm, vec!["-Djava.library.path=/m/natives"]);
}

#[test]
fn conditional_argument_with_single_value_is_filtered_by_os_rule() {
    let fx = Fixture::new();
    let osx_only = Argument::Conditional {
        rules: vec![Rule {
            action: RuleAction::Allow,
            os: Some(OsCondition {
                name: Some("osx".into()),
                arch: None,
                ..Default::default()
            }),
            features: None,
        }],
        value: ArgumentValue::Single("-XstartOnFirstThread".into()),
    };

    let mut profile = minimal_profile();
    profile.arguments = Some(Arguments {
        game: Vec::new(),
        jvm: vec![osx_only],
    });

    let rendered = render_args(&profile, &fx.rule_ctx(), &fx.template_ctx()).unwrap();
    assert!(
        rendered.jvm.is_empty(),
        "osx-only arg should be skipped on linux"
    );
}

#[test]
fn conditional_argument_with_multiple_value_pushes_all() {
    let fx = Fixture::new();
    let linux_arg = Argument::Conditional {
        rules: vec![Rule {
            action: RuleAction::Allow,
            os: Some(OsCondition {
                name: Some("linux".into()),
                arch: None,
                ..Default::default()
            }),
            features: None,
        }],
        value: ArgumentValue::Multiple(vec![
            "--add-opens".into(),
            "java.base/sun.security.util=ALL-UNNAMED".into(),
        ]),
    };

    let mut profile = minimal_profile();
    profile.arguments = Some(Arguments {
        game: Vec::new(),
        jvm: vec![linux_arg],
    });

    let rendered = render_args(&profile, &fx.rule_ctx(), &fx.template_ctx()).unwrap();
    assert_eq!(
        rendered.jvm,
        vec!["--add-opens", "java.base/sun.security.util=ALL-UNNAMED"]
    );
}

#[test]
fn missing_main_class_returns_error() {
    let fx = Fixture::new();
    let mut profile = minimal_profile();
    profile.main_class = None;

    let result = render_args(&profile, &fx.rule_ctx(), &fx.template_ctx());
    assert!(matches!(result, Err(RenderError::MissingMainClass)));
}

#[tokio::test]
async fn end_to_end_resolve_then_render_modern_forge_shape() {
    // exercises the full pipeline: load a synthetic vanilla profile
    // from disk, load a synthetic loader profile with inheritsFrom,
    // resolve the chain, then render args. catches integration bugs
    // that unit tests of each layer would miss.
    use crate::launch_profile::resolve;
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let vanilla_path = tmp
        .path()
        .join("cache/minecraft/versions")
        .join("1.20.1")
        .join("meta.json");
    std::fs::create_dir_all(vanilla_path.parent().unwrap()).unwrap();
    let vanilla_json = br#"{
            "id": "1.20.1",
            "mainClass": "net.minecraft.client.main.Main",
            "libraries": [
                {
                    "name": "org.lwjgl:lwjgl:3.3.1",
                    "downloads": {
                        "artifact": {
                            "url": "https://example.invalid/lwjgl.jar",
                            "path": "org/lwjgl/lwjgl/3.3.1/lwjgl-3.3.1.jar",
                            "sha1": "1111111111111111111111111111111111111111",
                            "size": 100
                        }
                    }
                }
            ],
            "arguments": {
                "game": ["--username", "${auth_player_name}", "--version", "${version_name}"],
                "jvm": ["-Djava.library.path=${natives_directory}"]
            }
        }"#;
    std::fs::write(&vanilla_path, vanilla_json).unwrap();

    let loader_json = r#"{
            "id": "1.20.1-forge-47.2.0",
            "inheritsFrom": "1.20.1",
            "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
            "libraries": [
                { "name": "net.minecraftforge:forge:47.2.0" }
            ],
            "arguments": {
                "game": ["--launchTarget", "forge_client"],
                "jvm": [
                    "--add-opens", "java.base/sun.security.util=cpw.mods.securejarhandler"
                ]
            }
        }"#;
    let loader_profile: LaunchProfile = serde_json::from_str(loader_json).unwrap();

    let merged = resolve::resolve(loader_profile, tmp.path()).await.unwrap();

    let fx = Fixture::new();
    let rendered = render_args(&merged, &fx.rule_ctx(), &fx.template_ctx()).unwrap();

    // child main_class wins after merge
    assert_eq!(
        rendered.main_class,
        "cpw.mods.bootstraplauncher.BootstrapLauncher"
    );
    // game args: parent first then child
    assert_eq!(
        rendered.game,
        vec![
            "--username",
            "Player",
            "--version",
            "1.20.1",
            "--launchTarget",
            "forge_client"
        ]
    );
    // jvm args: parent first then child
    assert_eq!(
        rendered.jvm,
        vec![
            "-Djava.library.path=/m/natives",
            "--add-opens",
            "java.base/sun.security.util=cpw.mods.securejarhandler"
        ]
    );
}

#[test]
fn modern_arguments_takes_precedence_over_legacy_field() {
    // a profile that somehow has both arguments and minecraft_arguments
    // should use arguments only (legacy is fallback).
    let fx = Fixture::new();
    let mut profile = minimal_profile();
    profile.arguments = Some(Arguments {
        game: vec![Argument::Literal("--from-arguments".into())],
        jvm: Vec::new(),
    });
    profile.minecraft_arguments = Some("--from-legacy".into());

    let rendered = render_args(&profile, &fx.rule_ctx(), &fx.template_ctx()).unwrap();
    assert_eq!(rendered.game, vec!["--from-arguments"]);
}
