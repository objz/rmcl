// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// integration tests for build_launch_invocation - the seam that takes an
// instance config plus resolved auth credentials and returns the full java
// invocation that would be spawned. these tests build the expected on-disk
// fixture layout in a tempdir, call the seam, and assert on the rendered
// LaunchInvocation. nothing here actually spawns a process.

use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::json;
use tempfile::TempDir;

use rmcl::instance::launch::{
    LaunchAuth, LaunchError, build_launch_invocation, supports_quick_play,
};
use rmcl::instance::models::{InstanceConfig, ModLoader};

// ---------- helpers ----------

const PLAYER: &str = "TestPlayer";
const PLAYER_UUID: &str = "00000000-0000-0000-0000-000000000001";
const PLAYER_TOKEN: &str = "secret-access-token";

fn test_auth() -> LaunchAuth<'static> {
    LaunchAuth {
        username: PLAYER,
        uuid: PLAYER_UUID,
        token: PLAYER_TOKEN,
        user_type: "msa",
    }
}

fn make_config(name: &str, game_version: &str, loader: ModLoader) -> InstanceConfig {
    make_config_with(name, game_version, loader, None)
}

fn make_config_with(
    name: &str,
    game_version: &str,
    loader: ModLoader,
    loader_version: Option<&str>,
) -> InstanceConfig {
    InstanceConfig {
        name: name.into(),
        game_version: game_version.into(),
        loader,
        loader_version: loader_version.map(str::to_owned),
        created: Utc::now(),
        last_played: None,
        // pinned to a deterministic value so detect_java_path is never
        // invoked and tests don't depend on the host's java install.
        java_path: Some("/usr/bin/java-test".into()),
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
    }
}

#[cfg(unix)]
fn fake_java(tmp: &TempDir, major: u32) -> String {
    use std::os::unix::fs::PermissionsExt;

    let path = tmp.path().join(format!("java-{major}"));
    std::fs::write(
        &path,
        format!("#!/bin/sh\necho 'openjdk version \"{major}.0.1\"' >&2\n"),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
    path.to_string_lossy().into_owned()
}

// builds the on-disk layout that build_launch_invocation expects:
//   <tmp>/instances/<name>/minecraft/         (instance dir, created empty)
//   <tmp>/meta/versions/<game_version>/meta.json
//   <tmp>/meta/cache/loaders/profiles/         (created empty)
//   <tmp>/meta/libraries/                     (created empty)
struct Fixture {
    _tmp: TempDir,
    instances_dir: PathBuf,
    meta_dir: PathBuf,
}

impl Fixture {
    fn new(instance_name: &str, game_version: &str, vanilla_meta: serde_json::Value) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let instances_dir = tmp.path().join("instances");
        let meta_dir = tmp.path().join("meta");

        let instance_minecraft = instances_dir.join(instance_name).join("minecraft");
        std::fs::create_dir_all(&instance_minecraft).unwrap();

        std::fs::create_dir_all(meta_dir.join("cache/minecraft/libraries")).unwrap();
        std::fs::create_dir_all(meta_dir.join("cache/loaders/profiles")).unwrap();
        let version_dir = meta_dir.join("cache/minecraft/versions").join(game_version);
        std::fs::create_dir_all(&version_dir).unwrap();
        std::fs::write(
            version_dir.join("meta.json"),
            serde_json::to_vec_pretty(&vanilla_meta).unwrap(),
        )
        .unwrap();

        Self {
            _tmp: tmp,
            instances_dir,
            meta_dir,
        }
    }

    fn write_loader_profile(&self, filename: &str, content: serde_json::Value) {
        std::fs::write(
            self.meta_dir.join("cache/loaders/profiles").join(filename),
            serde_json::to_vec_pretty(&content).unwrap(),
        )
        .unwrap();
    }

    fn instance_libraries_dir(&self, instance_name: &str) -> PathBuf {
        self.instances_dir
            .join(instance_name)
            .join("minecraft")
            .join("libraries")
    }
}

