// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn reversed_search_pages_cover_every_result_once() {
    let ids: Vec<_> = (0..11).collect();
    let pages = (0..11)
        .step_by(4)
        .flat_map(|offset| {
            let (start, count) = reversed_window(ids.len(), offset, 4).unwrap();
            ids[start..start + count]
                .iter()
                .rev()
                .copied()
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    assert_eq!(pages, (0..11).rev().collect::<Vec<_>>());
    assert_eq!(reversed_window(11, 11, 4), None);
}

fn version_with_files(files: Vec<VersionFile>) -> VersionInfo {
    VersionInfo {
        id: "version-id".to_owned(),
        project_id: "project-id".to_owned(),
        name: "Version 1".to_owned(),
        version_number: "1.0.0".to_owned(),
        game_versions: vec!["1.21.1".to_owned()],
        loaders: vec!["fabric".to_owned()],
        version_type: VersionType::Release,
        dependencies: Vec::new(),
        date_published: String::new(),
        files,
    }
}

#[test]
fn version_dependencies_and_release_type_are_deserialized() {
    let version: VersionInfo = serde_json::from_str(
        r#"{
            "id": "version",
            "project_id": "project",
            "name": "Beta",
            "version_number": "2.0.0",
            "game_versions": ["1.21.1"],
            "loaders": ["fabric"],
            "version_type": "beta",
            "dependencies": [{
                "project_id": "fabric-api",
                "dependency_type": "required"
            }],
            "files": []
        }"#,
    )
    .unwrap();

    assert_eq!(version.version_type, VersionType::Beta);
    assert_eq!(version.dependencies.len(), 1);
    assert_eq!(
        version.dependencies[0].project_id.as_deref(),
        Some("fabric-api")
    );
    assert_eq!(
        version.dependencies[0].dependency_type,
        DependencyType::Required
    );
}

#[test]
fn only_exclusively_library_categorized_projects_are_cleanup_eligible() {
    let project = |categories: &[&str]| ProjectInfo {
        id: "project".to_owned(),
        slug: "project".to_owned(),
        title: "Project".to_owned(),
        description: String::new(),
        body: String::new(),
        icon_url: None,
        categories: categories
            .iter()
            .map(|category| (*category).to_owned())
            .collect(),
        additional_categories: Vec::new(),
        project_type: "mod".to_owned(),
        loaders: Vec::new(),
        ..ProjectInfo::default()
    };

    assert!(project(&["library"]).is_library_only());
    assert!(project(&["api-and-library"]).is_library_only());
    assert!(!project(&[]).is_library_only());
    assert!(!project(&["library", "utility"]).is_library_only());
}

#[test]
fn project_metadata_caches_environment_fields() {
    let project: ProjectInfo = serde_json::from_str(
        r#"{"id":"test","slug":"test","title":"Test","updated":"2025-02-01T00:00:00Z","client_side":"required","server_side":"unsupported"}"#,
    )
    .unwrap();
    assert_eq!(project.date_modified, "2025-02-01T00:00:00Z");
    assert_eq!(project.client_side, "required");
}

#[test]
fn discovery_mod_facets_include_instance_compatibility() {
    let filters = crate::instance::content::provider::DiscoverySearchFilters {
        game_versions: vec!["1.21.1".to_owned()],
        ..Default::default()
    };
    let facets = discovery_facets(ContentKind::Mod, &filters, ModLoader::Fabric);
    assert_eq!(
        serde_json::from_str::<Vec<Vec<String>>>(&facets).unwrap(),
        vec![
            vec!["project_type:mod"],
            vec!["versions:1.21.1"],
            vec!["categories:fabric"],
        ]
    );
}

#[test]
fn discovery_facets_match_any_selected_minecraft_version() {
    let filters = crate::instance::content::provider::DiscoverySearchFilters {
        game_versions: vec!["1.21.1".to_owned(), "1.20.1".to_owned()],
        ..Default::default()
    };
    let facets = discovery_facets(ContentKind::Mod, &filters, ModLoader::Fabric);
    assert_eq!(
        serde_json::from_str::<Vec<Vec<String>>>(&facets).unwrap(),
        vec![
            vec!["project_type:mod"],
            vec!["versions:1.21.1", "versions:1.20.1"],
            vec!["categories:fabric"],
        ]
    );
}

