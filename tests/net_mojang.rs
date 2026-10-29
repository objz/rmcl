// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use rmcl::net::HttpClient;
use rmcl::net::mojang::{
    Artifact, AssetIndex, Download, JavaVersion, Library, LibraryDownloads, VersionDownloads,
    VersionEntry, VersionMeta, download_assets, download_assets_from,
    download_libraries_for_platform, fetch_version_manifest_from, fetch_version_meta_with_raw,
};

fn test_platform() -> rmcl::launch_profile::system::JavaPlatform {
    rmcl::launch_profile::system::JavaPlatform::from_java_properties(
        match std::env::consts::OS {
            "windows" => "Windows 10",
            "macos" => "Mac OS X",
            _ => "Linux",
        },
        std::env::consts::ARCH,
        if cfg!(target_pointer_width = "64") {
            "64"
        } else {
            "32"
        },
        "10.0",
    )
    .unwrap()
}

async fn download_libraries(
    client: &HttpClient,
    meta: &VersionMeta,
    meta_dir: &std::path::Path,
) -> Result<(), rmcl::net::NetError> {
    download_libraries_for_platform(client, meta, meta_dir, &test_platform()).await
}

fn sha1(bytes: &[u8]) -> String {
    use sha1::Digest;
    format!("{:x}", sha1::Sha1::digest(bytes))
}

fn synthetic_manifest() -> serde_json::Value {
    json!({
        "latest": { "release": "1.20.1", "snapshot": "24w01a" },
        "versions": [
            {
                "id": "1.20.1",
                "type": "release",
                "url": "https://example.com/1.20.1.json",
                "sha1": "0000000000000000000000000000000000000000"
            },
            {
                "id": "1.7.10",
                "type": "release",
                "url": "https://example.com/1.7.10.json",
                "sha1": "0000000000000000000000000000000000000000"
            }
        ]
    })
}

fn synthetic_version_meta() -> serde_json::Value {
    json!({
        "id": "1.20.1",
        "mainClass": "net.minecraft.client.main.Main",
        "assetIndex": {
            "id": "5",
            "url": "https://example.com/assets/5.json",
            "sha1": "0000000000000000000000000000000000000000"
        },
        "downloads": {
            "client": {
                "url": "https://example.com/client.jar",
                "sha1": "0000000000000000000000000000000000000000",
                "size": 12345
            }
        },
        "libraries": [],
        "javaVersion": { "majorVersion": 17 }
    })
}

#[tokio::test]
async fn fetch_version_manifest_parses_synthetic_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/manifest.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(synthetic_manifest()))
        .expect(1)
        .mount(&server)
        .await;

    let url = format!("{}/manifest.json", server.uri());
    let manifest = fetch_version_manifest_from(&HttpClient::new(), &url)
        .await
        .expect("manifest");

    assert_eq!(manifest.latest.release, "1.20.1");
    assert_eq!(manifest.latest.snapshot, "24w01a");
    assert_eq!(manifest.versions.len(), 2);
    assert_eq!(manifest.versions[0].id, "1.20.1");
    assert_eq!(manifest.versions[1].id, "1.7.10");
}

#[tokio::test]
async fn fetch_version_meta_returns_struct_and_raw_bytes() {
    let server = MockServer::start().await;
    let body_json = synthetic_version_meta();
    let body = serde_json::to_vec(&body_json).unwrap();
    Mock::given(method("GET"))
        .and(path("/1.20.1.json"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
        .expect(1)
        .mount(&server)
        .await;

    let entry = VersionEntry {
        id: "1.20.1".to_string(),
        version_type: "release".to_string(),
        url: format!("{}/1.20.1.json", server.uri()),
        sha1: sha1(&body),
    };

    let (meta, raw) = fetch_version_meta_with_raw(&HttpClient::new(), &entry)
        .await
        .expect("meta");

    assert_eq!(meta.id, "1.20.1");
    assert_eq!(meta.main_class, "net.minecraft.client.main.Main");
    assert_eq!(meta.asset_index.id, "5");
    assert_eq!(meta.downloads.client.size, 12345);
    assert_eq!(meta.java_version.unwrap().major_version, 17);

    let reparsed: serde_json::Value = serde_json::from_slice(&raw).expect("raw is json");
    assert_eq!(reparsed["id"], "1.20.1");
    assert_eq!(reparsed["mainClass"], "net.minecraft.client.main.Main");
}

#[tokio::test]
async fn version_metadata_rejects_a_wrong_manifest_hash() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/version.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(synthetic_version_meta()))
        .mount(&server)
        .await;
    let entry = VersionEntry {
        id: "1.20.1".to_owned(),
        version_type: "release".to_owned(),
        url: format!("{}/version.json", server.uri()),
        sha1: "0".repeat(40),
    };
    assert!(
        fetch_version_meta_with_raw(&HttpClient::new(), &entry)
            .await
            .is_err()
    );
}