fn modern_vanilla_meta(id: &str) -> serde_json::Value {
    json!({
        "id": id,
        "type": "release",
        "mainClass": "net.minecraft.client.main.Main",
        "assetIndex": {
            "id": "5",
            "url": "https://example.com/assets/index.json",
            "sha1": "0000000000000000000000000000000000000000"
        },
        "libraries": [
            {
                "name": "org.slf4j:slf4j-api:2.0.7",
                "downloads": {
                    "artifact": {
                        "url": "https://example.com/slf4j.jar",
                        "path": "org/slf4j/slf4j-api/2.0.7/slf4j-api-2.0.7.jar",
                        "sha1": "0000000000000000000000000000000000000000",
                        "size": 0
                    }
                }
            }
        ],
        "arguments": {
            "game": [
                "--username", "${auth_player_name}",
                "--version", "${version_name}",
                "--uuid", "${auth_uuid}",
                "--accessToken", "${auth_access_token}",
                "--userType", "${user_type}",
                "--versionType", "${version_type}"
            ],
            "jvm": [
                "-Djava.library.path=${natives_directory}",
                "-Dminecraft.launcher.brand=${launcher_name}"
            ]
        }
    })
}

#[cfg(unix)]
fn modern_vanilla_meta_with_java(id: &str, java_major: u32) -> serde_json::Value {
    let mut meta = modern_vanilla_meta(id);
    meta["javaVersion"] = json!({
        "component": format!("java-runtime-{java_major}"),
        "majorVersion": java_major
    });
    meta
}

fn legacy_vanilla_meta(id: &str) -> serde_json::Value {
    json!({
        "id": id,
        "type": "release",
        "mainClass": "net.minecraft.launchwrapper.Launch",
        "assetIndex": {
            "id": "1.7.10",
            "url": "https://example.com/assets/index.json",
            "sha1": "0000000000000000000000000000000000000000"
        },
        "libraries": [],
        "minecraftArguments": "--username ${auth_player_name} --uuid ${auth_uuid} --accessToken ${auth_access_token} --userType ${user_type}"
    })
}

fn platform_classpath_sep() -> &'static str {
    if cfg!(windows) { ";" } else { ":" }
}

// ---------- vanilla ----------

#[tokio::test]
async fn vanilla_modern_builds_complete_invocation() {
    let fx = Fixture::new("v1", "1.20.1", modern_vanilla_meta("1.20.1"));
    let config = make_config("v1", "1.20.1", ModLoader::Vanilla);

    let inv = build_launch_invocation(&config, &fx.instances_dir, &fx.meta_dir, &test_auth(), None)
        .await
        .unwrap();

    assert_eq!(inv.java, "/usr/bin/java-test");
    assert_eq!(inv.main_class, "net.minecraft.client.main.Main");
    assert!(inv.extra_args.is_empty());

    let expected_natives = fx
        .meta_dir
        .join("cache/minecraft/versions")
        .join("1.20.1")
        .join("natives");
    let actual_natives = inv
        .jvm_args
        .iter()
        .find_map(|arg| arg.strip_prefix("-Djava.library.path="))
        .map(Path::new);
    assert_eq!(
        actual_natives,
        Some(expected_natives.as_path()),
        "jvm_args missing natives substitution: {:?}",
        inv.jvm_args,
    );
    assert!(
        inv.jvm_args
            .iter()
            .any(|a| a == "-Dminecraft.launcher.brand=rmcl"),
        "jvm_args missing launcher brand: {:?}",
        inv.jvm_args
    );

    // working_dir is <instances_dir>/<name>/minecraft
    assert_eq!(
        inv.working_dir,
        fx.instances_dir.join("v1").join("minecraft")
    );
    // classpath = the one vanilla lib + the vanilla client jar
    let slf4j = fx
        .meta_dir
        .join("cache/minecraft/libraries/org/slf4j/slf4j-api/2.0.7/slf4j-api-2.0.7.jar");
    let client_jar = fx
        .meta_dir
        .join("cache/minecraft/versions/1.20.1/1.20.1.jar");
    assert!(inv.classpath.contains(&slf4j));
    assert!(inv.classpath.contains(&client_jar));
}

