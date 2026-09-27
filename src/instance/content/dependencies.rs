// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::instance::content::provider::{ContentProvider, FingerprintQuery, ProviderRegistry};
use crate::instance::{
    ContentFileRecord, ContentKind, ContentManifest, InstanceConfig, ProviderProject,
};
use crate::net::NetError;
use crate::net::modrinth::{
    DependencyType, ProjectInfo, VersionDependency, VersionInfo, VersionType,
};

static NEXT_INSTALL_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct InstallRoot {
    pub provider: String,
    pub project_id: String,
    pub title: String,
    pub version: VersionInfo,
    pub installed_path: Option<PathBuf>,
    pub kind: ContentKind,
    pub target_world: Option<PathBuf>,
    pub force_reinstall: bool,
}

#[derive(Debug, Clone)]
pub struct PlannedInstall {
    pub provider: String,
    pub project_id: String,
    pub title: String,
    pub version: VersionInfo,
    pub installed_path: Option<PathBuf>,
    pub kind: ContentKind,
    pub destination: PathBuf,
    pub provider_aliases: Vec<ProviderProject>,
    pub required_dependencies: Vec<ProviderProject>,
    pub automatic_dependency: bool,
    pub cleanup_eligible: bool,
    pub replacement: bool,
}

impl PlannedInstall {
    pub fn needs_download(&self) -> bool {
        self.installed_path.is_none() || self.replacement
    }