// constructs a minimal VersionMeta with a single library pointing at the
// wiremock server. lets download_libraries hit synthetic URLs without any
// production-side URL override.
fn meta_with_one_library(server_uri: &str) -> VersionMeta {
    VersionMeta {
        id: "test".to_string(),
        main_class: "net.test.Main".to_string(),
        asset_index: AssetIndex {
            id: "5".to_string(),
            url: format!("{server_uri}/assets/index.json"),
            sha1: "0".repeat(40),
        },
        downloads: VersionDownloads {
            client: Download {
                url: format!("{server_uri}/client.jar"),
                sha1: "0".repeat(40),
                size: 0,
            },
        },
        libraries: vec![Library {
            name: "org.slf4j:slf4j-api:2.0.7".to_string(),
            downloads: LibraryDownloads {
                artifact: Some(Artifact {
                    url: format!("{server_uri}/slf4j.jar"),
                    path: "org/slf4j/slf4j-api/2.0.7/slf4j-api-2.0.7.jar".to_string(),
                    sha1: sha1(b"jar-bytes"),
                    size: b"jar-bytes".len() as u64,
                }),
                classifiers: None,
            },
            rules: None,
            natives: None,
            extract: None,
        }],
        java_version: Some(JavaVersion { major_version: 17 }),
    }
}

#[tokio::test]
async fn download_libraries_writes_artifact_to_meta_dir() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slf4j.jar"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"jar-bytes".to_vec()))
        .expect(1)
        .mount(&server)
        .await;

    let meta = meta_with_one_library(&server.uri());
    let tmp = tempfile::tempdir().unwrap();

    download_libraries(&HttpClient::new(), &meta, tmp.path())
        .await
        .expect("download_libraries");

    let written = tmp
        .path()
        .join("cache/minecraft/libraries/org/slf4j/slf4j-api/2.0.7/slf4j-api-2.0.7.jar");
    assert!(written.exists(), "expected library file at {written:?}");
    let contents = std::fs::read(&written).unwrap();
    assert_eq!(contents, b"jar-bytes");
}

#[tokio::test]
async fn download_libraries_skips_when_destination_exists() {
    // pre-create the destination file with content matching the recorded
    // size + sha1. download_libraries should verify and skip the network
    // entirely; wiremock's expect(0) would panic on any request.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slf4j.jar"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let mut meta = meta_with_one_library(&server.uri());
    let tmp = tempfile::tempdir().unwrap();
    let existing = tmp
        .path()
        .join("cache/minecraft/libraries/org/slf4j/slf4j-api/2.0.7/slf4j-api-2.0.7.jar");
    std::fs::create_dir_all(existing.parent().unwrap()).unwrap();
    std::fs::write(&existing, b"already there").unwrap();

    use sha1::Digest;
    let mut hasher = sha1::Sha1::new();
    hasher.update(b"already there");
    let artifact = meta.libraries[0].downloads.artifact.as_mut().unwrap();
    artifact.sha1 = format!("{:x}", hasher.finalize());
    artifact.size = b"already there".len() as u64;

    download_libraries(&HttpClient::new(), &meta, tmp.path())
        .await
        .expect("noop succeeds");

    assert_eq!(std::fs::read(&existing).unwrap(), b"already there");
}

