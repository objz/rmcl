// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

// the factory maps every ModLoader variant to its concrete installer.
// covering all five arms catches a misordered match or a copy-paste typo
// that would route, say, NeoForge to the Forge installer.
#[rstest::rstest]
#[case::vanilla(ModLoader::Vanilla)]
#[case::forge(ModLoader::Forge)]
#[case::neoforge(ModLoader::NeoForge)]
#[case::fabric(ModLoader::Fabric)]
#[case::quilt(ModLoader::Quilt)]
fn get_installer_returns_matching_loader_type(#[case] loader: ModLoader) {
    let installer = get_installer(loader);
    assert_eq!(installer.loader_type(), loader);
}

#[test]
fn save_installer_profile_copies_raw_bytes_verbatim() {
    use tempfile::TempDir;
    let tmp = TempDir::new().unwrap();
    let instance_dir = tmp.path().join("instance");
    let meta_dir = tmp.path().join("meta");

    // a synthetic installer version JSON with the modern arguments
    // object - exactly the shape we used to strip.
    let installer_json = br#"{
            "id": "1.20.1-forge-47.2.0",
            "inheritsFrom": "1.20.1",
            "mainClass": "cpw.mods.bootstraplauncher.BootstrapLauncher",
            "libraries": [{ "name": "net.minecraftforge:forge:47.2.0" }],
            "arguments": {
                "game": ["--launchTarget", "forge_client"],
                "jvm": ["--add-opens", "java.base/sun.security.util=cpw.mods.securejarhandler"]
            }
        }"#;

    let ver_dir = instance_dir
        .join(crate::storage::MINECRAFT_DIR_NAME)
        .join("versions")
        .join("1.20.1-forge-47.2.0");
    std::fs::create_dir_all(&ver_dir).unwrap();
    let ver_json_path = ver_dir.join("1.20.1-forge-47.2.0.json");
    std::fs::write(&ver_json_path, installer_json).unwrap();

    save_installer_profile(
        &instance_dir,
        &meta_dir,
        "1.20.1-forge-47.2.0",
        "forge-1.20.1-47.2.0.json",
    )
    .unwrap();

    let saved = std::fs::read(
        meta_dir
            .join("cache/loaders/profiles")
            .join("forge-1.20.1-47.2.0.json"),
    )
    .unwrap();
    assert_eq!(
        saved,
        installer_json.to_vec(),
        "saved profile should be byte-for-byte identical to installer output"
    );
}

#[test]
fn save_installer_profile_rejects_invalid_json_without_overwriting_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let instance_dir = tmp.path().join("instance");
    let meta_dir = tmp.path().join("meta");
    let version_name = "1.20.1-forge-broken";
    let version_dir = instance_dir
        .join(crate::storage::MINECRAFT_DIR_NAME)
        .join("versions")
        .join(version_name);
    std::fs::create_dir_all(&version_dir).unwrap();
    std::fs::write(
        version_dir.join(format!("{version_name}.json")),
        b"not json",
    )
    .unwrap();
    let cached = crate::storage::MetadataPaths::new(&meta_dir)
        .loader_profiles()
        .join("forge-broken.json");
    std::fs::create_dir_all(cached.parent().unwrap()).unwrap();
    std::fs::write(&cached, b"previous").unwrap();

    assert!(
        save_installer_profile(&instance_dir, &meta_dir, version_name, "forge-broken.json")
            .is_err()
    );
    assert_eq!(std::fs::read(cached).unwrap(), b"previous");
}

#[rstest::rstest]
#[case(ModLoader::Vanilla, None)]
#[case(ModLoader::Fabric, Some("fabric-1.21.1-1.0.json"))]
#[case(ModLoader::Quilt, Some("quilt-1.21.1-1.0.json"))]
#[case(ModLoader::Forge, Some("forge-1.21.1-1.0.json"))]
#[case(ModLoader::NeoForge, Some("neoforge-1.0.json"))]
fn profile_filenames_match_launch_cache_names(
    #[case] loader: ModLoader,
    #[case] expected: Option<&str>,
) {
    assert_eq!(
        profile_filename(loader, "1.21.1", "1.0").as_deref(),
        expected
    );
}

// shape-pinning test: a synthetic versionInfo from a 1.7.10 forge
// install_profile.json must deserialise as a LaunchProfile so the
// launch flow's render_args + resolve pipeline can consume it. no
// filesystem round-trip; serde_json directly on the literal bytes.
#[test]
fn legacy_forge_version_info_deserialises_as_launch_profile() {
    let bytes = br#"{
            "id": "1.7.10-Forge10.13.4.1614-1.7.10",
            "mainClass": "net.minecraft.launchwrapper.Launch",
            "minecraftArguments": "--username ${auth_player_name} --tweakClass cpw.mods.fml.common.launcher.FMLTweaker",
            "libraries": [
                { "name": "net.minecraftforge:forge:10.13.4.1614", "url": "http://files.minecraftforge.net/maven/" },
                { "name": "net.minecraft:launchwrapper:1.9" }
            ]
        }"#;

    let profile: crate::launch_profile::model::LaunchProfile =
        serde_json::from_slice(bytes).unwrap();
    assert_eq!(profile.id, "1.7.10-Forge10.13.4.1614-1.7.10");
    assert_eq!(
        profile.main_class.as_deref(),
        Some("net.minecraft.launchwrapper.Launch")
    );
    // legacy forge profiles omit inheritsFrom; the launch flow's
    // implicit fallback adds it before resolve.
    assert!(profile.inherits_from.is_none());
    assert!(
        profile
            .minecraft_arguments
            .as_deref()
            .unwrap()
            .contains("--tweakClass")
    );
    assert_eq!(profile.libraries.len(), 2);
    assert_eq!(
        profile.libraries[0].name,
        "net.minecraftforge:forge:10.13.4.1614"
    );
    assert_eq!(
        profile.libraries[0].url.as_deref(),
        Some("http://files.minecraftforge.net/maven/")
    );
    // legacy libs typically have no downloads.artifact; they resolve
    // at launch time via maven_coord_to_path(name).
    assert!(profile.libraries[0].downloads.is_none());
}

// shape-pinning test: a synthetic upstream fabric profile (no
// inheritsFrom, no arguments, libraries with name+url) must
// deserialise as a LaunchProfile so the install path can write it
// through to disk and the launch flow can read it back.
#[test]
fn raw_fabric_profile_bytes_parse_as_launch_profile() {
    let bytes = br#"{
            "id": "fabric-loader-0.14.21-1.20.1",
            "mainClass": "net.fabricmc.loader.impl.launch.knot.KnotClient",
            "libraries": [
                { "name": "net.fabricmc:fabric-loader:0.14.21", "url": "https://maven.fabricmc.net/" },
                { "name": "net.fabricmc:intermediary:1.20.1", "url": "https://maven.fabricmc.net/" }
            ]
        }"#;

    let parsed: crate::launch_profile::model::LaunchProfile =
        serde_json::from_slice(bytes).unwrap();
    assert_eq!(parsed.id, "fabric-loader-0.14.21-1.20.1");
    assert_eq!(
        parsed.main_class.as_deref(),
        Some("net.fabricmc.loader.impl.launch.knot.KnotClient")
    );
    // upstream Fabric profiles omit inheritsFrom; the launch flow's
    // implicit fallback handles it before resolve.
    assert!(parsed.inherits_from.is_none());
    assert!(parsed.arguments.is_none());
    assert_eq!(parsed.libraries.len(), 2);
    assert_eq!(
        parsed.libraries[0].url.as_deref(),
        Some("https://maven.fabricmc.net/")
    );
}
