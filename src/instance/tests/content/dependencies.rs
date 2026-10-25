// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::Utc;

use super::*;
use crate::instance::content::provider::{DiscoverySort, FingerprintQuery, ResolvedFile};
use crate::instance::{FileFingerprint, ModLoader, Resolution};
use crate::net::modrinth::{
    DependencyType, DiscoveryResults, ProjectInfo, VersionDependency, VersionType,
};

#[test]
fn concurrent_installs_use_distinct_staging_directories() {
    let temp = tempfile::tempdir().unwrap();
    let first = StagingDirectory::new(temp.path()).unwrap();
    let second = StagingDirectory::new(temp.path()).unwrap();
    assert_ne!(first.path(), second.path());
}

#[test]
fn merge_keeps_replacement_when_a_dependency_is_also_an_update_root() {
    let selected = version(
        "library-new",
        "library",
        VersionType::Release,
        "2026-02-01",
        Vec::new(),
    );
    let item = |replacement: bool| PlannedInstall {
        provider: "modrinth".to_owned(),
        project_id: "library".to_owned(),
        title: "Library".to_owned(),
        version: selected.clone(),
        installed_path: Some(PathBuf::from("/minecraft/mods/library.jar")),
        expected_record: None,
        kind: ContentKind::Mod,
        destination: PathBuf::from("/minecraft/mods"),
        provider_aliases: Vec::new(),
        required_dependencies: Vec::new(),
        automatic_dependency: !replacement,
        cleanup_eligible: !replacement,
        replacement,
    };

    let merged = merge(vec![
        DependencyPlan {
            items: vec![item(false)],
            root_count: 0,
            optional_dependencies: 0,
        },
        DependencyPlan {
            items: vec![item(true)],
            root_count: 1,
            optional_dependencies: 0,
        },
    ])
    .unwrap();

    assert_eq!(merged.root_count, 1);
    assert!(merged.items[0].replacement);
    assert!(!merged.items[0].automatic_dependency);
}

#[tokio::test]
async fn invalid_manifest_does_not_replace_installed_content() {
    let temp = tempfile::tempdir().unwrap();
    let minecraft = temp.path().join("minecraft");
    let mods = minecraft.join("mods");
    tokio::fs::create_dir_all(&mods).await.unwrap();
    let old = mods.join("old.jar");
    tokio::fs::write(&old, b"original").await.unwrap();
    let manifest = temp.path().join("content.json");
    tokio::fs::write(&manifest, b"{bad JSON").await.unwrap();
    let selected = version(
        "new",
        "root",
        VersionType::Release,
        "2026-02-01",
        Vec::new(),
    );
    let plan = DependencyPlan {
        items: vec![PlannedInstall {
            provider: "modrinth".to_owned(),
            project_id: "root".to_owned(),
            title: "Root".to_owned(),
            version: selected.clone(),
            installed_path: Some(old.clone()),
            expected_record: None,
            kind: ContentKind::Mod,
            destination: mods.clone(),
            provider_aliases: Vec::new(),
            required_dependencies: Vec::new(),
            automatic_dependency: false,
            cleanup_eligible: false,
            replacement: true,
        }],
        root_count: 1,
        optional_dependencies: 0,
    };

    assert!(
        install(
            &provider(vec![selected]).registry(),
            &manifest,
            &minecraft,
            &plan
        )
        .await
        .is_err()
    );
    assert_eq!(tokio::fs::read(old).await.unwrap(), b"original");
    assert!(!mods.join("new.jar").exists());
}

struct FakeProvider {
    versions: HashMap<String, VersionInfo>,
    compatible: HashMap<String, Vec<String>>,
    projects: HashMap<String, String>,
    project_types: HashMap<String, String>,
    categories: HashMap<String, Vec<String>>,
    resolved: HashMap<String, ProviderProject>,
    fail_project: Option<String>,
    fail_download: Option<String>,
    download_barrier: Option<std::sync::Arc<tokio::sync::Barrier>>,
    download_pause: Option<std::sync::Arc<tokio::sync::Notify>>,
    download_filename: Option<String>,
    compatible_started: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    compatible_pause: HashMap<String, std::sync::Arc<tokio::sync::Semaphore>>,
}

impl FakeProvider {
    fn registry(self) -> ProviderRegistry {
        ProviderRegistry::new(vec![Box::new(self)])
    }
}

#[async_trait]
impl ContentProvider for FakeProvider {
    fn id(&self) -> &'static str {
        "modrinth"
    }

    async fn search(
        &self,
        _kind: ContentKind,
        _query: &str,
        _instance: &InstanceConfig,
        _filters: &crate::instance::content::provider::DiscoverySearchFilters,
        _sort: DiscoverySort,
        _reversed: bool,
        _offset: usize,
        _limit: usize,
    ) -> Result<DiscoveryResults, NetError> {
        unreachable!()
    }

    async fn search_modpacks(
        &self,
        _query: &str,
        _filters: &crate::instance::content::provider::DiscoverySearchFilters,
        _sort: DiscoverySort,
        _reversed: bool,
        _offset: usize,
        _limit: usize,
    ) -> Result<DiscoveryResults, NetError> {
        unreachable!()
    }

    async fn resolve_files(
        &self,
        files: &[FingerprintQuery],
    ) -> Result<Vec<ResolvedFile>, NetError> {
        Ok(files
            .iter()
            .filter_map(|file| {
                self.resolved
                    .get(&file.key)
                    .cloned()
                    .map(|project| ResolvedFile {
                        key: file.key.clone(),
                        project,
                    })
            })
            .collect())
    }

    async fn project(&self, project_id: &str) -> Result<ProjectInfo, NetError> {
        if self.fail_project.as_deref() == Some(project_id) {
            return Err(NetError::Parse(format!(
                "failed to load project {project_id}"
            )));
        }
        Ok(ProjectInfo {
            id: project_id.to_owned(),
            slug: project_id.to_owned(),
            title: self
                .projects
                .get(project_id)
                .cloned()
                .unwrap_or_else(|| project_id.to_owned()),
            description: String::new(),
            body: String::new(),
            icon_url: None,
            categories: self.categories.get(project_id).cloned().unwrap_or_default(),
            additional_categories: Vec::new(),
            project_type: self
                .project_types
                .get(project_id)
                .cloned()
                .unwrap_or_else(|| "mod".to_owned()),
            loaders: Vec::new(),
            ..ProjectInfo::default()
        })
    }

    async fn compatible_versions(
        &self,
        project_id: &str,
        _kind: ContentKind,
        _game_version: &str,
        _loader: ModLoader,
    ) -> Result<Vec<VersionInfo>, NetError> {
        if let Some(started) = &self.compatible_started {
            let _ = started.send(project_id.to_owned());
        }
        if let Some(pause) = self.compatible_pause.get(project_id) {
            pause.acquire().await.unwrap().forget();
        }
        Ok(self
            .compatible
            .get(project_id)
            .into_iter()
            .flatten()
            .filter_map(|id| self.versions.get(id).cloned())
            .collect())
    }

    async fn version(&self, version_id: &str) -> Result<VersionInfo, NetError> {
        self.versions
            .get(version_id)
            .cloned()
            .ok_or_else(|| NetError::Parse(format!("missing version {version_id}")))
    }

    async fn icon(&self, _url: &str) -> Result<Vec<u8>, NetError> {
        unreachable!()
    }

    async fn download_version(
        &self,
        version: &VersionInfo,
        destination: &Path,
        _installed_path: Option<&Path>,
    ) -> Result<crate::net::modrinth::DownloadOutcome, NetError> {
        if self.fail_download.as_deref() == Some(version.id.as_str()) {
            return Err(NetError::Parse(format!(
                "failed to download {}",
                version.id
            )));
        }
        let extension = if version.loaders.iter().any(|loader| loader == "datapack") {
            "zip"
        } else {
            "jar"
        };
        let path = destination.join(
            self.download_filename
                .clone()
                .unwrap_or_else(|| format!("{}.{extension}", version.id)),
        );
        tokio::fs::write(&path, version.id.as_bytes()).await?;
        if let Some(barrier) = &self.download_barrier {
            barrier.wait().await;
        }
        if let Some(pause) = &self.download_pause {
            pause.notified().await;
        }
        Ok(crate::net::modrinth::DownloadOutcome::Downloaded(path))
    }
}

