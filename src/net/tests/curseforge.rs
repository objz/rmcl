// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn build_key_overrides_the_default_and_empty_values_disable_curseforge() {
    for (build_key, default_key, expected) in [
        (None, "", None),
        (None, " default-key ", Some("default-key")),
        (Some(" override-key "), "default-key", Some("override-key")),
        (Some(""), "default-key", None),
        (Some(" \n\t"), "default-key", None),
    ] {
        assert_eq!(select_api_key(build_key, default_key), expected);
    }
}

#[tokio::test(start_paused = true)]
async fn authenticated_requests_retry_transient_failures_with_the_api_key() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    for verb in ["GET", "POST"] {
        let attempts = Arc::new(AtomicUsize::new(0));
        Mock::given(method(verb))
            .and(path(format!("/{verb}")))
            .and(header("x-api-key", "test-key"))
            .respond_with(move |_: &wiremock::Request| {
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    ResponseTemplate::new(503)
                } else {
                    ResponseTemplate::new(200).set_body_json(serde_json::json!({"ok": true}))
                }
            })
            .expect(2)
            .mount(&server)
            .await;
    }
    let client: HttpClient = reqwest::Client::builder().build().unwrap().into();
    let fetched: serde_json::Value = get(&client, "test-key", &format!("{}/GET", server.uri()))
        .await
        .unwrap();
    let posted: serde_json::Value = post(
        &client,
        "test-key",
        &format!("{}/POST", server.uri()),
        &serde_json::json!({}),
    )
    .await
    .unwrap();
    assert!(fetched["ok"].as_bool().unwrap());
    assert!(posted["ok"].as_bool().unwrap());
}

#[test]
fn search_category_ids_match_the_selected_class() {
    let response: ApiResponse<Vec<SearchCategory>> = serde_json::from_str(r#"{"data":[{"id":6,"slug":"mc-mods","name":"Mods","classId":null},{"id":11,"slug":"magic","name":"Magic","classId":6},{"id":22,"slug":"magic","name":"Magic","classId":4471}]}"#).unwrap();
    assert_eq!(category_id(&response.data, 6, "magic"), Some(11));
    assert_eq!(category_id(&response.data, 4471, "magic"), Some(22));
    assert_eq!(category_id(&response.data, 12, "magic"), None);
}

#[test]
fn search_versions_use_curseforge_array_encoding() {
    assert!(search_version_params(&[]).is_empty());
    assert_eq!(
        search_version_params(&["1.21.1".to_owned()]),
        ["gameVersion=1.21.1"]
    );
    assert_eq!(
        search_version_params(&["1.21.1".to_owned(), "1.20.1".to_owned()]),
        ["gameVersions=%5B%221.21.1%22%2C%221.20.1%22%5D"]
    );
}

#[test]
fn curseforge_file_maps_to_shared_version() {
    let file: File = serde_json::from_str(
        r#"{
            "id": 9,
            "modId": 7,
            "displayName": "Example 1.0",
            "fileName": "example.jar",
            "fileLength": 12,
            "downloadUrl": "https://example.invalid/example.jar",
            "gameVersions": ["1.21.1", "Fabric"],
            "releaseType": 2,
            "dependencies": [
                {"modId": 8, "relationType": 3},
                {"modId": 9, "relationType": 2},
                {"modId": 10, "relationType": 5}
            ],
            "hashes": [{"value": "abc", "algo": 1}]
        }"#,
    )
    .unwrap();
    let version = version_info(file);
    assert_eq!(version.project_id, "7");
    assert_eq!(version.loaders, ["fabric"]);
    assert_eq!(version.version_type, VersionType::Beta);
    assert_eq!(
        version
            .dependencies
            .iter()
            .map(|dependency| dependency.dependency_type)
            .collect::<Vec<_>>(),
        [
            DependencyType::Required,
            DependencyType::Optional,
            DependencyType::Incompatible
        ]
    );
    assert_eq!(version.files[0].hashes["sha1"], "abc");
}

#[tokio::test]
async fn fingerprint_resolution_uses_file_fingerprint_and_keeps_exact_hashes() {
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/fingerprints/432"))
        .and(header("x-api-key", "test-key"))
        .and(body_json(serde_json::json!({"fingerprints": [12345]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": {"exactMatches": [{"id": 7, "file": {
                "id": 9, "modId": 7, "displayName": "Example", "fileName": "example.jar",
                "fileFingerprint": 12345, "hashes": [{"algo": 1, "value": "abc"}]
            }}]}
        })))
        .expect(1)
        .mount(&server)
        .await;
    let resolved =
        resolve_fingerprint_versions_from(&HttpClient::new(), "test-key", &server.uri(), &[12345])
            .await
            .unwrap();
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].0, 12345);
    assert_eq!(resolved[0].1.project_id, "7");
    assert_eq!(resolved[0].1.id, "9");
    assert_eq!(resolved[0].1.files[0].hashes["sha1"], "abc");
}