    pub fn identity(&self) -> ProviderProject {
        ProviderProject {
            provider: self.provider.clone(),
            project_id: self.project_id.clone(),
            version_id: self.version.id.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DependencyPlan {
    pub items: Vec<PlannedInstall>,
    pub root_count: usize,
    pub optional_dependencies: usize,
}

pub struct InstallResult {
    pub root_path: PathBuf,
    pub replaced: bool,
    pub skipped: bool,
    pub orphaned_dependencies: Vec<PathBuf>,
}

impl DependencyPlan {
    pub fn dependency_installs(&self) -> impl Iterator<Item = &PlannedInstall> {
        self.items
            .iter()
            .skip(self.root_count)
            .filter(|item| item.installed_path.is_none())
    }

    pub fn dependency_replacements(&self) -> impl Iterator<Item = &PlannedInstall> {
        self.items
            .iter()
            .skip(self.root_count)
            .filter(|item| item.replacement)
    }

    pub fn has_dependency_changes(&self) -> bool {
        self.dependency_installs().next().is_some()
            || self.dependency_replacements().next().is_some()
            || self.optional_dependencies > 0
    }
}

pub async fn install(
    registry: &ProviderRegistry,
    manifest_path: &Path,
    minecraft_dir: &Path,
    plan: &DependencyPlan,
) -> Result<InstallResult, NetError> {
    let root = plan
        .items
        .first()
        .ok_or_else(|| NetError::Parse("Dependency plan is empty".to_owned()))?;
    for item in &plan.items {
        tokio::fs::create_dir_all(&item.destination).await?;
    }
    let staging = create_staging_directory(minecraft_dir).await?;

    let result = install_staged(registry, manifest_path, minecraft_dir, &staging, plan).await;
    if let Err(error) = tokio::fs::remove_dir_all(&staging).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!(
            "Failed to remove dependency install staging directory '{}': {error}",
            staging.display()
        );
    }
    result.map(|(root_path, orphaned_dependencies)| InstallResult {
        root_path,
        replaced: root.replacement,
        skipped: !plan.items.iter().any(PlannedInstall::needs_download),
        orphaned_dependencies,
    })
}

fn staging_directory(minecraft_dir: &Path) -> PathBuf {
    minecraft_dir.join(format!(
        ".rmcl-install-{}",
        NEXT_INSTALL_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

async fn create_staging_directory(minecraft_dir: &Path) -> Result<PathBuf, NetError> {
    loop {
        let staging = staging_directory(minecraft_dir);
        match tokio::fs::create_dir(&staging).await {
            Ok(()) => return Ok(staging),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
}

struct StagedFile {
    item: usize,
    source: PathBuf,
    target: PathBuf,
}

struct CommittedFile {
    target: PathBuf,
    old_path: Option<PathBuf>,
    backup: Option<PathBuf>,
}

async fn install_staged(
    registry: &ProviderRegistry,
    manifest_path: &Path,
    minecraft_dir: &Path,
    staging: &Path,
    plan: &DependencyPlan,
) -> Result<(PathBuf, Vec<PathBuf>), NetError> {
    let mut staged = Vec::new();
    let mut targets = HashSet::new();
    for (index, item) in plan.items.iter().enumerate() {
        if !item.needs_download() {
            continue;
        }
        let provider = registry.get(&item.provider).ok_or_else(|| {
            NetError::Parse(format!("{} content provider is unavailable", item.provider))
        })?;
        let item_staging = staging.join(format!("item-{index}"));
        tokio::fs::create_dir(&item_staging).await?;
        let outcome = provider
            .download_version(&item.version, &item_staging, None)
            .await?;
        let source = match outcome {
            crate::net::modrinth::DownloadOutcome::Downloaded(path)
            | crate::net::modrinth::DownloadOutcome::SkippedExisting(path) => path,
        };
        let file_name = source.file_name().ok_or_else(|| {
            NetError::Parse(format!(
                "Downloaded dependency '{}' has no filename",
                item.title
            ))
        })?;
        let target = item.destination.join(file_name);
        if !targets.insert(target.clone()) {
            return Err(NetError::Parse(format!(
                "Multiple selected projects install '{}'",
                target.display()
            )));
        }
        if target.exists()
            && item
                .installed_path
                .as_ref()
                .is_none_or(|installed| installed != &target)
        {
            return Err(NetError::Parse(format!(
                "Cannot install '{}' because '{}' already exists",
                item.title,
                target.display()
            )));
        }
        staged.push(StagedFile {
            item: index,
            source,
            target,
        });
    }

    let mut committed = Vec::new();
    for (index, file) in staged.iter().enumerate() {
        let item = &plan.items[file.item];
        let old_path = item.installed_path.clone();
        let backup = if let Some(old_path) = old_path.as_ref() {
            let backup = staging.join(format!("backup-{index}"));
            if let Err(error) = tokio::fs::rename(old_path, &backup).await {
                rollback_files(&committed).await;
                return Err(error.into());
            }
            Some(backup)
        } else {
            None
        };
        if let Err(error) = tokio::fs::rename(&file.source, &file.target).await {
            if let (Some(old_path), Some(backup)) = (&old_path, &backup) {
                let _ = tokio::fs::rename(backup, old_path).await;
            }
            rollback_files(&committed).await;
            return Err(error.into());
        }
        committed.push(CommittedFile {
            target: file.target.clone(),
            old_path,
            backup,
        });
    }

    let previous =
        ContentManifest::load(manifest_path).map_err(|error| NetError::Parse(error.to_string()))?;
    let records = match build_records(plan, &previous, minecraft_dir, &staged) {
        Ok(records) => records,
        Err(error) => {
            rollback_files(&committed).await;
            return Err(error);
        }
    };
    if let Err(error) = ContentManifest::update(manifest_path, |manifest| {
        for (old_relative, record) in &records {
            if let Some(old_relative) = old_relative
                && old_relative != &record.relative_path
            {
                manifest.remove(old_relative);
            }
            manifest.upsert(record.clone());
        }
        Ok(())
    }) {
        rollback_files(&committed).await;
        return Err(NetError::Parse(error.to_string()));
    }
    for committed in &committed {
        if let Some(backup) = &committed.backup
            && let Err(error) = tokio::fs::remove_file(backup).await
        {
            tracing::warn!(
                "Failed to remove dependency install backup '{}': {error}",
                backup.display()
            );
        }
    }

    let orphaned_dependencies = match ContentManifest::load(manifest_path) {
        Ok(updated) => updated
            .orphaned_dependencies()
            .into_iter()
            .map(|relative| minecraft_dir.join(relative))
            .collect(),
        Err(error) => {
            tracing::warn!("Failed to check for unused dependencies: {error}");
            Vec::new()
        }
    };
    let root = &plan.items[0];
    let root_path = if root.needs_download() {
        staged
            .iter()
            .find(|file| file.item == 0)
            .map(|file| file.target.clone())
            .ok_or_else(|| NetError::Parse("Installed root file is missing".to_owned()))
    } else {
        root.installed_path
            .clone()
            .ok_or_else(|| NetError::Parse("Installed root path is missing".to_owned()))
    }?;
    Ok((root_path, orphaned_dependencies))
}

fn build_records(
    plan: &DependencyPlan,
    previous: &ContentManifest,
    minecraft_dir: &Path,
    staged: &[StagedFile],
) -> Result<Vec<(Option<PathBuf>, ContentFileRecord)>, NetError> {
    plan.items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let old_relative = item
                .installed_path
                .as_ref()
                .and_then(|path| path.strip_prefix(minecraft_dir).ok())
                .map(Path::to_owned);
            if !item.needs_download() {
                let old_relative = old_relative.ok_or_else(|| {
                    NetError::Parse(format!("Installed path for '{}' is invalid", item.title))
                })?;
                let mut record = previous.record(&old_relative).cloned().ok_or_else(|| {
                    NetError::Parse(format!("Manifest record for '{}' is missing", item.title))
                })?;
                let identity = item.identity();
                if !record.matches_project(&identity.provider, &identity.project_id)
                    && !record.provider_aliases.contains(&identity)
                {
                    record.provider_aliases.push(identity);
                }
                record.required_dependencies = item.required_dependencies.clone();
                record.automatic_dependency = item.automatic_dependency;
                record.cleanup_eligible = item.cleanup_eligible;
                return Ok((Some(old_relative), record));
            }
            let path = staged
                .iter()
                .find(|file| file.item == index)
                .map(|file| file.target.clone())
                .ok_or_else(|| {
                    NetError::Parse(format!("Staged file for '{}' is missing", item.title))
                })?;
            let relative_path = path
                .strip_prefix(minecraft_dir)
                .map_err(|error| NetError::Parse(error.to_string()))?
                .to_owned();
            Ok((
                old_relative,
                ContentFileRecord {
                    relative_path,
                    kind: item.kind,
                    enabled: true,
                    fingerprint: crate::instance::content::manifest::fingerprint(&path)?,
                    resolution: crate::instance::Resolution::Resolved {
                        project: item.identity(),
                    },
                    provider_aliases: item.provider_aliases.clone(),
                    provider_checks: vec![item.provider.clone()],
                    required_dependencies: item.required_dependencies.clone(),
                    automatic_dependency: item.automatic_dependency,
                    cleanup_eligible: item.cleanup_eligible,
                },
            ))
        })
        .collect()
}

async fn rollback_files(committed: &[CommittedFile]) {
    for committed in committed.iter().rev() {
        if let Err(error) = tokio::fs::remove_file(&committed.target).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                "Failed to remove rolled back content '{}': {error}",
                committed.target.display()
            );
        }
        if let (Some(old_path), Some(backup)) = (&committed.old_path, &committed.backup)
            && let Err(error) = tokio::fs::rename(backup, old_path).await
        {
            tracing::warn!(
                "Failed to restore rolled back content '{}': {error}",
                old_path.display()
            );
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProjectKey {
    provider: String,
    project_id: String,
}

impl ProjectKey {
    fn new(provider: &str, project_id: impl Into<String>) -> Self {
        Self {
            provider: provider.to_owned(),
            project_id: project_id.into(),
        }
    }
}

#[derive(Debug, Clone)]
struct Node {
    title: String,
    kind: ContentKind,
    version: VersionInfo,
    installed: Option<InstalledMatch>,
    automatic_dependency: bool,
    cleanup_eligible: bool,
    exact: bool,
    dependencies: Vec<ProjectKey>,
    optional_dependencies: usize,
    incompatible: Vec<Incompatible>,
}

#[derive(Debug, Clone)]
struct InstalledMatch {
    relative_path: PathBuf,
    identity: ProviderProject,
    aliases: Vec<ProviderProject>,
    automatic_dependency: bool,
}

#[derive(Debug, Clone)]
struct Incompatible {
    project_id: String,
    version_id: Option<String>,
}

#[derive(Debug, Clone)]
struct Requirement {
    parent: ProjectKey,
    parent_version_id: String,
    dependency: VersionDependency,
    ancestors: Vec<ProjectKey>,
}

pub async fn resolve(
    registry: &ProviderRegistry,
    manifest: &ContentManifest,
    minecraft_dir: &Path,
    instance: &InstanceConfig,
    root: InstallRoot,
) -> Result<DependencyPlan, NetError> {
    if root.kind == ContentKind::DataPack && root.target_world.is_none() {
        return Err(NetError::Parse(
            "A target world is required for datapack installation".to_owned(),
        ));
    }
    let provider = registry.get(&root.provider).ok_or_else(|| {
        NetError::Parse(format!("{} content provider is unavailable", root.provider))
    })?;
    let root_version = provider.version(&root.version.id).await?;
    validate_compatible(&root_version, instance, root.kind)?;
    if !root_version.project_id.is_empty() && root_version.project_id != root.project_id {
        return Err(NetError::Parse(format!(
            "Selected version belongs to project '{}', not '{}'",
            root_version.project_id, root.project_id
        )));
    }
    let remote_matches = resolve_installed_files(provider, manifest).await?;
    let root_key = ProjectKey::new(&root.provider, root.project_id.clone());
    let root_installed = installed_root(manifest, minecraft_dir, &root, &remote_matches);
    let mut nodes = HashMap::from([(
        root_key.clone(),
        Node {
            title: root.title,
            kind: root.kind,
            version: root_version.clone(),
            automatic_dependency: false,
            cleanup_eligible: false,
            exact: true,
            installed: root_installed,
            dependencies: Vec::new(),
            optional_dependencies: 0,
            incompatible: Vec::new(),
        },
    )]);
    let mut queue = VecDeque::new();
    expand_node(
        provider,
        &root_key,
        &root_version,
        vec![root_key.clone()],
        &mut nodes,
        &mut queue,
    )
    .await?;

    while let Some(requirement) = queue.pop_front() {
        if nodes
            .get(&requirement.parent)
            .is_none_or(|parent| parent.version.id != requirement.parent_version_id)
        {
            continue;
        }
        let exact = requirement.dependency.version_id.is_some();
        let parent_kind = nodes
            .get(&requirement.parent)
            .map(|node| node.kind)
            .unwrap_or(root.kind);
        let choice = resolve_requirement(
            provider,
            manifest,
            &remote_matches,
            instance,
            parent_kind,
            root.target_world.as_deref(),
            &requirement.dependency,
        )
        .await?;
        if choice.kind == ContentKind::DataPack && root.target_world.is_none() {
            return Err(NetError::Parse(format!(
                "Datapack dependency '{}' requires a target world",
                choice.title
            )));
        }
        let key = ProjectKey::new(provider.id(), choice.version.project_id.clone());
        if requirement.ancestors.contains(&key) {
            return Err(NetError::Parse(format!(
                "Dependency cycle detected at '{}'",
                choice.title
            )));
        }
        if let Some(parent) = nodes.get_mut(&requirement.parent)
            && !parent.dependencies.contains(&key)
        {
            parent.dependencies.push(key.clone());
        }

        let replace_selection = match nodes.get(&key) {
            Some(existing)
                if exact && existing.exact && existing.version.id != choice.version.id =>
            {
                return Err(NetError::Parse(format!(
                    "Conflicting required versions for '{}': '{}' and '{}'",
                    choice.title, existing.version.version_number, choice.version.version_number
                )));
            }
            Some(existing) => exact && !existing.exact && existing.version.id != choice.version.id,
            None => true,
        };
        if !replace_selection {
            continue;
        }

        let mut ancestors = requirement.ancestors;
        ancestors.push(key.clone());
        nodes.insert(
            key.clone(),
            Node {
                title: choice.title,
                kind: choice.kind,
                version: choice.version.clone(),
                installed: choice.installed,
                automatic_dependency: choice.automatic_dependency,
                cleanup_eligible: choice.cleanup_eligible,
                exact,
                dependencies: Vec::new(),
                optional_dependencies: 0,
                incompatible: Vec::new(),
            },
        );
        expand_node(
            provider,
            &key,
            &choice.version,
            ancestors,
            &mut nodes,
            &mut queue,
        )
        .await?;
    }

    let order = reachable_order(&root_key, &nodes);
    reject_installed_incompatibilities(
        provider,
        manifest,
        &remote_matches,
        &order,
        &nodes,
        root.target_world.as_deref(),
    )?;
    let optional_dependencies = order
        .iter()
        .filter_map(|key| nodes.get(key))
        .map(|node| node.optional_dependencies)
        .sum();
    let items = order
        .iter()
        .filter_map(|key| {
            nodes.get(key).map(|node| {
                planned_install(
                    key,
                    node,
                    &nodes,
                    minecraft_dir,
                    root.target_world.as_deref(),
                    root.force_reinstall && key == &root_key,
                )
            })
        })
        .collect();
    Ok(DependencyPlan {
        items,
        root_count: 1,
        optional_dependencies,
    })
}

pub fn merge(plans: Vec<DependencyPlan>) -> Result<DependencyPlan, NetError> {
    let optional_dependencies = plans.iter().map(|plan| plan.optional_dependencies).sum();
    let mut merged: Vec<(PlannedInstall, bool)> = Vec::new();
    for plan in plans {
        for (index, item) in plan.items.into_iter().enumerate() {
            let root = index < plan.root_count;
            if let Some((existing, existing_root)) = merged.iter_mut().find(|(existing, _)| {
                existing.provider == item.provider
                    && existing.project_id == item.project_id
                    && existing.destination == item.destination
            }) {
                if existing.version.id != item.version.id {
                    return Err(NetError::Parse(format!(
                        "Conflicting selected versions for '{}': '{}' and '{}'",
                        item.title, existing.version.version_number, item.version.version_number
                    )));
                }
                for dependency in item.required_dependencies {
                    if !existing.required_dependencies.contains(&dependency) {
                        existing.required_dependencies.push(dependency);
                    }
                }
                for alias in item.provider_aliases {
                    if !existing.provider_aliases.contains(&alias) {
                        existing.provider_aliases.push(alias);
                    }
                }
                if root && !*existing_root {
                    existing.title = item.title;
                    if item.installed_path.is_some() {
                        existing.installed_path = item.installed_path;
                    }
                    existing.replacement |= item.replacement;
                    existing.automatic_dependency = false;
                    existing.cleanup_eligible = false;
                    *existing_root = true;
                }
                continue;
            }
            merged.push((item, root));
        }
    }
    merged.sort_by_key(|(_, root)| !*root);
    let root_count = merged.iter().take_while(|(_, root)| *root).count();
    Ok(DependencyPlan {
        items: merged.into_iter().map(|(item, _)| item).collect(),
        root_count,
        optional_dependencies,
    })
}

struct ResolvedChoice {
    title: String,
    kind: ContentKind,
    version: VersionInfo,
    installed: Option<InstalledMatch>,
    automatic_dependency: bool,
    cleanup_eligible: bool,
}

async fn resolve_requirement(
    provider: &dyn ContentProvider,
    manifest: &ContentManifest,
    remote_matches: &HashMap<PathBuf, ProviderProject>,
    instance: &InstanceConfig,
    parent_kind: ContentKind,
    target_world: Option<&Path>,
    dependency: &VersionDependency,
) -> Result<ResolvedChoice, NetError> {
    let exact_version = match dependency.version_id.as_deref() {
        Some(version_id) => Some(provider.version(version_id).await?),
        None => None,
    };
    let project_id = dependency
        .project_id
        .clone()
        .or_else(|| {
            exact_version
                .as_ref()
                .map(|version| version.project_id.clone())
        })
        .filter(|project_id| !project_id.is_empty())
        .ok_or_else(|| {
            NetError::Parse(format!(
                "Required dependency '{}' has no provider project",
                dependency.file_name.as_deref().unwrap_or("unknown")
            ))
        })?;
    let project = match provider.project(&project_id).await {
        Ok(project) => Some(project),
        Err(error) => {
            tracing::warn!(
                "Could not load metadata for dependency '{project_id}'; keeping it out of automatic cleanup: {error}"
            );
            None
        }
    };
    let kind = dependency_kind(project.as_ref(), exact_version.as_ref(), parent_kind);
    let installed = find_installed(
        manifest,
        remote_matches,
        provider.id(),
        &project_id,
        kind,
        target_world,
    );
    let version = if let Some(version) = exact_version {
        version
    } else {
        let installed_version = match &installed {
            Some(installed) => match provider.version(&installed.identity.version_id).await {
                Ok(version) => Some(version),
                Err(error) => {
                    tracing::warn!(
                        "Could not load installed dependency version '{}'; selecting a compatible replacement: {error}",
                        installed.identity.version_id
                    );
                    None
                }
            },
            None => None,
        };
        match installed_version {
            Some(version) if validate_compatible(&version, instance, kind).is_ok() => version,
            _ => {
                let versions = provider
                    .compatible_versions(&project_id, kind, &instance.game_version, instance.loader)
                    .await?;
                select_preferred_version(versions).ok_or_else(|| {
                    NetError::Parse(format!(
                        "No compatible dependency version found for project '{project_id}'"
                    ))
                })?
            }
        }
    };
    validate_compatible(&version, instance, kind)?;
    if !version.project_id.is_empty() && version.project_id != project_id {
        return Err(NetError::Parse(format!(
            "Dependency version '{}' belongs to project '{}', not '{}'",
            version.version_number, version.project_id, project_id
        )));
    }
    let automatic_dependency = installed
        .as_ref()
        .is_none_or(|installed| installed.automatic_dependency);
    let cleanup_eligible =
        automatic_dependency && project.as_ref().is_some_and(ProjectInfo::is_library_only);
    Ok(ResolvedChoice {
        title: project
            .map(|project| project.title)
            .unwrap_or_else(|| project_id.clone()),
        version,
        kind,
        automatic_dependency,
        cleanup_eligible,
        installed,
    })
}

async fn expand_node(
    provider: &dyn ContentProvider,
    key: &ProjectKey,
    version: &VersionInfo,
    ancestors: Vec<ProjectKey>,
    nodes: &mut HashMap<ProjectKey, Node>,
    queue: &mut VecDeque<Requirement>,
) -> Result<(), NetError> {
    let mut required = Vec::new();
    let mut incompatible = Vec::new();
    let mut optional_dependencies = 0;
    for dependency in &version.dependencies {
        match dependency.dependency_type {
            DependencyType::Required => required.push(dependency.clone()),
            DependencyType::Optional => optional_dependencies += 1,
            DependencyType::Incompatible => {
                let (project_id, version_id) = dependency_identity(provider, dependency).await?;
                incompatible.push(Incompatible {
                    project_id,
                    version_id,
                });
            }
            DependencyType::Embedded | DependencyType::Unknown => {}
        }
    }
    if let Some(node) = nodes.get_mut(key) {
        node.optional_dependencies = optional_dependencies;
        node.incompatible = incompatible;
    }
    queue.extend(required.into_iter().map(|dependency| Requirement {
        parent: key.clone(),
        parent_version_id: version.id.clone(),
        dependency,
        ancestors: ancestors.clone(),
    }));
    Ok(())
}

async fn dependency_identity(
    provider: &dyn ContentProvider,
    dependency: &VersionDependency,
) -> Result<(String, Option<String>), NetError> {
    if let Some(project_id) = dependency.project_id.clone() {
        return Ok((project_id, dependency.version_id.clone()));
    }
    let version_id = dependency.version_id.as_deref().ok_or_else(|| {
        NetError::Parse("Incompatible dependency has no provider project".to_owned())
    })?;
    let version = provider.version(version_id).await?;
    Ok((version.project_id, Some(version_id.to_owned())))
}

fn select_preferred_version(mut versions: Vec<VersionInfo>) -> Option<VersionInfo> {
    versions.sort_by(|left, right| {
        release_rank(left.version_type)
            .cmp(&release_rank(right.version_type))
            .then_with(|| right.date_published.cmp(&left.date_published))
    });
    versions.into_iter().next()
}

fn dependency_kind(
    project: Option<&ProjectInfo>,
    version: Option<&VersionInfo>,
    parent_kind: ContentKind,
) -> ContentKind {
    if version.is_some_and(|version| {
        version
            .loaders
            .iter()
            .any(|loader| loader.eq_ignore_ascii_case("datapack"))
    }) {
        return ContentKind::DataPack;
    }
    if version.is_some_and(|version| {
        version.loaders.iter().any(|loader| {
            matches!(
                loader.to_ascii_lowercase().as_str(),
                "fabric" | "forge" | "neoforge" | "quilt"
            )
        })
    }) {
        return ContentKind::Mod;
    }
    if project.is_some_and(|project| {
        project
            .loaders
            .iter()
            .any(|loader| loader.eq_ignore_ascii_case("datapack"))
    }) {
        return ContentKind::DataPack;
    }
    match project.map(|project| project.project_type.as_str()) {
        Some("resourcepack") => ContentKind::ResourcePack,
        Some("shader") => ContentKind::Shader,
        Some("datapack") => ContentKind::DataPack,
        Some("mod") => ContentKind::Mod,
        _ => parent_kind,
    }
}

fn release_rank(version_type: VersionType) -> u8 {
    match version_type {
        VersionType::Release => 0,
        VersionType::Beta => 1,
        VersionType::Alpha => 2,
        VersionType::Unknown => 3,
    }
}

fn validate_compatible(
    version: &VersionInfo,
    instance: &InstanceConfig,
    kind: ContentKind,
) -> Result<(), NetError> {
    let loader = instance.loader.to_string().to_ascii_lowercase();
    let supports_game = version
        .game_versions
        .iter()
        .any(|game_version| game_version == &instance.game_version);
    let supports_loader = kind != ContentKind::Mod
        || version
            .loaders
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(&loader));
    if !supports_game || !supports_loader {
        return Err(NetError::Parse(format!(
            "Required dependency '{}' does not support Minecraft {} with {}",
            version.version_number, instance.game_version, instance.loader
        )));
    }
    Ok(())
}

async fn resolve_installed_files(
    provider: &dyn ContentProvider,
    manifest: &ContentManifest,
) -> Result<HashMap<PathBuf, ProviderProject>, NetError> {
    let files = manifest
        .files
        .iter()
        .filter(|record| record.enabled)
        .map(|record| FingerprintQuery {
            key: record.relative_path.to_string_lossy().into_owned(),
            kind: record.kind,
            fingerprint: record.fingerprint.clone(),
        })
        .collect::<Vec<_>>();
    if files.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(provider
        .resolve_files(&files)
        .await?
        .into_iter()
        .map(|resolved| (PathBuf::from(resolved.key), resolved.project))
        .collect())
}

fn installed_root(
    manifest: &ContentManifest,
    minecraft_dir: &Path,
    root: &InstallRoot,
    remote_matches: &HashMap<PathBuf, ProviderProject>,
) -> Option<InstalledMatch> {
    let path = root.installed_path.as_ref()?;
    let relative_path = path.strip_prefix(minecraft_dir).ok()?.to_owned();
    let record = manifest.record(&relative_path)?;
    Some(installed_match(
        record,
        remote_matches.get(&relative_path),
        &root.provider,
        &root.project_id,
    ))
}

fn find_installed(
    manifest: &ContentManifest,
    remote_matches: &HashMap<PathBuf, ProviderProject>,
    provider: &str,
    project_id: &str,
    kind: ContentKind,
    target_world: Option<&Path>,
) -> Option<InstalledMatch> {
    manifest
        .files
        .iter()
        .filter(|record| record.enabled && record.kind == kind)
        .filter(|record| record_in_target(record, kind, target_world))
        .find_map(|record| {
            let remote = remote_matches.get(&record.relative_path);
            if !record.matches_project(provider, project_id)
                && !remote.is_some_and(|project| {
                    project.provider == provider && project.project_id == project_id
                })
            {
                return None;
            }
            Some(installed_match(record, remote, provider, project_id))
        })
}

fn record_in_target(
    record: &ContentFileRecord,
    kind: ContentKind,
    target_world: Option<&Path>,
) -> bool {
    if kind != ContentKind::DataPack {
        return true;
    }
    let Some(world_name) = target_world.and_then(Path::file_name) else {
        return false;
    };
    record
        .relative_path
        .starts_with(Path::new("saves").join(world_name).join("datapacks"))
}

fn installed_match(
    record: &ContentFileRecord,
    remote: Option<&ProviderProject>,
    provider: &str,
    project_id: &str,
) -> InstalledMatch {
    let identity = record
        .project_for_provider(provider, project_id)
        .or_else(|| {
            remote
                .filter(|project| project.provider == provider && project.project_id == project_id)
        })
        .cloned()
        .unwrap_or_else(|| ProviderProject {
            provider: provider.to_owned(),
            project_id: project_id.to_owned(),
            version_id: String::new(),
        });
    let mut aliases = record.provider_aliases.clone();
    if let Some(project) = record.resolved_project()
        && project != &identity
        && !aliases.contains(project)
    {
        aliases.push(project.clone());
    }
    if record.resolved_project() != Some(&identity) && !aliases.contains(&identity) {
        aliases.push(identity.clone());
    }
    InstalledMatch {
        relative_path: record.relative_path.clone(),
        identity,
        aliases,
        automatic_dependency: record.automatic_dependency,
    }
}

fn reachable_order(root: &ProjectKey, nodes: &HashMap<ProjectKey, Node>) -> Vec<ProjectKey> {
    let mut order = Vec::new();
    let mut pending = VecDeque::from([root.clone()]);
    let mut seen = HashSet::new();
    while let Some(key) = pending.pop_front() {
        if !seen.insert(key.clone()) {
            continue;
        }
        order.push(key.clone());
        if let Some(node) = nodes.get(&key) {
            pending.extend(node.dependencies.iter().cloned());
        }
    }
    order
}

fn reject_installed_incompatibilities(
    provider: &dyn ContentProvider,
    manifest: &ContentManifest,
    remote_matches: &HashMap<PathBuf, ProviderProject>,
    order: &[ProjectKey],
    nodes: &HashMap<ProjectKey, Node>,
    target_world: Option<&Path>,
) -> Result<(), NetError> {
    for key in order {
        let Some(node) = nodes.get(key) else {
            continue;
        };
        for incompatible in &node.incompatible {
            let installed = find_installed_incompatible(
                manifest,
                remote_matches,
                provider.id(),
                &incompatible.project_id,
                target_world,
            );
            let selected = order.iter().find_map(|selected_key| {
                (selected_key.provider == provider.id()
                    && selected_key.project_id == incompatible.project_id)
                    .then(|| nodes.get(selected_key))
                    .flatten()
            });
            let conflicts = installed.as_ref().is_some_and(|installed| {
                incompatible
                    .version_id
                    .as_ref()
                    .is_none_or(|version_id| installed.identity.version_id == *version_id)
            }) || selected.is_some_and(|selected| {
                incompatible
                    .version_id
                    .as_ref()
                    .is_none_or(|version_id| selected.version.id == *version_id)
            });
            if conflicts {
                return Err(NetError::Parse(format!(
                    "'{}' is incompatible with installed project '{}'",
                    node.title, incompatible.project_id
                )));
            }
        }
    }
    Ok(())
}

fn find_installed_incompatible(
    manifest: &ContentManifest,
    remote_matches: &HashMap<PathBuf, ProviderProject>,
    provider: &str,
    project_id: &str,
    target_world: Option<&Path>,
) -> Option<InstalledMatch> {
    manifest
        .files
        .iter()
        .filter(|record| record.enabled)
        .filter(|record| {
            record.kind != ContentKind::DataPack
                || record_in_target(record, ContentKind::DataPack, target_world)
        })
        .find_map(|record| {
            let remote = remote_matches.get(&record.relative_path);
            (record.matches_project(provider, project_id)
                || remote.is_some_and(|project| {
                    project.provider == provider && project.project_id == project_id
                }))
            .then(|| installed_match(record, remote, provider, project_id))
        })
}

fn planned_install(
    key: &ProjectKey,
    node: &Node,
    nodes: &HashMap<ProjectKey, Node>,
    minecraft_dir: &Path,
    target_world: Option<&Path>,
    force_reinstall: bool,
) -> PlannedInstall {
    let installed_path = node
        .installed
        .as_ref()
        .map(|installed| minecraft_dir.join(&installed.relative_path));
    let replacement = force_reinstall
        || node.installed.as_ref().is_some_and(|installed| {
            installed.identity.version_id.is_empty()
                || installed.identity.version_id != node.version.id
        });
    PlannedInstall {
        provider: key.provider.clone(),
        project_id: key.project_id.clone(),
        title: node.title.clone(),
        version: node.version.clone(),
        installed_path,
        kind: node.kind,
        destination: match node.kind {
            ContentKind::DataPack => target_world
                .expect("datapack dependency plan requires a world")
                .join("datapacks"),
            kind => minecraft_dir.join(kind.directory()),
        },
        provider_aliases: node.installed.as_ref().map_or_else(Vec::new, |installed| {
            if replacement {
                Vec::new()
            } else {
                installed.aliases.clone()
            }
        }),
        required_dependencies: node
            .dependencies
            .iter()
            .filter_map(|dependency| {
                nodes
                    .get(dependency)
                    .map(|dependency_node| ProviderProject {
                        provider: dependency.provider.clone(),
                        project_id: dependency.project_id.clone(),
                        version_id: dependency_node.version.id.clone(),
                    })
            })
            .collect(),
        automatic_dependency: node.automatic_dependency,
        cleanup_eligible: node.cleanup_eligible,
        replacement,
    }
}

#[cfg(test)]
#[path = "../tests/content/dependencies.rs"]
mod tests;