#[test]
fn discovery_facets_filter_included_and_excluded_categories_and_versions() {
    let filters = crate::instance::content::provider::DiscoverySearchFilters {
        game_versions: vec!["1.21.1".to_owned()],
        excluded_versions: vec!["1.20.1".to_owned(), "1.19.4".to_owned()],
        included_categories: vec!["magic".to_owned(), "technology".to_owned()],
        excluded_categories: vec!["cursed".to_owned(), "library".to_owned()],
    };
    let facets = discovery_facets(ContentKind::Mod, &filters, ModLoader::Fabric);
    assert_eq!(
        serde_json::from_str::<Vec<Vec<String>>>(&facets).unwrap(),
        vec![
            vec!["project_type:mod"],
            vec!["versions:1.21.1"],
            vec!["categories:fabric"],
            vec!["categories:magic", "categories:technology"],
            vec!["categories!=cursed"],
            vec!["categories!=library"],
            vec!["versions!=1.20.1"],
            vec!["versions!=1.19.4"],
        ]
    );
}

#[test]
fn discovery_resource_pack_facets_do_not_require_loader() {
    let filters = crate::instance::content::provider::DiscoverySearchFilters {
        game_versions: vec!["1.20.1".to_owned()],
        ..Default::default()
    };
    let facets = discovery_facets(ContentKind::ResourcePack, &filters, ModLoader::Forge);
    assert_eq!(
        serde_json::from_str::<Vec<Vec<String>>>(&facets).unwrap(),
        vec![vec!["project_type:resourcepack"], vec!["versions:1.20.1"]]
    );
}

#[test]
fn discovery_datapack_facets_use_the_datapack_project_type() {
    let filters = crate::instance::content::provider::DiscoverySearchFilters {
        game_versions: vec!["1.21.1".to_owned()],
        ..Default::default()
    };
    let facets = discovery_facets(ContentKind::DataPack, &filters, ModLoader::Fabric);
    assert_eq!(
        serde_json::from_str::<Vec<Vec<String>>>(&facets).unwrap(),
        vec![vec!["all_project_types:datapack"], vec!["versions:1.21.1"]]
    );
}

#[test]
fn compatible_mod_versions_filter_by_game_and_loader() {
    let url = content_versions_url(
        "https://example.test/v2",
        "fabric-api",
        ContentKind::Mod,
        "1.21.1",
        ModLoader::Fabric,
    );
    assert_eq!(
        url,
        "https://example.test/v2/project/fabric-api/version?include_changelog=false&game_versions=%5B%221.21.1%22%5D&loaders=%5B%22fabric%22%5D"
    );
}

#[test]
fn compatible_resource_pack_versions_do_not_filter_by_loader() {
    let url = content_versions_url(
        "https://example.test/v2",
        "stay-true",
        ContentKind::ResourcePack,
        "1.21.1",
        ModLoader::Fabric,
    );
    assert_eq!(
        url,
        "https://example.test/v2/project/stay-true/version?include_changelog=false&game_versions=%5B%221.21.1%22%5D"
    );
}

#[test]
fn compatible_datapack_versions_filter_by_datapack_loader() {
    let url = content_versions_url(
        "https://example.test/v2",
        "terralith",
        ContentKind::DataPack,
        "1.21.1",
        ModLoader::Fabric,
    );
    assert_eq!(
        url,
        "https://example.test/v2/project/terralith/version?include_changelog=false&game_versions=%5B%221.21.1%22%5D&loaders=%5B%22datapack%22%5D"
    );
}

#[test]
fn compatible_versions_can_span_all_minecraft_versions() {
    let url = content_versions_url(
        "https://example.test/v2",
        "fabric-api",
        ContentKind::Mod,
        "",
        ModLoader::Fabric,
    );
    assert_eq!(
        url,
        "https://example.test/v2/project/fabric-api/version?include_changelog=false&loaders=%5B%22fabric%22%5D"
    );
}

#[test]
fn primary_file_selection_falls_back_to_first_file() {
    let version = version_with_files(vec![
        VersionFile {
            url: "https://example.test/first.jar".to_owned(),
            filename: "first.jar".to_owned(),
            size: 1,
            primary: false,
            hashes: HashMap::new(),
        },
        VersionFile {
            url: "https://example.test/primary.jar".to_owned(),
            filename: "primary.jar".to_owned(),
            size: 1,
            primary: true,
            hashes: HashMap::new(),
        },
    ]);
    assert_eq!(
        select_primary_file(&version).unwrap().filename,
        "primary.jar"
    );

    let fallback = version_with_files(vec![VersionFile {
        url: "https://example.test/first.jar".to_owned(),
        filename: "first.jar".to_owned(),
        size: 1,
        primary: false,
        hashes: HashMap::new(),
    }]);
    assert_eq!(
        select_primary_file(&fallback).unwrap().filename,
        "first.jar"
    );
}