#[test]
fn curseforge_library_category_maps_to_cleanup_metadata() {
    let project: Mod = serde_json::from_str(
        r#"{
            "id": 7,
            "name": "Library",
            "slug": "library",
            "categories": [{
                "name": "API and Library",
                "slug": "library-api"
            }]
        }"#,
    )
    .unwrap();

    assert!(project_info(project, String::new()).is_library_only());
}

#[test]
fn discovery_hides_projects_that_block_third_party_downloads() {
    let project: Mod = serde_json::from_str(
        r#"{
            "id": 7,
            "name": "Restricted",
            "slug": "restricted",
            "allowModDistribution": false
        }"#,
    )
    .unwrap();

    assert!(discovery_project(project).is_none());
}

#[test]
fn discovery_metadata_preserves_all_creators_and_tolerates_missing_creators() {
    for authors in [
        serde_json::json!([]),
        serde_json::json!([{"name": "Creator"}, {"name": "Maintainer"}]),
    ] {
        let project: Mod = serde_json::from_value(serde_json::json!({
            "id": 7, "name": "Example", "slug": "example", "authors": authors
        }))
        .unwrap();
        let expected = authors
            .as_array()
            .unwrap()
            .iter()
            .map(|author| author["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(discovery_metadata(&project).authors, expected);
    }
    let project: Mod =
        serde_json::from_str(r#"{"id":7,"name":"Example","slug":"example"}"#).unwrap();
    assert!(discovery_metadata(&project).authors.is_empty());
}

#[test]
fn datapack_discovery_uses_the_curseforge_data_packs_class() {
    assert_eq!(class_id(ContentKind::DataPack), 6945);
}

#[test]
fn discovery_sort_maps_to_curseforge_fields() {
    use crate::instance::content::provider::DiscoverySort;

    assert_eq!(curseforge_sort_field(DiscoverySort::Relevance, "sodium"), 2);
    assert_eq!(curseforge_sort_field(DiscoverySort::Relevance, ""), 6);
    assert_eq!(curseforge_sort_field(DiscoverySort::Downloads, ""), 6);
    assert_eq!(curseforge_sort_field(DiscoverySort::Popular, ""), 2);
    assert_eq!(curseforge_sort_field(DiscoverySort::Updated, ""), 3);
    assert_eq!(curseforge_sort_field(DiscoverySort::Released, ""), 11);
}

#[tokio::test]
async fn curseforge_versions_follow_pagination() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let files = |range: std::ops::Range<u64>| {
        range
            .map(|id| {
                serde_json::json!({
                    "id": id,
                    "modId": 7,
                    "displayName": format!("Version {id}"),
                    "fileName": format!("{id}.jar"),
                    "fileDate": if id == 50 { "2026-01-02T00:00:00Z" } else { "2026-01-01T00:00:00Z" }
                })
            })
            .collect::<Vec<_>>()
    };
    Mock::given(method("GET"))
        .and(path("/mods/7/files"))
        .and(query_param("index", "0"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": files(0..50),
            "pagination": {"totalCount": 51}
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/mods/7/files"))
        .and(query_param("index", "50"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": files(50..51),
            "pagination": {"totalCount": 51}
        })))
        .mount(&server)
        .await;

    let versions =
        fetch_versions_from(&HttpClient::new(), "test-key", &server.uri(), "7", "", None)
            .await
            .unwrap();
    assert_eq!(versions.len(), 51);
    assert_eq!(versions[0].id, "50");
    assert_eq!(versions[1].id, "0");
}

#[tokio::test]
async fn restricted_curseforge_download_has_an_actionable_error() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/mods/7/files/9/download-url"))
        .respond_with(ResponseTemplate::new(403))
        .mount(&server)
        .await;
    let mut version = version_info(
        serde_json::from_value(serde_json::json!({
            "id": 9,
            "modId": 7,
            "displayName": "Restricted",
            "fileName": "restricted.jar"
        }))
        .unwrap(),
    );

    let error =
        ensure_download_url_from(&HttpClient::new(), "test-key", &server.uri(), &mut version)
            .await
            .unwrap_err();

    assert!(matches!(error, NetError::Parse(message) if message.contains("third-party launchers")));
}