#[tokio::test]
async fn download_libraries_redownloads_corrupted_cache() {
    // a cached file whose content doesn't match the recorded sha1/size
    // (e.g. a previously killed download) must be replaced, not trusted.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slf4j.jar"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"fresh-bytes".to_vec()))
        .expect(1)
        .mount(&server)
        .await;

    let mut meta = meta_with_one_library(&server.uri());
    let artifact = meta.libraries[0].downloads.artifact.as_mut().unwrap();
    artifact.sha1 = sha1(b"fresh-bytes");
    artifact.size = b"fresh-bytes".len() as u64;
    let tmp = tempfile::tempdir().unwrap();
    let existing = tmp
        .path()
        .join("cache/minecraft/libraries/org/slf4j/slf4j-api/2.0.7/slf4j-api-2.0.7.jar");
    std::fs::create_dir_all(existing.parent().unwrap()).unwrap();
    std::fs::write(&existing, b"truncated").unwrap();

    download_libraries(&HttpClient::new(), &meta, tmp.path())
        .await
        .expect("re-download succeeds");

    assert_eq!(std::fs::read(&existing).unwrap(), b"fresh-bytes");
}

#[tokio::test]
async fn library_rejects_wrong_download_bytes_and_unsafe_paths() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slf4j.jar"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"wrong".to_vec()))
        .expect(1)
        .mount(&server)
        .await;
    let mut meta = meta_with_one_library(&server.uri());
    let temp = tempfile::tempdir().unwrap();
    let library = temp
        .path()
        .join("cache/minecraft/libraries/org/slf4j/slf4j-api/2.0.7/slf4j-api-2.0.7.jar");
    assert!(
        download_libraries(&HttpClient::new(), &meta, temp.path())
            .await
            .is_err()
    );
    assert!(!library.exists());

    meta.libraries[0].downloads.artifact.as_mut().unwrap().path =
        "../../../../escape.jar".to_owned();
    assert!(
        download_libraries(&HttpClient::new(), &meta, temp.path())
            .await
            .is_err()
    );
    assert!(!temp.path().join("escape.jar").exists());
}