fn dependency(project_id: &str, dependency_type: DependencyType) -> VersionDependency {
    VersionDependency {
        version_id: None,
        project_id: Some(project_id.to_owned()),
        file_name: None,
        dependency_type,
    }
}

fn version(
    id: &str,
    project_id: &str,
    version_type: VersionType,
    date: &str,
    dependencies: Vec<VersionDependency>,
) -> VersionInfo {
    VersionInfo {
        id: id.to_owned(),
        project_id: project_id.to_owned(),
        name: id.to_owned(),
        version_number: id.to_owned(),
        game_versions: vec!["1.21.1".to_owned()],
        loaders: vec!["fabric".to_owned()],
        version_type,
        dependencies,
        date_published: date.to_owned(),
        files: Vec::new(),
    }
}

fn instance() -> InstanceConfig {
    InstanceConfig {
        name: "Test".to_owned(),
        game_version: "1.21.1".to_owned(),
        loader: ModLoader::Fabric,
        loader_version: Some("0.16.0".to_owned()),
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
    }
}

fn root(version: VersionInfo) -> InstallRoot {
    InstallRoot {
        provider: "modrinth".to_owned(),
        project_id: version.project_id.clone(),
        title: "Root".to_owned(),
        version,
        installed_path: None,
        kind: ContentKind::Mod,
        target_world: None,
        force_reinstall: false,
    }
}

fn installed_record(project_id: &str, version_id: &str, enabled: bool) -> ContentFileRecord {
    ContentFileRecord {
        relative_path: PathBuf::from(format!("mods/{project_id}.jar")),
        kind: ContentKind::Mod,
        enabled,
        fingerprint: FileFingerprint {
            size: 1,
            modified_ns: 1,
            hashes: Default::default(),
        },
        resolution: Resolution::Resolved {
            project: ProviderProject {
                provider: "modrinth".to_owned(),
                project_id: project_id.to_owned(),
                version_id: version_id.to_owned(),
            },
        },
        provider_aliases: Vec::new(),
        provider_checks: Vec::new(),
        required_dependencies: Vec::new(),
        automatic_dependency: false,
        cleanup_eligible: false,
    }
}

fn provider(versions: Vec<VersionInfo>) -> FakeProvider {
    let compatible = versions.iter().fold(
        HashMap::<String, Vec<String>>::new(),
        |mut compatible, version| {
            compatible
                .entry(version.project_id.clone())
                .or_default()
                .push(version.id.clone());
            compatible
        },
    );
    let projects = versions
        .iter()
        .map(|version| (version.project_id.clone(), version.project_id.clone()))
        .collect();
    let categories = versions
        .iter()
        .map(|version| (version.project_id.clone(), vec!["library".to_owned()]))
        .collect();
    FakeProvider {
        versions: versions
            .into_iter()
            .map(|version| (version.id.clone(), version))
            .collect(),
        compatible,
        projects,
        project_types: HashMap::new(),
        categories,
        resolved: HashMap::new(),
        fail_project: None,
        fail_download: None,
        download_barrier: None,
        download_pause: None,
        download_filename: None,
        compatible_started: None,
        compatible_pause: HashMap::new(),
    }
}

pub(crate) fn controlled_update_registry(
    versions: Vec<VersionInfo>,
    started: tokio::sync::mpsc::UnboundedSender<String>,
    pauses: HashMap<String, std::sync::Arc<tokio::sync::Semaphore>>,
) -> ProviderRegistry {
    let mut provider = provider(versions);
    provider.compatible_started = Some(started);
    provider.compatible_pause = pauses;
    provider.registry()
}

#[test]
fn update_checks_prioritize_list_rows_and_publish_before_the_rest_finish() {
    use crate::instance::content::updates::{AvailableUpdate, UpdateSnapshot, scan_with_registry};
    let _guard = crate::tests::TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let mut manifest = ContentManifest::default();
            let mut versions = Vec::new();
            let mut pauses = HashMap::new();
            for index in 0..12 {
                let project = format!("project-{index:02}");
                let old = format!("{project}-old");
                manifest.files.push(installed_record(&project, &old, true));
                for (suffix, date) in [
                    ("old", "2026-01-01T00:00:00Z"),
                    ("new", "2026-02-01T00:00:00Z"),
                ] {
                    versions.push(version(
                        &format!("{project}-{suffix}"),
                        &project,
                        VersionType::Release,
                        date,
                        Vec::new(),
                    ));
                }
                pauses.insert(project, std::sync::Arc::new(tokio::sync::Semaphore::new(0)));
            }
            let priority = manifest
                .files
                .iter()
                .rev()
                .map(|record| record.resolved_project().unwrap().clone())
                .collect::<Vec<_>>();
            let first = priority[0].clone();
            let known = priority.last().unwrap().clone();
            let previous = UpdateSnapshot {
                game_version: "1.21.1".to_owned(),
                loader: ModLoader::Fabric,
                inventory: priority.clone(),
                checked_at: 0,
                updates: vec![AvailableUpdate {
                    installed: known.clone(),
                    current: None,
                    target: versions
                        .iter()
                        .find(|version| version.id == format!("{}-new", known.project_id))
                        .unwrap()
                        .clone(),
                    kind: ContentKind::Mod,
                }],
                failures: Vec::new(),
            };
            let (started, mut requests) = tokio::sync::mpsc::unbounded_channel();
            let (published, mut results) = tokio::sync::mpsc::unbounded_channel();
            let mut provider = provider(versions);
            provider
                .compatible
                .insert(known.project_id.clone(), vec![known.version_id.clone()]);
            provider.compatible_started = Some(started);
            provider.compatible_pause = pauses.clone();
            let registry = std::sync::Arc::new(provider.registry());
            let expected = priority[..8]
                .iter()
                .map(|project| project.project_id.clone())
                .collect::<HashSet<_>>();
            let scan = tokio::spawn(async move {
                let complete = scan_with_registry(
                    &instance(),
                    &manifest,
                    &priority,
                    registry,
                    Some(&previous),
                    |snapshot| {
                        published.send(snapshot.clone()).unwrap();
                    },
                )
                .await;
                let mut expired = complete.clone();
                expired.checked_at = 0;
                let offline = scan_with_registry(
                    &instance(),
                    &manifest,
                    &priority,
                    std::sync::Arc::new(ProviderRegistry::new(Vec::new())),
                    Some(&expired),
                    |_| {},
                )
                .await;
                assert_eq!(offline.updates.len(), complete.updates.len());
                assert_eq!(offline.failures.len(), 12);
                complete
            });
            let mut requested = HashSet::new();
            for _ in 0..8 {
                requested.insert(
                    tokio::time::timeout(std::time::Duration::from_secs(5), requests.recv())
                        .await
                        .unwrap()
                        .unwrap(),
                );
            }
            assert_eq!(requested, expected);
            assert!(requests.try_recv().is_err());
            assert_eq!(
                crate::feedback::progress::PROGRESS.lock().unwrap().progress,
                Some((0, 12))
            );
            pauses[&first.project_id].add_permits(1);
            let partial = tokio::time::timeout(std::time::Duration::from_secs(5), results.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(!scan.is_finished());
            assert!(partial.update_for(&first).is_some());
            assert!(partial.update_for(&known).is_some());
            assert_eq!(partial.checked_at, 0);
            assert_eq!(
                crate::feedback::progress::PROGRESS.lock().unwrap().progress,
                Some((1, 12))
            );
            let mut terminal =
                ratatui::Terminal::new(ratatui::backend::TestBackend::new(50, 5)).unwrap();
            terminal
                .draw(|frame| {
                    crate::tui::widgets::status::render(
                        frame,
                        frame.area(),
                        crate::tui::app::FocusedArea::Overview,
                        &mut throbber_widgets_tui::ThrobberState::default(),
                        None,
                    )
                })
                .unwrap();
            let overview = terminal.backend().to_string();
            assert!(overview.contains("8%"));
            assert!(overview.contains("1/12 item(s) checked"));
            assert!(overview.contains("Checking content updates"));
            assert!(!overview.contains(&instance().name));
            for pause in pauses.values() {
                pause.add_permits(1);
            }
            let complete = tokio::time::timeout(std::time::Duration::from_secs(5), scan)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(complete.updates.len(), 11);
            assert!(complete.failures.is_empty());
            assert!(complete.checked_at > 0);
            assert!(complete.update_for(&known).is_none());
        });
}