#[tokio::test]
async fn quick_play_passes_the_selected_save_folder() {
    let mut meta = modern_vanilla_meta("1.20.1");
    meta["arguments"]["game"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "rules": [{
                "action": "allow",
                "features": { "is_quick_play_singleplayer": true }
            }],
            "value": ["--quickPlaySingleplayer", "${quickPlaySingleplayer}"]
        }));
    let fx = Fixture::new("quick", "1.20.1", meta);
    let world_dir = fx.instances_dir.join("quick/minecraft/saves/Display World");
    std::fs::create_dir_all(&world_dir).unwrap();
    let config = make_config("quick", "1.20.1", ModLoader::Vanilla);

    assert!(supports_quick_play(&fx.meta_dir, "1.20.1"));
    let invocation = build_launch_invocation(
        &config,
        &fx.instances_dir,
        &fx.meta_dir,
        &test_auth(),
        Some("Display World"),
    )
    .await
    .unwrap();

    assert!(
        invocation
            .game_args
            .windows(2)
            .any(|args| args == ["--quickPlaySingleplayer", "Display World"])
    );
}

#[cfg(unix)]
#[tokio::test]
async fn launch_fails_when_selected_java_is_older_than_profile_requires() {
    let fx = Fixture::new(
        "java-old",
        "26.1.2",
        modern_vanilla_meta_with_java("26.1.2", 25),
    );
    let mut config = make_config("java-old", "26.1.2", ModLoader::Vanilla);
    config.java_path = Some(fake_java(&fx._tmp, 21));

    let err = build_launch_invocation(&config, &fx.instances_dir, &fx.meta_dir, &test_auth(), None)
        .await
        .expect_err("java 21 should fail for a Java 25 profile");

    assert!(
        matches!(
            err,
            LaunchError::JavaTooOld {
                required: 25,
                detected: 21,
                ..
            }
        ),
        "expected JavaTooOld, got {err:?}"
    );
}

#[tokio::test]
async fn vanilla_legacy_args_format_substitutes_tokens() {
    let fx = Fixture::new("vlegacy", "1.7.10", legacy_vanilla_meta("1.7.10"));
    let config = make_config("vlegacy", "1.7.10", ModLoader::Vanilla);

    let inv = build_launch_invocation(&config, &fx.instances_dir, &fx.meta_dir, &test_auth(), None)
        .await
        .unwrap();

    assert_eq!(inv.main_class, "net.minecraft.launchwrapper.Launch");

    // legacy format has no upstream jvm args; jvm_args should be just Xms/Xmx
    assert_eq!(inv.jvm_args.len(), 2);
    assert!(inv.jvm_args[0].starts_with("-Xms"));
    assert!(inv.jvm_args[1].starts_with("-Xmx"));

    // game_args should be space-split with substitutions applied
    let joined = inv.game_args.join(" ");
    assert!(joined.contains(&format!("--username {}", PLAYER)));
    assert!(joined.contains(&format!("--uuid {}", PLAYER_UUID)));
    assert!(joined.contains(&format!("--accessToken {}", PLAYER_TOKEN)));
    assert!(joined.contains("--userType msa"));
}

// ---------- forge ----------

#[tokio::test]
async fn forge_modern_includes_add_opens() {
    let fx = Fixture::new("f1", "1.20.1", modern_vanilla_meta("1.20.1"));
    fx.write_loader_profile(
        "forge-1.20.1-47.2.0.json",
        json!({
            "id": "1.20.1-forge-47.2.0",
            "inheritsFrom": "1.20.1",
            "type": "release",
            "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
            "libraries": [
                { "name": "net.minecraftforge:fmlloader:1.20.1-47.2.0" }
            ],
            "arguments": {
                "game": ["--launchTarget", "forge_client"],
                "jvm": [
                    "--add-opens",
                    "java.base/sun.security.util=ALL-UNNAMED",
                    "-DignoreList=bootstraplauncher,securejarhandler"
                ]
            }
        }),
    );
    let config = make_config_with("f1", "1.20.1", ModLoader::Forge, Some("47.2.0"));

    let inv = build_launch_invocation(&config, &fx.instances_dir, &fx.meta_dir, &test_auth(), None)
        .await
        .unwrap();

    assert_eq!(
        inv.main_class,
        "cpw.mods.bootstraplauncher.BootstrapLauncher"
    );
    assert!(
        inv.jvm_args.iter().any(|a| a == "--add-opens"),
        "Forge --add-opens missing: {:?}",
        inv.jvm_args
    );
    assert!(
        inv.jvm_args
            .iter()
            .any(|a| a == "java.base/sun.security.util=ALL-UNNAMED"),
        "Forge --add-opens target missing: {:?}",
        inv.jvm_args
    );
    assert!(
        inv.game_args
            .windows(2)
            .any(|w| w == ["--launchTarget", "forge_client"]),
        "Forge launchTarget missing from game_args: {:?}",
        inv.game_args
    );
}