#[tokio::test]
async fn download_assets_from_writes_index_and_assets() {
    let server = MockServer::start().await;
    let hash = sha1(b"asset-bytes");
    let index = serde_json::to_vec(&json!({
        "virtual": true,
        "objects": {
            "minecraft/lang/en_us.json": {"hash": hash, "size": b"asset-bytes".len()},
            "lang/en_us.lang": {"hash": hash, "size": b"asset-bytes".len()}
        }
    }))
    .unwrap();
    Mock::given(method("GET"))
        .and(path("/assets/index.json"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(index.clone()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/cdn/{}/{}", &hash[..2], hash)))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"asset-bytes".to_vec()))
        .expect(1)
        .mount(&server)
        .await;

    let mut meta = meta_with_one_library(&server.uri());
    meta.asset_index.sha1 = sha1(&index);
    let tmp = tempfile::tempdir().unwrap();
    let cdn_base = format!("{}/cdn", server.uri());

    download_assets_from(&HttpClient::new(), &meta, tmp.path(), &cdn_base)
        .await
        .expect("download_assets_from");

    let index_path = tmp.path().join("cache/minecraft/assets/indexes/5.json");
    assert!(index_path.exists(), "index file missing");
    let asset_path = tmp
        .path()
        .join("cache/minecraft/assets/objects")
        .join(&hash[..2])
        .join(&hash);
    assert!(asset_path.exists(), "asset file missing");
    assert_eq!(std::fs::read(&asset_path).unwrap(), b"asset-bytes");
    for name in ["minecraft/lang/en_us.json", "lang/en_us.lang"] {
        assert_eq!(
            std::fs::read(
                tmp.path()
                    .join("cache/minecraft/assets/virtual/5")
                    .join(name)
            )
            .unwrap(),
            b"asset-bytes"
        );
    }
    download_assets_from(&HttpClient::new(), &meta, tmp.path(), &cdn_base)
        .await
        .unwrap();
}

#[tokio::test]
async fn download_assets_writes_index_when_objects_is_empty() {
    // an asset index with empty objects exercises the index fetch + write
    // path without triggering individual asset downloads (which go to the
    // hardcoded ASSETS_BASE_URL and can't be wiremocked here).
    let server = MockServer::start().await;
    let index = serde_json::to_vec(&json!({"objects": {}})).unwrap();
    Mock::given(method("GET"))
        .and(path("/assets/index.json"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(index.clone()))
        .expect(1)
        .mount(&server)
        .await;

    let mut meta = meta_with_one_library(&server.uri());
    meta.asset_index.sha1 = sha1(&index);
    let tmp = tempfile::tempdir().unwrap();

    download_assets(&HttpClient::new(), &meta, tmp.path())
        .await
        .expect("download_assets index-only");

    let index_path = tmp.path().join("cache/minecraft/assets/indexes/5.json");
    assert!(
        index_path.exists(),
        "expected asset index at {index_path:?}"
    );
    let body: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&index_path).unwrap()).unwrap();
    assert!(body.get("objects").is_some());
}

#[rstest::rstest]
#[case(b"{\"objects\":{\"fake\":{}}}")]
#[case(b"{\"objects\":{},\"virtual\":false}")]
#[tokio::test]
async fn mismatched_cached_index_is_refetched(#[case] cached: &[u8]) {
    let server = MockServer::start().await;
    let body = serde_json::to_vec(&json!({"objects": {}})).unwrap();
    Mock::given(method("GET"))
        .and(path("/assets/index.json"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
        .expect(1)
        .mount(&server)
        .await;
    let mut meta = meta_with_one_library(&server.uri());
    meta.asset_index.sha1 = sha1(&body);
    let tmp = tempfile::tempdir().unwrap();
    let index_path = tmp.path().join("cache/minecraft/assets/indexes/5.json");
    std::fs::create_dir_all(index_path.parent().unwrap()).unwrap();
    std::fs::write(&index_path, cached).unwrap();

    download_assets(&HttpClient::new(), &meta, tmp.path())
        .await
        .unwrap();
    assert_eq!(std::fs::read(index_path).unwrap(), body);
}

#[tokio::test]
async fn invalid_asset_hash_returns_an_error_without_escaping_or_panicking() {
    let server = MockServer::start().await;
    let body =
        serde_json::to_vec(&json!({"objects": {"bad": {"hash": "aé/../escape", "size": 1}}}))
            .unwrap();
    let mut meta = meta_with_one_library(&server.uri());
    meta.asset_index.sha1 = sha1(&body);
    let tmp = tempfile::tempdir().unwrap();
    let index_path = tmp.path().join("cache/minecraft/assets/indexes/5.json");
    std::fs::create_dir_all(index_path.parent().unwrap()).unwrap();
    std::fs::write(&index_path, body).unwrap();

    assert!(
        download_assets(&HttpClient::new(), &meta, tmp.path())
            .await
            .is_err()
    );
    assert!(!tmp.path().join("escape").exists());
    meta.asset_index.id = "../outside".to_owned();
    assert!(
        download_assets(&HttpClient::new(), &meta, tmp.path())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn downloaded_asset_must_match_index_hash_and_size() {
    let server = MockServer::start().await;
    let hash = sha1(b"expected");
    let body = serde_json::to_vec(&json!({"objects": {"one": {"hash": hash, "size": 8}}})).unwrap();
    Mock::given(method("GET"))
        .and(path("/assets/index.json"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(body.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/cdn/{}/{}", &hash[..2], hash)))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(b"bad bytes".to_vec()))
        .mount(&server)
        .await;
    let mut meta = meta_with_one_library(&server.uri());
    meta.asset_index.sha1 = sha1(&body);
    let tmp = tempfile::tempdir().unwrap();
    let asset = tmp
        .path()
        .join("cache/minecraft/assets/objects")
        .join(&hash[..2])
        .join(&hash);

    assert!(
        download_assets_from(
            &HttpClient::new(),
            &meta,
            tmp.path(),
            &format!("{}/cdn", server.uri())
        )
        .await
        .is_err()
    );
    assert!(!asset.exists());
}

#[tokio::test]
async fn download_libraries_downloads_and_extracts_natives() {
    // pre-1.13-era library: no artifact, only a natives classifier for the
    // running OS. the jar must be cached in libraries/ and unpacked into
    // versions/<id>/natives, honouring extract.exclude.
    let server = MockServer::start().await;

    use std::io::Write as _;
    let mut jar = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    jar.start_file(
        "META-INF/MANIFEST.MF",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    jar.write_all(b"manifest").unwrap();
    jar.start_file("lib/native.so", zip::write::SimpleFileOptions::default())
        .unwrap();
    jar.write_all(b"native-bits").unwrap();
    let jar_bytes = jar.finish().unwrap().into_inner();

    use sha1::Digest;
    let mut hasher = sha1::Sha1::new();
    hasher.update(&jar_bytes);
    let jar_sha1 = format!("{:x}", hasher.finalize());

    Mock::given(method("GET"))
        .and(path("/lwjgl-natives.jar"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(jar_bytes.clone()))
        .expect(1)
        .mount(&server)
        .await;

    let server_uri = server.uri();
    let meta = VersionMeta {
        id: "test".to_string(),
        main_class: "net.test.Main".to_string(),
        asset_index: AssetIndex {
            id: "5".to_string(),
            url: format!("{server_uri}/assets/index.json"),
            sha1: "0".repeat(40),
        },
        downloads: VersionDownloads {
            client: Download {
                url: format!("{server_uri}/client.jar"),
                sha1: "0".repeat(40),
                size: 0,
            },
        },
        libraries: vec![Library {
            name: "org.lwjgl.lwjgl:lwjgl:2.9.4".to_string(),
            downloads: LibraryDownloads {
                artifact: None,
                classifiers: Some(std::collections::HashMap::from([(
                    format!(
                        "natives-linux-{}",
                        if cfg!(target_pointer_width = "64") {
                            "64"
                        } else {
                            "32"
                        }
                    ),
                    Artifact {
                        url: format!("{server_uri}/lwjgl-natives.jar"),
                        path: "org/lwjgl/lwjgl/lwjgl/2.9.4/lwjgl-2.9.4-natives-linux.jar"
                            .to_string(),
                        sha1: jar_sha1,
                        size: jar_bytes.len() as u64,
                    },
                )])),
            },
            rules: None,
            natives: Some(std::collections::HashMap::from([(
                rmcl::launch_profile::system::mojang_os_name().to_string(),
                "natives-linux-${arch}".to_string(),
            )])),
            extract: Some(rmcl::net::mojang::LibraryExtract {
                exclude: Some(vec!["META-INF/".to_string()]),
            }),
        }],
        java_version: Some(JavaVersion { major_version: 8 }),
    };

    let tmp = tempfile::tempdir().unwrap();
    download_libraries(&HttpClient::new(), &meta, tmp.path())
        .await
        .expect("download_libraries with natives");

    let cached_jar = tmp.path().join(
        "cache/minecraft/libraries/org/lwjgl/lwjgl/lwjgl/2.9.4/lwjgl-2.9.4-natives-linux.jar",
    );
    assert_eq!(std::fs::read(&cached_jar).unwrap(), jar_bytes);

    let natives = test_platform().natives_directory(tmp.path(), "test");
    let extracted = natives.join("lib/native.so");
    assert_eq!(
        std::fs::read(&extracted).unwrap(),
        b"native-bits",
        "natives payload must be unpacked next to java.library.path"
    );
    let original_permissions = std::fs::metadata(&extracted).unwrap().permissions();
    let modified = std::fs::metadata(&extracted).unwrap().modified().unwrap();
    let mut protected = original_permissions.clone();
    protected.set_readonly(true);
    std::fs::set_permissions(&extracted, protected).unwrap();
    let reuse = download_libraries(&HttpClient::new(), &meta, tmp.path()).await;
    std::fs::set_permissions(&extracted, original_permissions).unwrap();
    reuse.expect("unchanged natives must be reusable without replacing a protected/loaded library");
    assert_eq!(
        std::fs::metadata(&extracted).unwrap().modified().unwrap(),
        modified
    );
    std::fs::write(&extracted, b"corrupted extraction").unwrap();
    download_libraries(&HttpClient::new(), &meta, tmp.path())
        .await
        .unwrap();
    assert_eq!(std::fs::read(&extracted).unwrap(), b"native-bits");
    assert!(
        !natives.join("META-INF").exists(),
        "extract.exclude prefixes must be skipped"
    );
}

fn native_zip(payload: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("native.dll", zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(payload).unwrap();
    zip.finish().unwrap().into_inner()
}

#[tokio::test]
async fn selected_java_bitness_controls_downloads_and_extraction_is_isolated() {
    let server = MockServer::start().await;
    let mut classifiers = serde_json::Map::new();
    for bits in [32, 64] {
        let bytes = native_zip(bits.to_string().as_bytes());
        let url = format!("/native-{bits}.jar");
        Mock::given(method("GET"))
            .and(path(&url))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.clone()))
            .expect(1)
            .mount(&server)
            .await;
        classifiers.insert(
            format!("natives-windows-{bits}"),
            json!({
                "url":format!("{}{url}", server.uri()), "sha1":sha1(&bytes), "size":bytes.len(),
            }),
        );
    }
    let mut meta = meta_with_one_library(&server.uri());
    meta.libraries = serde_json::from_value(json!([{
        "name":"tv.twitch:twitch-platform:5.16", "downloads":{"classifiers":classifiers},
        "natives":{"windows":"natives-windows-${arch}"},
        "rules":[{"action":"allow", "os":{"name":"windows", "version":"^10\\."}}],
    }]))
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    for (arch, bits) in [("x86", "32"), ("amd64", "64")] {
        let platform = rmcl::launch_profile::system::JavaPlatform::from_java_properties(
            "Windows 10",
            arch,
            bits,
            "10.0",
        )
        .unwrap();
        download_libraries_for_platform(&HttpClient::new(), &meta, temp.path(), &platform)
            .await
            .unwrap();
        let natives = platform.natives_directory(temp.path(), &meta.id);
        assert_eq!(
            std::fs::read(natives.join("native.dll")).unwrap(),
            bits.as_bytes()
        );
    }
    // The second extraction must leave the first architecture's payload intact.
    let java32 = rmcl::launch_profile::system::JavaPlatform::from_java_properties(
        "Windows 10",
        "x86",
        "32",
        "10.0",
    )
    .unwrap();
    assert_eq!(
        std::fs::read(
            java32
                .natives_directory(temp.path(), &meta.id)
                .join("native.dll")
        )
        .unwrap(),
        b"32"
    );
    assert!(
        !temp
            .path()
            .join("cache/minecraft/versions")
            .join(&meta.id)
            .join("natives/native.dll")
            .exists()
    );
    let old_windows = rmcl::launch_profile::system::JavaPlatform::from_java_properties(
        "Windows 7",
        "x86",
        "32",
        "6.1",
    )
    .unwrap();
    let other = tempfile::tempdir().unwrap();
    download_libraries_for_platform(&HttpClient::new(), &meta, other.path(), &old_windows)
        .await
        .unwrap();
    assert!(
        !old_windows
            .natives_directory(other.path(), &meta.id)
            .exists()
    );
}

#[tokio::test]
async fn missing_selected_native_classifier_is_reported() {
    let mut meta = meta_with_one_library("https://example.invalid");
    meta.libraries = serde_json::from_value(json!([{
        "name":"tv.twitch:twitch-platform:5.16", "downloads":{"classifiers":{}},
        "natives":{"windows":"natives-windows-${arch}"},
    }]))
    .unwrap();
    let java32 = rmcl::launch_profile::system::JavaPlatform::from_java_properties(
        "Windows 10",
        "x86",
        "32",
        "10.0",
    )
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let error = download_libraries_for_platform(&HttpClient::new(), &meta, temp.path(), &java32)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("natives-windows-32"));
}