#[test]
fn update_checks_reuse_fresh_results_only_for_unchanged_installed_versions() {
    use crate::instance::content::updates::{UpdateCheckFailure, scan_with_registry};
    let _guard = crate::tests::TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let versions = ["alpha", "bravo", "charlie"]
                .into_iter()
                .flat_map(|project| {
                    [
                        ("old", "2026-01-01T00:00:00Z"),
                        ("new", "2026-02-01T00:00:00Z"),
                    ]
                    .into_iter()
                    .map(move |(suffix, date)| {
                        version(
                            &format!("{project}-{suffix}"),
                            project,
                            VersionType::Release,
                            date,
                            Vec::new(),
                        )
                    })
                })
                .collect();
            let (started, mut requests) = tokio::sync::mpsc::unbounded_channel();
            let registry = std::sync::Arc::new(controlled_update_registry(
                versions,
                started,
                HashMap::new(),
            ));
            let instance = instance();
            let mut manifest = ContentManifest::default();
            for project in ["alpha", "bravo"] {
                manifest.upsert(installed_record(project, &format!("{project}-old"), true));
            }
            let mut previous =
                scan_with_registry(&instance, &manifest, &[], registry.clone(), None, |_| {}).await;
            assert_eq!(std::iter::from_fn(|| requests.try_recv().ok()).count(), 2);
            previous.checked_at -= 60;
            let original_checked_at = previous.checked_at;

            manifest.upsert(installed_record("charlie", "charlie-old", true));
            let mut snapshot = scan_with_registry(
                &instance,
                &manifest,
                &[],
                registry.clone(),
                Some(&previous),
                |_| {},
            )
            .await;
            assert_eq!(requests.try_recv().unwrap(), "charlie");
            assert!(requests.try_recv().is_err());
            assert_eq!(snapshot.updates.len(), 3);
            assert_eq!(snapshot.checked_at, original_checked_at);

            manifest.remove(Path::new("mods/alpha.jar"));
            snapshot = scan_with_registry(
                &instance,
                &manifest,
                &[],
                registry.clone(),
                Some(&snapshot),
                |_| {},
            )
            .await;
            assert!(
                requests.try_recv().is_err(),
                "Removal must not rescan the remaining projects"
            );
            assert_eq!(snapshot.updates.len(), 2);
            assert!(snapshot.matches_manifest(&manifest));

            manifest.upsert(installed_record("bravo", "bravo-new", true));
            snapshot = scan_with_registry(
                &instance,
                &manifest,
                &[],
                registry.clone(),
                Some(&snapshot),
                |_| {},
            )
            .await;
            assert_eq!(requests.try_recv().unwrap(), "bravo");
            assert!(requests.try_recv().is_err());
            assert_eq!(snapshot.updates.len(), 1);
            assert_eq!(snapshot.updates[0].installed.project_id, "charlie");
            assert_eq!(
                snapshot.checked_at, original_checked_at,
                "Inventory changes must not prolong cached results"
            );

            snapshot.failures.push(UpdateCheckFailure {
                installed: manifest
                    .files
                    .last()
                    .unwrap()
                    .resolved_project()
                    .unwrap()
                    .clone(),
                kind: ContentKind::Mod,
                reason: "offline".to_owned(),
            });
            snapshot = scan_with_registry(
                &instance,
                &manifest,
                &[],
                registry.clone(),
                Some(&snapshot),
                |_| {},
            )
            .await;
            assert_eq!(requests.try_recv().unwrap(), "charlie");
            assert!(requests.try_recv().is_err());
            assert!(snapshot.failures.is_empty());

            snapshot.checked_at = 0;
            snapshot = scan_with_registry(
                &instance,
                &manifest,
                &[],
                registry.clone(),
                Some(&snapshot),
                |_| {},
            )
            .await;
            assert_eq!(
                std::iter::from_fn(|| requests.try_recv().ok()).count(),
                2,
                "Expired results must be refreshed"
            );
            let mut changed_instance = instance.clone();
            changed_instance.game_version = "1.21.2".to_owned();
            scan_with_registry(
                &changed_instance,
                &manifest,
                &[],
                registry.clone(),
                Some(&snapshot),
                |_| {},
            )
            .await;
            assert_eq!(
                std::iter::from_fn(|| requests.try_recv().ok()).count(),
                2,
                "Changing compatibility invalidates cached results"
            );
            scan_with_registry(&instance, &manifest, &[], registry, None, |_| {}).await;
            assert_eq!(
                std::iter::from_fn(|| requests.try_recv().ok()).count(),
                2,
                "An explicit scan must still check everything"
            );
        });
}

#[tokio::test]
async fn datapack_dependencies_are_routed_to_the_world_or_instance_by_kind() {
    let temp = tempfile::tempdir().unwrap();
    let minecraft = temp.path().join("minecraft");
    let world = minecraft.join("saves/world");
    let manifest_path = temp.path().join("manifest.json");
    let mut root_version = version(
        "root",
        "root",
        VersionType::Release,
        "2026-01-01",
        vec![dependency("library", DependencyType::Required)],
    );
    root_version.loaders = vec!["datapack".to_owned()];
    let library = version(
        "library",
        "library",
        VersionType::Release,
        "2026-01-01",
        Vec::new(),
    );
    let registry = provider(vec![root_version.clone(), library]).registry();
    let mut install_root = root(root_version);
    install_root.kind = ContentKind::DataPack;
    install_root.target_world = Some(world.clone());

    let plan = resolve(
        &registry,
        &ContentManifest::default(),
        &minecraft,
        &instance(),
        install_root,
    )
    .await
    .unwrap();

    assert_eq!(plan.items[0].kind, ContentKind::DataPack);
    assert_eq!(plan.items[0].destination, world.join("datapacks"));
    assert_eq!(plan.items[1].kind, ContentKind::Mod);
    assert_eq!(plan.items[1].destination, minecraft.join("mods"));

    install(&registry, &manifest_path, &minecraft, &plan)
        .await
        .unwrap();
    let manifest = ContentManifest::load(&manifest_path).unwrap();
    assert_eq!(
        manifest
            .record(Path::new("saves/world/datapacks/root.zip"))
            .unwrap()
            .kind,
        ContentKind::DataPack
    );
    assert_eq!(
        manifest.record(Path::new("mods/library.jar")).unwrap().kind,
        ContentKind::Mod
    );
}

#[tokio::test]
async fn datapack_install_rejects_an_incompatible_instance_mod() {
    let mut root_version = version(
        "root",
        "root",
        VersionType::Release,
        "2026-01-01",
        vec![dependency("conflict", DependencyType::Incompatible)],
    );
    root_version.loaders = vec!["datapack".to_owned()];
    let registry = provider(vec![root_version.clone()]).registry();
    let mut install_root = root(root_version);
    install_root.kind = ContentKind::DataPack;
    install_root.target_world = Some(PathBuf::from("/minecraft/saves/world"));
    let manifest = ContentManifest {
        version: 1,
        files: vec![installed_record("conflict", "conflict-version", true)],
    };

    let error = resolve(
        &registry,
        &manifest,
        Path::new("/minecraft"),
        &instance(),
        install_root,
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("incompatible"));
}

#[test]
fn legacy_modrinth_datapack_projects_are_classified_by_loader() {
    let project = ProjectInfo {
        id: "legacy".to_owned(),
        slug: "legacy".to_owned(),
        title: "Legacy datapack".to_owned(),
        description: String::new(),
        body: String::new(),
        icon_url: None,
        categories: Vec::new(),
        additional_categories: Vec::new(),
        project_type: "mod".to_owned(),
        loaders: vec!["datapack".to_owned()],
        ..ProjectInfo::default()
    };

    assert_eq!(
        dependency_kind(Some(&project), None, ContentKind::DataPack),
        ContentKind::DataPack
    );
}