#[tokio::test]
async fn forge_local_lib_dir_preferred_over_meta_dir() {
    let fx = Fixture::new("f2", "1.20.1", modern_vanilla_meta("1.20.1"));
    fx.write_loader_profile(
        "forge-1.20.1-47.2.0.json",
        json!({
            "id": "1.20.1-forge-47.2.0",
            "inheritsFrom": "1.20.1",
            "type": "release",
            "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
            "libraries": [
                { "name": "net.minecraftforge:fmlloader:1.20.1-47.2.0" }
            ],
            "arguments": { "game": [], "jvm": [] }
        }),
    );

    // drop fmlloader into the instance-local libraries dir so the launcher
    // finds it there rather than under the shared meta cache.
    let local_lib = fx
        .instance_libraries_dir("f2")
        .join("net/minecraftforge/fmlloader/1.20.1-47.2.0");
    std::fs::create_dir_all(&local_lib).unwrap();
    let local_jar = local_lib.join("fmlloader-1.20.1-47.2.0.jar");
    std::fs::write(&local_jar, b"jar").unwrap();

    let config = make_config_with("f2", "1.20.1", ModLoader::Forge, Some("47.2.0"));
    let inv = build_launch_invocation(&config, &fx.instances_dir, &fx.meta_dir, &test_auth(), None)
        .await
        .unwrap();

    assert!(
        inv.classpath.contains(&local_jar),
        "expected local fmlloader on classpath: {:?}",
        inv.classpath
    );
    // and the meta-dir candidate should not appear
    let meta_candidate = fx
        .meta_dir
        .join("libraries/net/minecraftforge/fmlloader/1.20.1-47.2.0/fmlloader-1.20.1-47.2.0.jar");
    assert!(
        !inv.classpath.contains(&meta_candidate),
        "meta-dir candidate should not be on classpath when local exists"
    );
}

// ---------- fabric ----------

#[tokio::test]
async fn fabric_implicit_inheritsfrom_resolves() {
    let fx = Fixture::new("fab", "1.20.1", modern_vanilla_meta("1.20.1"));
    // Fabric upstream profiles omit inheritsFrom; the launch flow patches it
    // in before resolving. assert that the merge picks up Fabric's main class.
    fx.write_loader_profile(
        "fabric-1.20.1-0.15.0.json",
        json!({
            "id": "fabric-loader-0.15.0-1.20.1",
            "type": "release",
            "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "libraries": [
                { "name": "net.fabricmc:fabric-loader:0.15.0" }
            ]
        }),
    );
    let config = make_config_with("fab", "1.20.1", ModLoader::Fabric, Some("0.15.0"));

    let inv = build_launch_invocation(&config, &fx.instances_dir, &fx.meta_dir, &test_auth(), None)
        .await
        .unwrap();

    assert_eq!(
        inv.main_class,
        "net.fabricmc.loader.impl.launch.knot.KnotClient"
    );
    // both fabric-loader and vanilla slf4j must appear on the merged classpath
    let fabric_loader = fx.meta_dir.join(
        "cache/minecraft/libraries/net/fabricmc/fabric-loader/0.15.0/fabric-loader-0.15.0.jar",
    );
    let slf4j = fx
        .meta_dir
        .join("cache/minecraft/libraries/org/slf4j/slf4j-api/2.0.7/slf4j-api-2.0.7.jar");
    assert!(
        inv.classpath.contains(&fabric_loader),
        "fabric-loader missing from classpath: {:?}",
        inv.classpath
    );
    assert!(
        inv.classpath.contains(&slf4j),
        "vanilla slf4j missing from classpath: {:?}",
        inv.classpath
    );
}

