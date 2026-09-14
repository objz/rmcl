// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::Utc;

use super::*;
use crate::instance::content::provider::{FingerprintQuery, ResolvedFile};
use crate::instance::{FileFingerprint, ModLoader, Resolution};
use crate::net::modrinth::{
    DependencyType, DiscoveryResults, ProjectInfo, VersionDependency, VersionType,
};

#[test]
fn concurrent_installs_use_distinct_staging_directories() {
    let minecraft = Path::new("/instance/minecraft");
    assert_ne!(staging_directory(minecraft), staging_directory(minecraft));
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

struct FakeProvider {
    versions: HashMap<String, VersionInfo>,
    compatible: HashMap<String, Vec<String>>,
    projects: HashMap<String, String>,
    project_types: HashMap<String, String>,
    categories: HashMap<String, Vec<String>>,
    resolved: HashMap<String, ProviderProject>,
    fail_project: Option<String>,
    fail_download: Option<String>,
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
        _offset: usize,
        _limit: usize,
    ) -> Result<DiscoveryResults, NetError> {
        unreachable!()
    }

    async fn search_modpacks(
        &self,
        _query: &str,
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
        })
    }

    async fn compatible_versions(
        &self,
        project_id: &str,
        _kind: ContentKind,
        _game_version: &str,
        _loader: ModLoader,
    ) -> Result<Vec<VersionInfo>, NetError> {
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
        let path = destination.join(format!("{}.{extension}", version.id));
        tokio::fs::write(&path, version.id.as_bytes()).await?;
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
    }
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

    let installed = install(&registry, &manifest_path, &minecraft, &plan)
        .await
        .unwrap();

    assert_eq!(installed.root_path, mods.join("root.jar"));
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
    assert_eq!(std::fs::read_dir(&mods).unwrap().count(), 0);
}