#[tokio::test]
async fn required_dependencies_prefer_the_newest_stable_release() {
    let root_version = version(
        "root",
        "root",
        VersionType::Release,
        "2026-01-01",
        vec![
            dependency("library", DependencyType::Required),
            dependency("optional", DependencyType::Optional),
        ],
    );
    let beta = version(
        "library-beta",
        "library",
        VersionType::Beta,
        "2026-03-01",
        Vec::new(),
    );
    let stable_old = version(
        "library-1",
        "library",
        VersionType::Release,
        "2026-01-01",
        Vec::new(),
    );
    let stable_new = version(
        "library-2",
        "library",
        VersionType::Release,
        "2026-02-01",
        Vec::new(),
    );
    let registry = provider(vec![root_version.clone(), beta, stable_old, stable_new]).registry();

    let plan = resolve(
        &registry,
        &ContentManifest::default(),
        Path::new("/minecraft"),
        &instance(),
        root(root_version),
    )
    .await
    .unwrap();

    assert_eq!(plan.items.len(), 2);
    assert_eq!(plan.items[1].version.id, "library-2");
    assert!(plan.items[1].automatic_dependency);
    assert_eq!(plan.optional_dependencies, 1);
}

#[tokio::test]
async fn functional_mod_dependencies_are_not_cleanup_eligible() {
    let root_version = version(
        "root",
        "root",
        VersionType::Release,
        "2026-01-01",
        vec![dependency("sodium", DependencyType::Required)],
    );
    let sodium = version(
        "sodium",
        "sodium",
        VersionType::Release,
        "2026-01-01",
        Vec::new(),
    );
    let mut fake = provider(vec![root_version.clone(), sodium]);
    fake.categories.insert(
        "sodium".to_owned(),
        vec!["library".to_owned(), "optimization".to_owned()],
    );
    let registry = fake.registry();

    let plan = resolve(
        &registry,
        &ContentManifest::default(),
        Path::new("/minecraft"),
        &instance(),
        root(root_version),
    )
    .await
    .unwrap();

    assert!(plan.items[1].automatic_dependency);
    assert!(!plan.items[1].cleanup_eligible);
}

#[tokio::test]
async fn missing_project_metadata_does_not_block_required_dependencies() {
    let root_version = version(
        "root",
        "root",
        VersionType::Release,
        "2026-01-01",
        vec![dependency("library", DependencyType::Required)],
    );
    let library = version(
        "library",
        "library",
        VersionType::Release,
        "2026-01-01",
        Vec::new(),
    );
    let mut fake = provider(vec![root_version.clone(), library]);
    fake.fail_project = Some("library".to_owned());
    let registry = fake.registry();

    let plan = resolve(
        &registry,
        &ContentManifest::default(),
        Path::new("/minecraft"),
        &instance(),
        root(root_version),
    )
    .await
    .unwrap();

    assert_eq!(plan.items[1].title, "library");
    assert!(plan.items[1].automatic_dependency);
    assert!(!plan.items[1].cleanup_eligible);
}

#[tokio::test]
async fn incompatible_installed_dependency_is_replaced_with_a_compatible_version() {
    let root_version = version(
        "root",
        "root",
        VersionType::Release,
        "2026-01-01",
        vec![dependency("library", DependencyType::Required)],
    );
    let mut incompatible = version(
        "library-old",
        "library",
        VersionType::Release,
        "2026-01-01",
        Vec::new(),
    );
    incompatible.game_versions = vec!["1.20.1".to_owned()];
    let compatible = version(
        "library-new",
        "library",
        VersionType::Release,
        "2026-02-01",
        Vec::new(),
    );
    let registry = provider(vec![root_version.clone(), incompatible, compatible]).registry();
    let manifest = ContentManifest {
        version: 1,
        files: vec![installed_record("library", "library-old", true)],
    };

    let plan = resolve(
        &registry,
        &manifest,
        Path::new("/minecraft"),
        &instance(),
        root(root_version),
    )
    .await
    .unwrap();

    assert_eq!(plan.items[1].version.id, "library-new");
    assert!(plan.items[1].replacement);
}

#[tokio::test]
async fn missing_installed_dependency_version_is_replaced() {
    let root_version = version(
        "root",
        "root",
        VersionType::Release,
        "2026-01-01",
        vec![dependency("library", DependencyType::Required)],
    );
    let compatible = version(
        "library-new",
        "library",
        VersionType::Release,
        "2026-02-01",
        Vec::new(),
    );
    let registry = provider(vec![root_version.clone(), compatible]).registry();
    let manifest = ContentManifest {
        version: 1,
        files: vec![installed_record("library", "deleted-version", true)],
    };

    let plan = resolve(
        &registry,
        &manifest,
        Path::new("/minecraft"),
        &instance(),
        root(root_version),
    )
    .await
    .unwrap();

    assert_eq!(plan.items[1].version.id, "library-new");
    assert!(plan.items[1].replacement);
}

#[tokio::test]
async fn disabled_dependency_is_not_treated_as_installed() {
    let root_version = version(
        "root",
        "root",
        VersionType::Release,
        "2026-01-01",
        vec![dependency("library", DependencyType::Required)],
    );
    let library = version(
        "library",
        "library",
        VersionType::Release,
        "2026-01-01",
        Vec::new(),
    );
    let registry = provider(vec![root_version.clone(), library]).registry();
    let manifest = ContentManifest {
        version: 1,
        files: vec![installed_record("library", "library", false)],
    };

    let plan = resolve(
        &registry,
        &manifest,
        Path::new("/minecraft"),
        &instance(),
        root(root_version),
    )
    .await
    .unwrap();

    assert!(plan.items[1].installed_path.is_none());
    assert!(!plan.items[1].replacement);
}

#[tokio::test]
async fn superseded_version_dependencies_are_not_kept_in_the_plan() {
    let root_version = version(
        "root",
        "root",
        VersionType::Release,
        "2026-01-01",
        vec![
            dependency("library", DependencyType::Required),
            VersionDependency {
                version_id: Some("library-old".to_owned()),
                project_id: Some("library".to_owned()),
                file_name: None,
                dependency_type: DependencyType::Required,
            },
        ],
    );
    let library_new = version(
        "library-new",
        "library",
        VersionType::Release,
        "2026-03-01",
        vec![dependency("stale", DependencyType::Required)],
    );
    let library_old = version(
        "library-old",
        "library",
        VersionType::Release,
        "2026-02-01",
        vec![dependency("current", DependencyType::Required)],
    );
    let stale = version(
        "stale",
        "stale",
        VersionType::Release,
        "2026-01-01",
        Vec::new(),
    );
    let current = version(
        "current",
        "current",
        VersionType::Release,
        "2026-01-01",
        Vec::new(),
    );
    let registry = provider(vec![
        root_version.clone(),
        library_new,
        library_old,
        stale,
        current,
    ])
    .registry();

    let plan = resolve(
        &registry,
        &ContentManifest::default(),
        Path::new("/minecraft"),
        &instance(),
        root(root_version),
    )
    .await
    .unwrap();

    assert_eq!(plan.items[1].version.id, "library-old");
    assert!(plan.items.iter().any(|item| item.project_id == "current"));
    assert!(!plan.items.iter().any(|item| item.project_id == "stale"));
}

#[tokio::test]
async fn dependency_version_from_another_project_is_rejected() {
    let root_version = version(
        "root",
        "root",
        VersionType::Release,
        "2026-01-01",
        vec![VersionDependency {
            version_id: Some("wrong-version".to_owned()),
            project_id: Some("expected".to_owned()),
            file_name: None,
            dependency_type: DependencyType::Required,
        }],
    );
    let wrong = version(
        "wrong-version",
        "other",
        VersionType::Release,
        "2026-01-01",
        Vec::new(),
    );
    let registry = provider(vec![root_version.clone(), wrong]).registry();

    let error = resolve(
        &registry,
        &ContentManifest::default(),
        Path::new("/minecraft"),
        &instance(),
        root(root_version),
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("not 'expected'"));
}