// ---------- neoforge ----------

#[tokio::test]
async fn neoforge_inheritsfrom_resolves() {
    let fx = Fixture::new("ne", "1.20.6", modern_vanilla_meta("1.20.6"));
    fx.write_loader_profile(
        "neoforge-20.4.190.json",
        json!({
            "id": "neoforge-20.4.190",
            "inheritsFrom": "1.20.6",
            "type": "release",
            "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
            "libraries": [
                { "name": "net.neoforged:neoforge:20.4.190" }
            ],
            "arguments": {
                "game": [],
                "jvm": ["--add-modules", "ALL-MODULE-PATH"]
            }
        }),
    );
    let config = make_config_with("ne", "1.20.6", ModLoader::NeoForge, Some("20.4.190"));

    let inv = build_launch_invocation(&config, &fx.instances_dir, &fx.meta_dir, &test_auth(), None)
        .await
        .unwrap();

    assert_eq!(
        inv.main_class,
        "cpw.mods.bootstraplauncher.BootstrapLauncher"
    );
    assert!(
        inv.jvm_args
            .windows(2)
            .any(|w| w == ["--add-modules", "ALL-MODULE-PATH"]),
        "NeoForge --add-modules missing: {:?}",
        inv.jvm_args
    );
}

// ---------- template substitution ----------

#[tokio::test]
async fn auth_credentials_substituted_in_game_args() {
    let fx = Fixture::new("auth", "1.20.1", modern_vanilla_meta("1.20.1"));
    let config = make_config("auth", "1.20.1", ModLoader::Vanilla);

    let inv = build_launch_invocation(&config, &fx.instances_dir, &fx.meta_dir, &test_auth(), None)
        .await
        .unwrap();

    assert!(
        inv.game_args.iter().any(|a| a == PLAYER),
        "auth_player_name not substituted: {:?}",
        inv.game_args
    );
    assert!(
        inv.game_args.iter().any(|a| a == PLAYER_UUID),
        "auth_uuid not substituted: {:?}",
        inv.game_args
    );
    assert!(
        inv.game_args.iter().any(|a| a == PLAYER_TOKEN),
        "auth_access_token not substituted: {:?}",
        inv.game_args
    );
    assert!(
        inv.game_args.iter().any(|a| a == "msa"),
        "user_type not substituted: {:?}",
        inv.game_args
    );
}

#[tokio::test]
async fn version_type_substituted() {
    // type = "snapshot" in the fixture; ${version_type} should render that
    let mut meta = modern_vanilla_meta("1.20.1");
    meta["type"] = json!("snapshot");
    let fx = Fixture::new("vt", "1.20.1", meta);
    let config = make_config("vt", "1.20.1", ModLoader::Vanilla);

    let inv = build_launch_invocation(&config, &fx.instances_dir, &fx.meta_dir, &test_auth(), None)
        .await
        .unwrap();

    assert!(
        inv.game_args.iter().any(|a| a == "snapshot"),
        "version_type not substituted to 'snapshot': {:?}",
        inv.game_args
    );
}

// ---------- classpath ----------

#[tokio::test]
async fn classpath_uses_platform_separator() {
    let fx = Fixture::new("cp", "1.20.1", modern_vanilla_meta("1.20.1"));
    let config = make_config("cp", "1.20.1", ModLoader::Vanilla);

    let inv = build_launch_invocation(&config, &fx.instances_dir, &fx.meta_dir, &test_auth(), None)
        .await
        .unwrap();

    // with two classpath entries we should see exactly one separator
    assert_eq!(inv.classpath.len(), 2);
    let sep = platform_classpath_sep();
    assert_eq!(inv.classpath_string.matches(sep).count(), 1);
}