#[test]
fn modpack_versions_are_not_limited_to_hardcoded_loaders() {
    assert_eq!(
        versions_url("https://example.test/v2", "vanilla pack"),
        "https://example.test/v2/project/vanilla%20pack/version"
    );
}

#[tokio::test]
async fn content_download_skips_an_existing_filename() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("example.jar");
    std::fs::write(&path, b"existing").unwrap();
    let version = version_with_files(vec![VersionFile {
        url: "https://example.test/example.jar".to_owned(),
        filename: "example.jar".to_owned(),
        size: b"existing".len() as u64,
        primary: true,
        hashes: HashMap::new(),
    }]);

    let outcome = download_version_file(&crate::net::HttpClient::new(), &version, directory.path())
        .await
        .unwrap();

    assert_eq!(outcome, DownloadOutcome::SkippedExisting(path));
    assert_eq!(
        std::fs::read(directory.path().join("example.jar")).unwrap(),
        b"existing"
    );
}

#[tokio::test]
async fn content_download_rejects_provider_path_components() {
    let directory = tempfile::tempdir().unwrap();
    let version = version_with_files(vec![VersionFile {
        url: "https://example.test/escape.jar".to_owned(),
        filename: "../escape.jar".to_owned(),
        size: 1,
        primary: true,
        hashes: HashMap::new(),
    }]);

    let error = download_version_file(&crate::net::HttpClient::new(), &version, directory.path())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("Invalid provider filename"));
    assert!(
        !directory
            .path()
            .parent()
            .unwrap()
            .join("escape.jar")
            .exists()
    );
}

#[tokio::test]
async fn staged_update_replaces_the_old_file_and_cleans_its_backup() {
    let directory = tempfile::tempdir().unwrap();
    let installed = directory.path().join("example.jar");
    let temporary = directory.path().join(".example.jar.rmcl-download");
    let backup = directory.path().join(".example.jar.rmcl-backup");
    std::fs::write(&installed, b"old version").unwrap();
    std::fs::write(&temporary, b"new version").unwrap();

    replace_installed_file(&temporary, &installed, &installed, &backup)
        .await
        .unwrap();

    assert_eq!(std::fs::read(installed).unwrap(), b"new version");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn discovery_search_ignores_new_modrinth_enum_values() {
    let response: DiscoverySearchResponse = serde_json::from_str(
        r#"{
                "hits": [{
                    "project_id": "project-id",
                    "slug": "example",
                    "title": "Example",
                    "description": "Example project",
                    "downloads": 42,
                    "icon_url": null,
                    "client_side": "unknown",
                    "server_side": "unknown"
                }],
                "total_hits": 1
            }"#,
    )
    .unwrap();

    assert_eq!(response.hits.len(), 1);
    assert_eq!(response.hits[0].project_id, "project-id");
    assert_eq!(response.total_hits, 1);
}

#[test]
fn discovery_search_treats_blank_icon_urls_as_missing() {
    let hit: DiscoverySearchHit = serde_json::from_str(
        r#"{
                "project_id": "project-id",
                "slug": "example",
                "title": "Example",
                "description": "Example project",
                "icon_url": "   "
            }"#,
    )
    .unwrap();

    assert!(DiscoveryProject::from(hit).icon_url.is_none());
}

#[test]
fn discovery_sort_maps_to_modrinth_indexes() {
    use crate::instance::content::provider::DiscoverySort;

    assert_eq!(
        discovery_index(DiscoverySort::Relevance, "sodium"),
        "relevance"
    );
    assert_eq!(discovery_index(DiscoverySort::Relevance, ""), "downloads");
    assert_eq!(
        discovery_index(DiscoverySort::Downloads, "sodium"),
        "downloads"
    );
    assert_eq!(discovery_index(DiscoverySort::Popular, ""), "follows");
    assert_eq!(discovery_index(DiscoverySort::Updated, ""), "updated");
    assert_eq!(discovery_index(DiscoverySort::Newest, ""), "newest");
}

// covers each branch of url_encode: unreserved bytes pass through; the
// reserved set + spaces + non-ascii bytes get percent-encoded. emoji
// exercises multi-byte UTF-8 since the encoder operates on bytes, not
// chars, so each byte of the codepoint encodes separately.
#[rstest::rstest]
#[case::ascii_unreserved("abcXYZ0-9_.~", "abcXYZ0-9_.~")]
#[case::space("hello world", "hello%20world")]
#[case::reserved("/?&=#", "%2F%3F%26%3D%23")]
#[case::utf8_emoji("\u{2603}", "%E2%98%83")]
#[case::empty("", "")]
fn url_encode_handles(#[case] input: &str, #[case] expected: &str) {
    assert_eq!(url_encode(input), expected);
}