#[tokio::test]
async fn exact_cross_provider_match_replaces_only_the_wrong_version() {
    let root_version = version(
        "root",
        "root",
        VersionType::Release,
        "2026-01-01",
        vec![VersionDependency {
            version_id: Some("library-new".to_owned()),
            project_id: Some("library".to_owned()),
            file_name: None,
            dependency_type: DependencyType::Required,
        }],
    );
    let library = version(
        "library-new",
        "library",
        VersionType::Release,
        "2026-02-01",
        Vec::new(),
    );
    let mut fake = provider(vec![root_version.clone(), library]);
    fake.resolved.insert(
        "mods/library.jar".to_owned(),
        ProviderProject {
            provider: "modrinth".to_owned(),
            project_id: "library".to_owned(),
            version_id: "library-old".to_owned(),
        },
    );
    let registry = fake.registry();
    let manifest = ContentManifest {
        version: 1,
        files: vec![ContentFileRecord {
            relative_path: PathBuf::from("mods/library.jar"),
            kind: ContentKind::Mod,
            enabled: true,
            fingerprint: FileFingerprint {
                size: 1,
                modified_ns: 1,
                hashes: Default::default(),
            },
            resolution: Resolution::Resolved {
                project: ProviderProject {
                    provider: "curseforge".to_owned(),
                    project_id: "cf-library".to_owned(),
                    version_id: "7".to_owned(),
                },
            },
            provider_aliases: Vec::new(),
            provider_checks: Vec::new(),
            required_dependencies: Vec::new(),
            automatic_dependency: false,
            cleanup_eligible: false,
        }],
    };

    let plan = resolve(
        &registry,
        &manifest,
        Path::new("/minecraft"),
        &instance(),
        root(root_version),
    )
    .await
    .unwrap();

    assert!(plan.items[1].replacement);
    assert_eq!(
        plan.items[1].installed_path.as_deref(),
        Some(Path::new("/minecraft/mods/library.jar"))
    );
}

#[tokio::test]
async fn dependency_cycles_are_rejected_before_downloads() {
    let root_version = version(
        "root",
        "root",
        VersionType::Release,
        "2026-01-01",
        vec![dependency("library", DependencyType::Required)],
    );
    let library = version(
        "library",
        "library",
        VersionType::Release,
        "2026-01-01",
        vec![dependency("root", DependencyType::Required)],
    );
    let registry = provider(vec![root_version.clone(), library]).registry();

    let error = resolve(
        &registry,
        &ContentManifest::default(),
        Path::new("/minecraft"),
        &instance(),
        root(root_version),
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("cycle"));
}

#[tokio::test]
async fn selected_version_is_refetched_before_resolving_dependencies() {
    let fresh = version(
        "root",
        "root",
        VersionType::Release,
        "2026-01-01",
        vec![dependency("library", DependencyType::Required)],
    );
    let mut cached = fresh.clone();
    cached.dependencies.clear();
    let library = version(
        "library",
        "library",
        VersionType::Release,
        "2026-01-01",
        Vec::new(),
    );
    let registry = provider(vec![fresh, library]).registry();

    let plan = resolve(
        &registry,
        &ContentManifest::default(),
        Path::new("/minecraft"),
        &instance(),
        root(cached),
    )
    .await
    .unwrap();

    assert_eq!(plan.items.len(), 2);
}

#[tokio::test]
async fn installed_incompatible_projects_block_the_plan() {
    let root_version = version(
        "root",
        "root",
        VersionType::Release,
        "2026-01-01",
        vec![dependency("conflict", DependencyType::Incompatible)],
    );
    let conflict = version(
        "conflict-version",
        "conflict",
        VersionType::Release,
        "2026-01-01",
        Vec::new(),
    );
    let registry = provider(vec![root_version.clone(), conflict]).registry();
    let manifest = ContentManifest {
        version: 1,
        files: vec![ContentFileRecord {
            relative_path: PathBuf::from("mods/conflict.jar"),
            kind: ContentKind::Mod,
            enabled: true,
            fingerprint: FileFingerprint {
                size: 1,
                modified_ns: 1,
                hashes: Default::default(),
            },
            resolution: Resolution::Resolved {
                project: ProviderProject {
                    provider: "modrinth".to_owned(),
                    project_id: "conflict".to_owned(),
                    version_id: "conflict-version".to_owned(),
                },
            },
            provider_aliases: Vec::new(),
            provider_checks: Vec::new(),
            required_dependencies: Vec::new(),
            automatic_dependency: false,
            cleanup_eligible: false,
        }],
    };

    let error = resolve(
        &registry,
        &manifest,
        Path::new("/minecraft"),
        &instance(),
        root(root_version),
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("incompatible"));
}

#[tokio::test]
async fn dependency_install_commits_files_and_manifest_together() {
    let temp = tempfile::tempdir().unwrap();
    let paths = crate::storage::InstancePaths::new(temp.path().join("Instance"));
    let minecraft = paths.minecraft();
    let mods = minecraft.join("mods");
    let manifest_path = paths.content_manifest();
    let root_version = version(
        "root",
        "root",
        VersionType::Release,
        "2026-01-01",
        vec![dependency("library", DependencyType::Required)],
    );
    let library = version(
        "library",
        "library",
        VersionType::Release,
        "2026-01-01",
        Vec::new(),
    );
    let registry = provider(vec![root_version.clone(), library]).registry();
    let plan = resolve(
        &registry,
        &ContentManifest::default(),
        &minecraft,
        &instance(),
        root(root_version),
    )
    .await
    .unwrap();

    let mut running = crate::instance::runtime::lock_instance(temp.path(), "Instance").unwrap();
    assert!(
        install(&registry, &manifest_path, &minecraft, &plan)
            .await
            .is_err()
    );
    running.allow_content_changes().unwrap();
    let installed = install(&registry, &manifest_path, &minecraft, &plan)
        .await
        .unwrap();

    assert_eq!(installed.root_path, mods.join("root.jar"));
    assert!(crate::instance::runtime::lock_instance(temp.path(), "Instance").is_err());
    assert!(mods.join("library.jar").exists());
    let manifest = ContentManifest::load(&manifest_path).unwrap();
    assert_eq!(manifest.files.len(), 2);
    assert!(
        manifest
            .record(Path::new("mods/library.jar"))
            .unwrap()
            .automatic_dependency
    );
    assert!(
        manifest
            .record(Path::new("mods/library.jar"))
            .unwrap()
            .cleanup_eligible
    );
    assert_eq!(
        manifest
            .record(Path::new("mods/root.jar"))
            .unwrap()
            .required_dependencies[0]
            .project_id,
        "library"
    );
}

#[tokio::test]
async fn failed_dependency_download_leaves_no_partial_install() {
    let temp = tempfile::tempdir().unwrap();
    let minecraft = temp.path().join("minecraft");
    let mods = minecraft.join("mods");
    let manifest_path = temp.path().join("manifest.json");
    let root_version = version(
        "root",
        "root",
        VersionType::Release,
        "2026-01-01",
        vec![dependency("library", DependencyType::Required)],
    );
    let library = version(
        "library",
        "library",
        VersionType::Release,
        "2026-01-01",
        Vec::new(),
    );
    let mut fake = provider(vec![root_version.clone(), library]);
    fake.fail_download = Some("library".to_owned());
    let registry = fake.registry();
    let plan = resolve(
        &registry,
        &ContentManifest::default(),
        &minecraft,
        &instance(),
        root(root_version),
    )
    .await
    .unwrap();

    assert!(
        install(&registry, &manifest_path, &minecraft, &plan)
            .await
            .is_err()
    );
    assert!(!manifest_path.exists());
    match std::fs::read_dir(&mods) {
        Ok(entries) => assert_eq!(entries.count(), 0),
        Err(error) => assert_eq!(error.kind(), std::io::ErrorKind::NotFound),
    }
}

fn required(project: &str, pin: Option<&str>) -> VersionDependency {
    VersionDependency {
        version_id: pin.map(str::to_owned),
        ..dependency(project, DependencyType::Required)
    }
}