#[tokio::test]
async fn rule_disallow_excludes_library() {
    // a library whose rule always denies must not appear on the classpath
    let mut meta = modern_vanilla_meta("1.20.1");
    meta["libraries"] = json!([
        {
            "name": "org.slf4j:slf4j-api:2.0.7",
            "downloads": {
                "artifact": {
                    "url": "",
                    "path": "org/slf4j/slf4j-api/2.0.7/slf4j-api-2.0.7.jar",
                    "sha1": "0000000000000000000000000000000000000000",
                    "size": 0
                }
            }
        },
        {
            "name": "com.denied:lib:1.0",
            "rules": [{ "action": "disallow" }],
            "downloads": {
                "artifact": {
                    "url": "",
                    "path": "com/denied/lib/1.0/lib-1.0.jar",
                    "sha1": "0000000000000000000000000000000000000000",
                    "size": 0
                }
            }
        }
    ]);
    let fx = Fixture::new("rule", "1.20.1", meta);
    let config = make_config("rule", "1.20.1", ModLoader::Vanilla);

    let inv = build_launch_invocation(&config, &fx.instances_dir, &fx.meta_dir, &test_auth(), None)
        .await
        .unwrap();

    let denied = fx.meta_dir.join("libraries/com/denied/lib/1.0/lib-1.0.jar");
    assert!(
        !inv.classpath.contains(&denied),
        "denied library was included: {:?}",
        inv.classpath
    );
}

// ---------- memory ----------

#[tokio::test]
async fn xms_xmx_use_config_memory() {
    let fx = Fixture::new("mem", "1.20.1", modern_vanilla_meta("1.20.1"));
    let mut config = make_config("mem", "1.20.1", ModLoader::Vanilla);
    config.memory_min = Some("1G".into());
    config.memory_max = Some("4G".into());

    let inv = build_launch_invocation(&config, &fx.instances_dir, &fx.meta_dir, &test_auth(), None)
        .await
        .unwrap();

    assert!(inv.jvm_args.iter().any(|a| a == "-Xms1G"));
    assert!(inv.jvm_args.iter().any(|a| a == "-Xmx4G"));
}

#[tokio::test]
async fn instance_launch_options_are_injected() {
    let fx = Fixture::new("options", "1.20.1", modern_vanilla_meta("1.20.1"));
    let mut config = make_config("options", "1.20.1", ModLoader::Vanilla);
    config.window_mode = rmcl::instance::WindowMode::Fullscreen;
    config.glfw_path = Some("/usr/lib/libglfw.so.3".to_owned());
    config
        .environment
        .insert("MESA_LOADER_DRIVER_OVERRIDE".to_owned(), "zink".to_owned());

    let inv = build_launch_invocation(&config, &fx.instances_dir, &fx.meta_dir, &test_auth(), None)
        .await
        .unwrap();

    assert!(
        inv.game_args
            .iter()
            .any(|argument| argument == "--fullscreen")
    );
    assert!(
        inv.jvm_args
            .iter()
            .any(|argument| argument == "-Dorg.lwjgl.glfw.libname=/usr/lib/libglfw.so.3")
    );
    assert_eq!(
        inv.environment
            .get("MESA_LOADER_DRIVER_OVERRIDE")
            .map(String::as_str),
        Some("zink")
    );
}

#[tokio::test]
async fn default_memory_used_when_unset() {
    let fx = Fixture::new("memdef", "1.20.1", modern_vanilla_meta("1.20.1"));
    let config = make_config("memdef", "1.20.1", ModLoader::Vanilla);
    // memory_min and memory_max default to None in make_config

    let inv = build_launch_invocation(&config, &fx.instances_dir, &fx.meta_dir, &test_auth(), None)
        .await
        .unwrap();

    assert!(inv.jvm_args.iter().any(|a| a == "-Xms512M"));
    assert!(inv.jvm_args.iter().any(|a| a == "-Xmx2G"));
}

// ---------- error paths ----------