fn release(id: &str, project: &str, dependencies: Vec<VersionDependency>) -> VersionInfo {
    version(
        id,
        project,
        VersionType::Release,
        if id.ends_with('2') {
            "2026-02-01"
        } else {
            "2026-01-01"
        },
        dependencies,
    )
}

async fn resolve_graph(versions: Vec<VersionInfo>) -> Result<DependencyPlan, NetError> {
    let selected = versions[0].clone();
    resolve(
        &provider(versions).registry(),
        &ContentManifest::default(),
        Path::new("/minecraft"),
        &instance(),
        root(selected),
    )
    .await
}

#[tokio::test]
async fn matching_exact_requirement_remains_binding() {
    let error = resolve_graph(vec![
        release(
            "r1",
            "root",
            vec![
                required("c", None),
                required("d", None),
                required("e", None),
            ],
        ),
        release("d1", "d", vec![required("c", Some("c2"))]),
        release("e1", "e", vec![required("c", Some("c1"))]),
        release("c1", "c", Vec::new()),
        release("c2", "c", Vec::new()),
    ])
    .await
    .unwrap_err();
    assert!(error.to_string().contains("Conflicting required versions"));
}

#[tokio::test]
async fn replacing_a_parent_retracts_its_exact_requirements() {
    let plan = resolve_graph(vec![
        release("r1", "root", vec![required("d", None), required("e", None)]),
        release("d1", "d", vec![required("b", None)]),
        release("e1", "e", vec![required("f", None)]),
        release("f1", "f", vec![required("b", Some("b1"))]),
        release("b2", "b", vec![required("c", Some("c1"))]),
        release("b1", "b", vec![required("c", Some("c2"))]),
        release("c1", "c", Vec::new()),
        release("c2", "c", Vec::new()),
    ])
    .await
    .unwrap();
    assert_eq!(
        plan.items
            .iter()
            .find(|item| item.project_id == "b")
            .unwrap()
            .version
            .id,
        "b1"
    );
    let c = plan
        .items
        .iter()
        .find(|item| item.project_id == "c")
        .unwrap();
    assert_eq!(c.version.id, "c2");
    assert!(
        plan.items
            .iter()
            .find(|item| item.project_id == "b")
            .unwrap()
            .required_dependencies
            .contains(&c.identity())
    );
}

#[tokio::test]
async fn settling_retries_a_pin_after_its_competitor_disappears() {
    let plan = resolve_graph(vec![
        release("r1", "root", vec![required("d", None), required("e", None)]),
        release("d1", "d", vec![required("b", None)]),
        release("e1", "e", vec![required("f", None)]),
        release(
            "f1",
            "f",
            vec![required("c", Some("c2")), required("g", None)],
        ),
        release("g1", "g", vec![required("b", Some("b1"))]),
        release("b2", "b", vec![required("c", Some("c1"))]),
        release("b1", "b", Vec::new()),
        release("c1", "c", Vec::new()),
        release("c2", "c", Vec::new()),
    ])
    .await
    .unwrap();
    assert_eq!(
        plan.items
            .iter()
            .find(|item| item.project_id == "c")
            .unwrap()
            .version
            .id,
        "c2"
    );
}

#[tokio::test]
async fn unreachable_parents_do_not_keep_exact_requirements() {
    let plan = resolve_graph(vec![
        release("r1", "root", vec![required("d", None), required("e", None)]),
        release("d1", "d", vec![required("b", None)]),
        release("e1", "e", vec![required("f", None)]),
        release("f1", "f", vec![required("g", None)]),
        release(
            "g1",
            "g",
            vec![required("c", Some("c2")), required("h", None)],
        ),
        release("h1", "h", vec![required("b", Some("b1"))]),
        release("b2", "b", vec![required("orphan", None)]),
        release("orphan1", "orphan", vec![required("c", Some("c1"))]),
        release("b1", "b", Vec::new()),
        release("c1", "c", Vec::new()),
        release("c2", "c", Vec::new()),
    ])
    .await
    .unwrap();
    assert!(!plan.items.iter().any(|item| item.project_id == "orphan"));
    assert_eq!(
        plan.items
            .iter()
            .find(|item| item.project_id == "c")
            .unwrap()
            .version
            .id,
        "c2"
    );
}

#[tokio::test]
async fn superseded_parent_errors_and_cycles_do_not_reject_the_final_graph() {
    for old_dependencies in [vec![required("missing", None)], vec![required("c", None)]] {
        let plan = resolve_graph(vec![
            release("r1", "root", vec![required("d", None), required("e", None)]),
            release("d1", "d", vec![required("b", None)]),
            release("e1", "e", vec![required("f", None)]),
            release("f1", "f", vec![required("b", Some("b1"))]),
            release("b2", "b", old_dependencies),
            release("b1", "b", Vec::new()),
            release("c1", "c", vec![required("b", None)]),
        ])
        .await
        .unwrap();
        assert_eq!(
            plan.items
                .iter()
                .find(|item| item.project_id == "b")
                .unwrap()
                .version
                .id,
            "b1"
        );
        assert!(!plan.items.iter().any(|item| item.project_id == "c"));
    }
}

#[tokio::test]
async fn cycles_through_reused_nodes_are_rejected_but_diamonds_are_valid() {
    for cycle in [false, true] {
        let result = resolve_graph(vec![
            release("r1", "root", vec![required("b", None), required("c", None)]),
            release("b1", "b", vec![required("d", None)]),
            release("c1", "c", vec![required("d", None)]),
            release(
                "d1",
                "d",
                if cycle {
                    vec![required("c", None)]
                } else {
                    Vec::new()
                },
            ),
        ])
        .await;
        if cycle {
            assert!(result.unwrap_err().to_string().contains("cycle"));
        } else {
            assert_eq!(
                result
                    .unwrap()
                    .items
                    .iter()
                    .filter(|item| item.project_id == "d")
                    .count(),
                1
            );
        }
    }
}

#[tokio::test]
async fn changing_version_cycles_terminate_with_a_diagnostic() {
    let error = resolve_graph(vec![
        release("r1", "root", vec![required("a", None)]),
        release("a2", "a", vec![required("b", Some("b2"))]),
        release("b2", "b", vec![required("a", Some("a1"))]),
        release("a1", "a", vec![required("b", Some("b1"))]),
        release("b1", "b", vec![required("a", Some("a2"))]),
    ])
    .await
    .unwrap_err();
    assert!(error.to_string().contains("did not converge"));
}

#[tokio::test]
async fn replaced_incompatible_versions_are_absent_from_the_final_inventory() {
    let temp = tempfile::tempdir().unwrap();
    let minecraft = temp.path().join("minecraft");
    let manifest_path = temp.path().join("manifest.json");
    let mut record = installed_record("b", "b1", true);
    let old = minecraft.join(&record.relative_path);
    std::fs::create_dir_all(old.parent().unwrap()).unwrap();
    std::fs::write(&old, b"b1").unwrap();
    record.fingerprint = super::super::manifest::fingerprint(&old).unwrap();
    let manifest = ContentManifest {
        version: 1,
        files: vec![record],
    };
    manifest.save(&manifest_path).unwrap();
    let mut a = release("a2", "a", vec![required("b", Some("b2"))]);
    a.dependencies.push(VersionDependency {
        version_id: Some("b1".to_owned()),
        ..dependency("b", DependencyType::Incompatible)
    });
    let registry = provider(vec![
        a.clone(),
        release("b1", "b", Vec::new()),
        release("b2", "b", Vec::new()),
    ])
    .registry();
    let plan = resolve(&registry, &manifest, &minecraft, &instance(), root(a))
        .await
        .unwrap();
    assert!(
        plan.items
            .iter()
            .find(|item| item.project_id == "b")
            .unwrap()
            .replacement
    );
    install(&registry, &manifest_path, &minecraft, &plan)
        .await
        .unwrap();
    assert!(!old.exists());
    assert_eq!(std::fs::read(minecraft.join("mods/b2.jar")).unwrap(), b"b2");
    let final_manifest = ContentManifest::load(&manifest_path).unwrap();
    assert!(
        final_manifest
            .files
            .iter()
            .all(|record| record.resolved_project().unwrap().version_id != "b1")
    );
}

#[tokio::test]
async fn an_unreplaced_duplicate_incompatible_file_still_blocks_the_plan() {
    let mut a = release("a2", "a", vec![required("b", Some("b2"))]);
    a.dependencies.push(VersionDependency {
        version_id: Some("b1".to_owned()),
        ..dependency("b", DependencyType::Incompatible)
    });
    let registry = provider(vec![
        a.clone(),
        release("b1", "b", Vec::new()),
        release("b2", "b", Vec::new()),
    ])
    .registry();
    let first = installed_record("b", "b1", true);
    let mut duplicate = first.clone();
    duplicate.relative_path = "mods/z-b.jar".into();
    let manifest = ContentManifest {
        version: 1,
        files: vec![first, duplicate],
    };
    let error = resolve(
        &registry,
        &manifest,
        Path::new("/minecraft"),
        &instance(),
        root(a),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("incompatible"));
}

fn update_request(
    minecraft: &Path,
    current: &VersionInfo,
    target: &VersionInfo,
) -> super::super::updates::UpdateRequest {
    super::super::updates::UpdateRequest {
        title: target.project_id.clone(),
        installed_path: minecraft.join(format!("mods/{}.jar", current.project_id)),
        target_world: None,
        update: super::super::updates::AvailableUpdate {
            installed: ProviderProject {
                provider: "modrinth".to_owned(),
                project_id: current.project_id.clone(),
                version_id: current.id.clone(),
            },
            current: Some(current.clone()),
            target: target.clone(),
            kind: ContentKind::Mod,
        },
    }
}

#[tokio::test]
async fn rejected_bulk_roots_are_removed_from_survivor_projections() {
    let minecraft = Path::new("/minecraft");
    let a1 = release("a1", "a", Vec::new());
    let mut a2 = release("a2", "a", Vec::new());
    a2.dependencies.push(VersionDependency {
        version_id: Some("b1".to_owned()),
        ..dependency("b", DependencyType::Incompatible)
    });
    let b1 = release("b1", "b", Vec::new());
    let b2 = release("b2", "b", vec![required("missing", None)]);
    let c1 = release("c1", "c", Vec::new());
    let c2 = release("c2", "c", Vec::new());
    let requests = vec![
        update_request(minecraft, &a1, &a2),
        update_request(minecraft, &b1, &b2),
        update_request(minecraft, &c1, &c2),
    ];
    let registry = provider(vec![a1, a2, b1, b2, c1, c2]).registry();
    let manifest = ContentManifest {
        version: 1,
        files: vec![
            installed_record("a", "a1", true),
            installed_record("b", "b1", true),
            installed_record("c", "c1", true),
        ],
    };
    let plan = super::super::updates::plan_bulk_with_registry(
        &registry,
        &instance(),
        &manifest,
        minecraft,
        requests,
        Vec::new(),
        None,
    )
    .await;
    assert_eq!(plan.roots.len(), 1);
    assert_eq!(plan.roots[0].target.id, "c2");
    assert_eq!(plan.dependency_plan.items.len(), 1);
    assert_eq!(plan.conflicts.len(), 2);
    assert!(
        plan.conflicts
            .iter()
            .any(|conflict| conflict.title == "a" && conflict.reason.contains("incompatible"))
    );
    assert!(
        plan.conflicts
            .iter()
            .any(|conflict| conflict.title == "b" && conflict.reason.contains("missing"))
    );
}

#[tokio::test]
async fn accepted_bulk_dependencies_keep_actual_old_ownership_and_replace_files() {
    let temp = tempfile::tempdir().unwrap();
    let minecraft = temp.path().join("minecraft");
    let manifest_path = temp.path().join("manifest.json");
    std::fs::create_dir_all(minecraft.join("mods")).unwrap();
    let a1 = release("a1", "a", Vec::new());
    let a2 = release("a2", "a", vec![required("b", None)]);
    let b1 = release("b1", "b", Vec::new());
    let b2 = release("b2", "b", Vec::new());
    let requests = vec![
        update_request(&minecraft, &a1, &a2),
        update_request(&minecraft, &b1, &b2),
    ];
    let registry = provider(vec![a1, a2, b1, b2]).registry();
    let mut manifest = ContentManifest::default();
    for (project, version) in [("a", "a1"), ("b", "b1")] {
        let mut record = installed_record(project, version, true);
        let path = minecraft.join(&record.relative_path);
        std::fs::write(&path, version.as_bytes()).unwrap();
        record.fingerprint = super::super::manifest::fingerprint(&path).unwrap();
        record.provider_aliases.push(ProviderProject {
            provider: "curseforge".to_owned(),
            project_id: project.to_owned(),
            version_id: format!("cf-{version}"),
        });
        manifest.upsert(record);
    }
    manifest.save(&manifest_path).unwrap();
    let plan = super::super::updates::plan_bulk_with_registry(
        &registry,
        &instance(),
        &manifest,
        &minecraft,
        requests,
        Vec::new(),
        None,
    )
    .await;
    assert!(plan.conflicts.is_empty());
    assert_eq!(plan.dependency_plan.root_count, 2);
    let b = plan
        .dependency_plan
        .items
        .iter()
        .find(|item| item.project_id == "b")
        .unwrap();
    assert!(b.replacement);
    assert_eq!(
        b.expected_record
            .as_ref()
            .unwrap()
            .resolved_project()
            .unwrap()
            .version_id,
        "b1"
    );
    assert!(b.provider_aliases.is_empty());
    install(&registry, &manifest_path, &minecraft, &plan.dependency_plan)
        .await
        .unwrap();
    assert_eq!(std::fs::read(minecraft.join("mods/a2.jar")).unwrap(), b"a2");
    assert_eq!(std::fs::read(minecraft.join("mods/b2.jar")).unwrap(), b"b2");
    assert!(!minecraft.join("mods/a.jar").exists());
    assert!(!minecraft.join("mods/b.jar").exists());
    assert!(
        ContentManifest::load(&manifest_path)
            .unwrap()
            .files
            .iter()
            .all(|record| record.provider_aliases.is_empty())
    );
}

#[tokio::test]
async fn merged_plans_recheck_incompatibilities_between_selected_roots() {
    let mut a = release("a2", "a", Vec::new());
    a.dependencies.push(VersionDependency {
        version_id: Some("b2".to_owned()),
        ..dependency("b", DependencyType::Incompatible)
    });
    let b = release("b2", "b", Vec::new());
    let registry = provider(vec![a.clone(), b.clone()]).registry();
    let mut plans = Vec::new();
    for selected in [a, b] {
        plans.push(
            resolve(
                &registry,
                &ContentManifest::default(),
                Path::new("/minecraft"),
                &instance(),
                root(selected),
            )
            .await
            .unwrap(),
        );
    }
    assert!(
        merge(plans)
            .unwrap_err()
            .to_string()
            .contains("incompatible")
    );
}

#[tokio::test]
async fn concurrent_installs_revalidate_target_names_and_project_ownership() {
    for same_project in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let minecraft = temp.path().join("minecraft");
        let manifest_path = temp.path().join("manifest.json");
        let first = release("a1", "a", Vec::new());
        let second = release("b2", if same_project { "a" } else { "b" }, Vec::new());
        let mut fake = provider(vec![first.clone(), second.clone()]);
        fake.download_barrier = Some(std::sync::Arc::new(tokio::sync::Barrier::new(2)));
        if !same_project {
            fake.download_filename = Some("shared.jar".to_owned());
        }
        let registry = fake.registry();
        let first = resolve(
            &registry,
            &ContentManifest::default(),
            &minecraft,
            &instance(),
            root(first),
        )
        .await
        .unwrap();
        let second = resolve(
            &registry,
            &ContentManifest::default(),
            &minecraft,
            &instance(),
            root(second),
        )
        .await
        .unwrap();
        let (left, right) = tokio::join!(
            install(&registry, &manifest_path, &minecraft, &first),
            install(&registry, &manifest_path, &minecraft, &second)
        );
        assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
        let manifest = ContentManifest::load(&manifest_path).unwrap();
        assert_eq!(manifest.files.len(), 1);
        let installed = &manifest.files[0];
        assert_eq!(
            std::fs::read(minecraft.join(&installed.relative_path)).unwrap(),
            installed.resolved_project().unwrap().version_id.as_bytes()
        );
        assert_eq!(
            std::fs::read_dir(minecraft.join("mods")).unwrap().count(),
            1
        );
    }
}