#[tokio::test]
async fn meta_not_found_returns_error() {
    let tmp = tempfile::tempdir().unwrap();
    let instances_dir = tmp.path().join("instances");
    let meta_dir = tmp.path().join("meta");
    std::fs::create_dir_all(instances_dir.join("ghost").join("minecraft")).unwrap();
    std::fs::create_dir_all(meta_dir.join("cache/minecraft/versions")).unwrap();

    let config = make_config("ghost", "1.20.1", ModLoader::Vanilla);
    let err = build_launch_invocation(&config, &instances_dir, &meta_dir, &test_auth(), None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, LaunchError::MetaNotFound(_)),
        "expected MetaNotFound, got: {err:?}"
    );
}

#[tokio::test]
async fn loader_profile_missing_returns_error() {
    let fx = Fixture::new("lpmiss", "1.20.1", modern_vanilla_meta("1.20.1"));
    let config = make_config_with("lpmiss", "1.20.1", ModLoader::Fabric, Some("0.15.0"));

    let err = build_launch_invocation(&config, &fx.instances_dir, &fx.meta_dir, &test_auth(), None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, LaunchError::MetaNotFound(_)),
        "expected MetaNotFound for missing loader profile, got: {err:?}"
    );
}

#[tokio::test]
async fn merged_profile_missing_main_class_fails() {
    // a meta.json that deserialises cleanly but lacks the required mainClass
    // must surface as a Parse error; otherwise the launcher would try to
    // unwrap None and crash later.
    let mut meta = modern_vanilla_meta("1.20.1");
    meta.as_object_mut().unwrap().remove("mainClass");
    let fx = Fixture::new("nomc", "1.20.1", meta);
    let config = make_config("nomc", "1.20.1", ModLoader::Vanilla);

    let err = build_launch_invocation(&config, &fx.instances_dir, &fx.meta_dir, &test_auth(), None)
        .await
        .unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        matches!(err, LaunchError::Parse(_)) && msg.contains("mainClass"),
        "expected Parse error mentioning mainClass, got: {err:?}"
    );
}

#[tokio::test]
async fn legacy_loader_profile_missing_loader_version_errors() {
    // an instance with no loader_version recorded must error out cleanly
    // when its loader profile is in the legacy stripped shape (no
    // inheritsFrom, no arguments, no minecraftArguments, but has the rmcl
    // 0.3.0-era gameArguments field).
    let fx = Fixture::new("nolv", "1.20.1", modern_vanilla_meta("1.20.1"));
    fx.write_loader_profile(
        "forge-1.20.1-unknown.json",
        json!({
            "id": "1.20.1-forge",
            "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
            "libraries": [],
            "gameArguments": ["--launchTarget", "forge_client"]
        }),
    );

    let mut config = make_config("nolv", "1.20.1", ModLoader::Forge);
    config.loader_version = None;

    let err = build_launch_invocation(&config, &fx.instances_dir, &fx.meta_dir, &test_auth(), None)
        .await
        .unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        matches!(err, LaunchError::Parse(_)) && msg.contains("Reinstall"),
        "expected Parse error asking for reinstall, got: {err:?}"
    );
}

#[tokio::test]
async fn legacy_loader_profile_missing_installer_json_errors() {
    // legacy stripped profile + loader_version, but the installer JSON the
    // migration would copy from isn't present on disk. Parse error.
    let fx = Fixture::new("noinst", "1.20.1", modern_vanilla_meta("1.20.1"));
    fx.write_loader_profile(
        "forge-1.20.1-47.2.0.json",
        json!({
            "id": "1.20.1-forge-47.2.0",
            "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
            "libraries": [],
            "gameArguments": ["--launchTarget", "forge_client"]
        }),
    );

    let config = make_config_with("noinst", "1.20.1", ModLoader::Forge, Some("47.2.0"));

    let err = build_launch_invocation(&config, &fx.instances_dir, &fx.meta_dir, &test_auth(), None)
        .await
        .unwrap_err();
    let msg = format!("{err:?}");
    assert!(
        matches!(err, LaunchError::Parse(_))
            && msg.contains("installer JSON")
            && msg.contains("missing"),
        "expected Parse error about missing installer JSON, got: {err:?}"
    );
}