#[tokio::test]
async fn install_rejects_changed_old_bytes_and_new_incompatible_inventory() {
    let temp = tempfile::tempdir().unwrap();
    let minecraft = temp.path().join("minecraft");
    let manifest_path = temp.path().join("manifest.json");
    std::fs::create_dir_all(minecraft.join("mods")).unwrap();
    let a1 = release("a1", "a", Vec::new());
    let a2 = release("a2", "a", Vec::new());
    let registry = provider(vec![a1, a2.clone()]).registry();
    let mut record = installed_record("a", "a1", true);
    let old = minecraft.join(&record.relative_path);
    std::fs::write(&old, b"original").unwrap();
    record.fingerprint = super::super::manifest::fingerprint(&old).unwrap();
    let manifest = ContentManifest {
        version: 1,
        files: vec![record.clone()],
    };
    manifest.save(&manifest_path).unwrap();
    let mut selected = root(a2);
    selected.installed_path = Some(old.clone());
    let plan = resolve(&registry, &manifest, &minecraft, &instance(), selected)
        .await
        .unwrap();
    let modified = std::fs::metadata(&old).unwrap().modified().unwrap();
    std::fs::write(&old, b"external").unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&old)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    assert!(
        install(&registry, &manifest_path, &minecraft, &plan)
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("changed")
    );
    assert_eq!(std::fs::read(&old).unwrap(), b"external");
    assert!(!minecraft.join("mods/a2.jar").exists());

    let mut b = release("b2", "b", Vec::new());
    b.dependencies
        .push(dependency("a", DependencyType::Incompatible));
    let registry = provider(vec![b.clone()]).registry();
    let plan = resolve(
        &registry,
        &ContentManifest::default(),
        &minecraft,
        &instance(),
        root(b),
    )
    .await
    .unwrap();
    assert!(
        install(&registry, &manifest_path, &minecraft, &plan)
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("incompatible")
    );
    assert_eq!(std::fs::read(old).unwrap(), b"external");
}

#[tokio::test]
async fn cancelling_a_download_removes_staging_without_changing_live_content() {
    let temp = tempfile::tempdir().unwrap();
    let minecraft = temp.path().join("minecraft");
    let manifest_path = temp.path().join("manifest.json");
    std::fs::create_dir_all(minecraft.join("mods")).unwrap();
    let old = minecraft.join("mods/a.jar");
    std::fs::write(&old, b"old").unwrap();
    let mut record = installed_record("a", "a1", true);
    record.fingerprint = super::super::manifest::fingerprint(&old).unwrap();
    let manifest = ContentManifest {
        version: 1,
        files: vec![record],
    };
    manifest.save(&manifest_path).unwrap();
    let a2 = release("a2", "a", Vec::new());
    let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let mut fake = provider(vec![a2.clone()]);
    fake.download_barrier = Some(barrier.clone());
    fake.download_pause = Some(std::sync::Arc::new(tokio::sync::Notify::new()));
    let registry = std::sync::Arc::new(fake.registry());
    let mut selected = root(a2);
    selected.installed_path = Some(old.clone());
    let plan = resolve(&registry, &manifest, &minecraft, &instance(), selected)
        .await
        .unwrap();
    let job_minecraft = minecraft.clone();
    let job_manifest = manifest_path.clone();
    let job =
        tokio::spawn(async move { install(&registry, &job_manifest, &job_minecraft, &plan).await });
    barrier.wait().await;
    job.abort();
    assert!(matches!(job.await, Err(error) if error.is_cancelled()));
    assert_eq!(std::fs::read(old).unwrap(), b"old");
    assert_eq!(
        ContentManifest::load(&manifest_path).unwrap().files,
        manifest.files
    );
    assert!(!minecraft.join("mods/a2.jar").exists());
    assert!(!std::fs::read_dir(&minecraft).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".rmcl-install-")
    }));
}

#[tokio::test]
async fn replacing_a_disabled_root_keeps_its_disabled_filename_and_record() {
    let temp = tempfile::tempdir().unwrap();
    let minecraft = temp.path().join("minecraft");
    let manifest_path = temp.path().join("manifest.json");
    let mut record = installed_record("a", "a1", false);
    record.relative_path = "mods/a.jar.disabled".into();
    let old = minecraft.join(&record.relative_path);
    std::fs::create_dir_all(old.parent().unwrap()).unwrap();
    std::fs::write(&old, b"old").unwrap();
    record.fingerprint = super::super::manifest::fingerprint(&old).unwrap();
    let manifest = ContentManifest {
        version: 1,
        files: vec![record],
    };
    manifest.save(&manifest_path).unwrap();
    let a2 = release("a2", "a", Vec::new());
    let registry = provider(vec![a2.clone()]).registry();
    let mut selected = root(a2);
    selected.installed_path = Some(old.clone());
    let plan = resolve(&registry, &manifest, &minecraft, &instance(), selected)
        .await
        .unwrap();
    let installed = install(&registry, &manifest_path, &minecraft, &plan)
        .await
        .unwrap();
    assert_eq!(installed.root_path, minecraft.join("mods/a2.jar.disabled"));
    assert_eq!(std::fs::read(&installed.root_path).unwrap(), b"a2");
    assert!(!old.exists());
    assert!(!ContentManifest::load(&manifest_path).unwrap().files[0].enabled);
}

#[tokio::test]
async fn installation_rejects_a_target_owned_by_another_record_even_when_missing() {
    let temp = tempfile::tempdir().unwrap();
    let minecraft = temp.path().join("minecraft");
    let manifest_path = temp.path().join("manifest.json");
    let a2 = release("a2", "a", Vec::new());
    let registry = provider(vec![a2.clone()]).registry();
    let plan = resolve(
        &registry,
        &ContentManifest::default(),
        &minecraft,
        &instance(),
        root(a2),
    )
    .await
    .unwrap();
    let mut record = installed_record("foreign", "foreign1", true);
    record.relative_path = "mods/a2.jar".into();
    let manifest = ContentManifest {
        version: 1,
        files: vec![record],
    };
    manifest.save(&manifest_path).unwrap();
    let error = install(&registry, &manifest_path, &minecraft, &plan)
        .await
        .err()
        .unwrap();
    assert!(error.to_string().contains("already owns"));
    assert_eq!(
        ContentManifest::load(&manifest_path).unwrap().files,
        manifest.files
    );
    assert!(!minecraft.join("mods/a2.jar").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn installation_rejects_linked_destinations_and_dangling_targets() {
    let temp = tempfile::tempdir().unwrap();
    let minecraft = temp.path().join("minecraft");
    let outside = temp.path().join("outside");
    std::fs::create_dir_all(&minecraft).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let manifest_path = temp.path().join("manifest.json");
    let a2 = release("a2", "a", Vec::new());
    let registry = provider(vec![a2.clone()]).registry();
    let plan = resolve(
        &registry,
        &ContentManifest::default(),
        &minecraft,
        &instance(),
        root(a2),
    )
    .await
    .unwrap();
    std::os::unix::fs::symlink(&outside, minecraft.join("mods")).unwrap();
    assert!(
        install(&registry, &manifest_path, &minecraft, &plan)
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("symlink")
    );
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    std::fs::remove_file(minecraft.join("mods")).unwrap();
    std::fs::create_dir(minecraft.join("mods")).unwrap();
    let target = minecraft.join("mods/a2.jar");
    std::os::unix::fs::symlink(outside.join("missing"), &target).unwrap();
    assert!(
        install(&registry, &manifest_path, &minecraft, &plan)
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("already exists")
    );
    assert!(
        std::fs::symlink_metadata(target)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(
        ContentManifest::load(&manifest_path)
            .unwrap()
            .files
            .is_empty()
    );
}
