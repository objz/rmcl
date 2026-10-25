// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

mod curseforge;
mod mmc;
mod mrpack;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::content::local::current_fingerprint;
use super::content::manifest::{
    ContentLock, ContentManifest, FileFingerprint, fingerprint_metadata, fingerprint_with_check,
};
pub use super::pack::PackFormat;
use super::{InstanceConfig, InstanceManager, ModLoader};
use crate::feedback::progress::{ProgressTask, ProgressTaskHandle};

pub type ExportResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub format: PackFormat,
    pub name: String,
    pub version: String,
    pub author: String,
    pub game_version: String,
    pub loader: ModLoader,
    pub loader_version: Option<String>,
    pub include: Vec<PathBuf>,
    pub bundle_unmatched: bool,
}

impl ExportOptions {
    pub fn for_instance(instance: &InstanceConfig) -> Self {
        Self {
            format: PackFormat::CurseForge,
            name: instance.name.clone(),
            version: "1.0.0".to_owned(),
            author: String::new(),
            game_version: instance.game_version.clone(),
            loader: instance.loader,
            loader_version: instance.loader_version.clone(),
            include: Vec::new(),
            bundle_unmatched: false,
        }
    }
}

pub struct ExportPlan {
    pub options: ExportOptions,
    pub unmatched: Vec<PathBuf>,
    root: PathBuf,
    files: BTreeMap<PathBuf, FileFingerprint>,
    archive: ArchiveContents,
    manifest_path: Option<PathBuf>,
    _lock: Option<Arc<super::runtime::InstanceLock>>,
    explicit_selection: bool,
}

impl ExportPlan {
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub fn referenced_count(&self) -> usize {
        self.archive.referenced.len()
    }

    pub fn referenced_paths(&self) -> &BTreeSet<PathBuf> {
        &self.archive.referenced
    }
}

pub struct ExportScan {
    config: InstanceConfig,
    root: PathBuf,
    files: BTreeMap<PathBuf, FileFingerprint>,
    manifest_path: PathBuf,
    pub defaults: BTreeSet<PathBuf>,
    pub skipped: Vec<PathBuf>,
    lock: Arc<super::runtime::InstanceLock>,
}

pub enum ExportReferences {
    CurseForge(BTreeMap<u32, crate::net::modrinth::VersionInfo>),
    Mrpack(HashMap<String, crate::net::modrinth::VersionInfo>),
    Bundled,
}

impl ExportReferences {
    pub fn empty(format: PackFormat) -> Self {
        match format {
            PackFormat::CurseForge => Self::CurseForge(BTreeMap::new()),
            PackFormat::Mrpack => Self::Mrpack(HashMap::new()),
            PackFormat::Mmc => Self::Bundled,
        }
    }
}

impl ExportScan {
    pub fn paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.files.keys()
    }

    pub fn needs_references(&self, format: PackFormat, selected: &BTreeSet<PathBuf>) -> bool {
        selected
            .iter()
            .any(|path| reference_candidate(path, format))
    }

    pub fn unmatched(
        &self,
        format: PackFormat,
        selected: &BTreeSet<PathBuf>,
        referenced: &BTreeSet<PathBuf>,
    ) -> Vec<PathBuf> {
        selected
            .iter()
            .filter(|path| {
                format != PackFormat::Mmc
                    && content_path(path, format)
                    && !referenced.contains(*path)
            })
            .cloned()
            .collect()
    }

    pub fn plan(
        &self,
        options: &ExportOptions,
        selected: &BTreeSet<PathBuf>,
        references: &ExportReferences,
    ) -> ExportResult<ExportPlan> {
        let mut options = options.clone();
        options.game_version = self.config.game_version.clone();
        options.loader = self.config.loader;
        options.loader_version = self.config.loader_version.clone();
        validate_options(&options)?;
        let files = selected
            .iter()
            .map(|path| {
                self.files
                    .get(path)
                    .cloned()
                    .map(|file| (path.clone(), file))
                    .ok_or_else(|| format!("File was not scanned: {}", path.display()).into())
            })
            .collect::<ExportResult<BTreeMap<_, _>>>()?;
        let mut plan = ExportPlan {
            root: self.root.clone(),
            files,
            options,
            unmatched: Vec::new(),
            manifest_path: Some(self.manifest_path.clone()),
            _lock: Some(self.lock.clone()),
            explicit_selection: true,
            archive: ArchiveContents {
                manifests: Vec::new(),
                referenced: BTreeSet::new(),
                prefix: "overrides",
            },
        };
        plan.archive = match (plan.options.format, references) {
            (PackFormat::CurseForge, ExportReferences::CurseForge(files)) => {
                curseforge::manifest(&plan, files)?
            }
            (PackFormat::Mrpack, ExportReferences::Mrpack(files)) => {
                mrpack::manifest(&plan, files)?
            }
            (PackFormat::Mmc, ExportReferences::Bundled) => mmc::prepare(&plan.options)?,
            _ => return Err("Content identification does not match the export format".into()),
        };
        plan.unmatched = self.unmatched(plan.options.format, selected, &plan.archive.referenced);
        Ok(plan)
    }
}

pub async fn scan_instance_export(
    manager: &InstanceManager,
    name: &str,
) -> ExportResult<ExportScan> {
    let lock = Arc::new(super::runtime::lock_instance(&manager.instances_dir, name)?);
    let config = manager.load_one(name)?;
    let paths = crate::storage::InstancePaths::new(manager.instances_dir.join(name));
    let root = paths.minecraft();
    // Dropping the awaiting future closes this channel, including when the popup aborts it.
    let (lifetime, receiver) = tokio::sync::oneshot::channel::<()>();
    let result = tokio::task::spawn_blocking(move || {
        let check = || -> std::io::Result<()> {
            if lifetime.is_closed() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "Export scan cancelled",
                ));
            }
            Ok(())
        };
        check()?;
        let progress = ProgressTask::start("Scanning export files");
        let root = root.canonicalize()?;
        let manifest_path = paths.content_manifest();
        let cached = load_fingerprints(&manifest_path);
        let mut files = BTreeMap::new();
        let mut skipped = Vec::new();
        scan_directory(
            &root,
            Path::new(""),
            &mut files,
            &mut skipped,
            &cached,
            &progress,
            &check,
        )?;
        check()?;
        let defaults = files
            .keys()
            .filter(|path| {
                DEFAULT_PATHS
                    .iter()
                    .any(|directory| path.starts_with(directory))
            })
            .cloned()
            .collect();
        Ok(ExportScan {
            config,
            root,
            files,
            manifest_path,
            defaults,
            skipped,
            lock,
        })
    })
    .await?;
    drop(receiver);
    result
}

fn scan_directory(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<PathBuf, FileFingerprint>,
    skipped: &mut Vec<PathBuf>,
    cached: &ContentManifest,
    progress: &ProgressTask,
    check: &impl Fn() -> std::io::Result<()>,
) -> ExportResult<()> {
    check()?;
    for entry in std::fs::read_dir(root.join(directory))? {
        check()?;
        let entry = entry?;
        let relative = directory.join(entry.file_name());
        let metadata = std::fs::symlink_metadata(entry.path())?;
        if !portable_export_path(&relative)
            || metadata.file_type().is_symlink()
            || !(metadata.is_dir() || metadata.is_file())
            || metadata.is_file() && relative.to_string_lossy().ends_with(".disabled")
        {
            skipped.push(relative);
            continue;
        }
        if metadata.is_dir() {
            scan_directory(root, &relative, files, skipped, cached, progress, check)?;
        } else {
            progress.set_sub_action(relative.display().to_string());
            let full = root.join(&relative);
            let hash = if reference_candidate(&relative, PackFormat::Mrpack) {
                export_fingerprint(
                    &full,
                    cached.record(&relative).map(|record| &record.fingerprint),
                    check,
                )?
            } else {
                fingerprint_metadata(&full)?
            };
            files.insert(relative, hash);
        }
    }
    Ok(())
}

fn load_fingerprints(path: &Path) -> ContentManifest {
    ContentManifest::load(path).unwrap_or_else(|error| {
        tracing::warn!(
            "Could not read cached export fingerprints '{}': {error}",
            path.display()
        );
        ContentManifest::default()
    })
}

fn export_fingerprint(
    path: &Path,
    cached: Option<&FileFingerprint>,
    check: impl Fn() -> std::io::Result<()>,
) -> std::io::Result<FileFingerprint> {
    check()?;
    let metadata = fingerprint_metadata(path)?;
    if let Some(cached) = cached
        && cached.size == metadata.size
        && cached.modified_ns == metadata.modified_ns
        && ["sha1", "sha512", "curseforge"]
            .iter()
            .all(|algorithm| cached.hash(algorithm).is_some())
    {
        // Browsing can reuse the index; writing still rehashes every referenced file.
        return Ok(cached.clone());
    }
    fingerprint_with_check(path, check)
}

pub async fn resolve_export_files(
    scan: &ExportScan,
    format: PackFormat,
) -> ExportResult<ExportReferences> {
    let progress = ProgressTask::start(format!("Identifying {} export files", format.label()));
    match format {
        PackFormat::CurseForge => Ok(ExportReferences::CurseForge(
            curseforge::resolve(&scan.files, &progress).await?,
        )),
        PackFormat::Mrpack => Ok(ExportReferences::Mrpack(
            mrpack::resolve(&scan.files, &progress).await?,
        )),
        PackFormat::Mmc => Ok(ExportReferences::Bundled),
    }
}

#[derive(Debug)]
pub struct ExportReport {
    pub output: PathBuf,
    pub referenced_count: usize,
    pub bundled_count: usize,
    pub unmatched: Vec<PathBuf>,
}

struct ArchiveContents {
    manifests: Vec<(&'static str, Vec<u8>)>,
    referenced: BTreeSet<PathBuf>,
    prefix: &'static str,
}

const DEFAULT_PATHS: &[&str] = &[
    "mods",
    "config",
    "defaultconfigs",
    "kubejs",
    "scripts",
    "resourcepacks",
    "shaderpacks",
    "datapacks",
];

fn portable_export_path(relative: &Path) -> bool {
    relative.to_str().is_some_and(|path| {
        super::import::portable_pack_path(path)
            .is_ok_and(|normalized| cfg!(windows) || normalized == path)
    })
}

fn collect(root: &Path, relative: &Path, files: &mut BTreeSet<PathBuf>) -> ExportResult<()> {
    if !portable_export_path(relative) {
        return Err(format!("Invalid export path: {}", relative.display()).into());
    }
    let mut ancestor = root.to_path_buf();
    for component in relative.components() {
        ancestor.push(component);
        if std::fs::symlink_metadata(&ancestor)?
            .file_type()
            .is_symlink()
        {
            return Err(format!("Export does not follow symlinks: {}", relative.display()).into());
        }
    }
    let path = root.join(relative);
    let metadata = std::fs::symlink_metadata(&path)?;
    if metadata.is_dir() {
        for entry in std::fs::read_dir(path)? {
            collect(root, &relative.join(entry?.file_name()), files)?;
        }
    } else if metadata.is_file() {
        if !relative.to_string_lossy().ends_with(".disabled") {
            files.insert(relative.to_owned());
        }
    } else {
        return Err(format!("Not a regular file: {}", path.display()).into());
    }
    Ok(())
}

fn selected_files(root: &Path, include: &[PathBuf]) -> ExportResult<BTreeSet<PathBuf>> {
    let mut files = BTreeSet::new();
    for name in DEFAULT_PATHS {
        match std::fs::symlink_metadata(root.join(name)) {
            Ok(_) => collect(root, Path::new(name), &mut files)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    for path in include {
        collect(root, path, &mut files)?;
    }
    Ok(files)
}

fn content_path(path: &Path, format: PackFormat) -> bool {
    path.starts_with("mods")
        || format == PackFormat::Mrpack
            && ["resourcepacks", "shaderpacks", "datapacks"]
                .iter()
                .any(|directory| path.starts_with(directory))
}

fn reference_candidate(path: &Path, format: PackFormat) -> bool {
    if format == PackFormat::Mmc || !content_path(path, format) {
        return false;
    }
    let extension = if path.starts_with("mods") {
        "jar"
    } else {
        "zip"
    };
    path.parent()
        .is_some_and(|parent| parent.components().count() == 1)
        && path
            .extension()
            .is_some_and(|value| value.eq_ignore_ascii_case(extension))
}

fn inventory(
    root: PathBuf,
    options: ExportOptions,
    lock: Option<Arc<super::runtime::InstanceLock>>,
    progress: &ProgressTaskHandle,
) -> ExportResult<ExportPlan> {
    validate_options(&options)?;
    let root = root.canonicalize()?;
    if !root.is_dir() {
        return Err("Game folder must be a directory".into());
    }
    let selected = selected_files(&root, &options.include)?;
    let manifest_path = root
        .parent()
        .map(|parent| crate::storage::InstancePaths::new(parent).content_manifest());
    let cached = manifest_path
        .as_deref()
        .map(load_fingerprints)
        .unwrap_or_default();
    let mut files = BTreeMap::new();
    for (index, path) in selected.iter().enumerate() {
        progress.set_sub_action(path.display().to_string());
        progress.set_progress(index as u64, selected.len() as u64);
        let full = root.join(path);
        let hash = if reference_candidate(path, options.format) {
            export_fingerprint(
                &full,
                cached.record(path).map(|record| &record.fingerprint),
                || Ok(()),
            )?
        } else {
            fingerprint_metadata(&full)?
        };
        files.insert(path.clone(), hash);
    }
    Ok(ExportPlan {
        options,
        root,
        files,
        unmatched: Vec::new(),
        manifest_path,
        _lock: lock,
        explicit_selection: false,
        archive: ArchiveContents {
            manifests: Vec::new(),
            referenced: BTreeSet::new(),
            prefix: "overrides",
        },
    })
}

fn validate_options(options: &ExportOptions) -> ExportResult<()> {
    for (label, value) in [
        ("Pack name", &options.name),
        ("Pack version", &options.version),
        ("Minecraft version", &options.game_version),
    ] {
        if value.trim().is_empty() || value.chars().any(char::is_control) {
            return Err(format!("{label} must not be empty or contain control characters").into());
        }
    }
    if options.loader != ModLoader::Vanilla
        && options
            .loader_version
            .as_ref()
            .is_none_or(|v| v.trim().is_empty())
    {
        return Err("A loader version is required for a modded pack".into());
    }
    Ok(())
}

pub async fn prepare_export(root: &Path, options: &ExportOptions) -> ExportResult<ExportPlan> {
    prepare(root.to_owned(), options.clone(), None).await
}

pub async fn prepare_instance_export(
    manager: &InstanceManager,
    name: &str,
    options: &ExportOptions,
) -> ExportResult<ExportPlan> {
    let lock = Arc::new(super::runtime::lock_instance(&manager.instances_dir, name)?);
    let config = manager.load_one(name)?;
    let mut options = options.clone();
    options.game_version = config.game_version;
    options.loader = config.loader;
    options.loader_version = config.loader_version;
    let root = crate::storage::InstancePaths::new(manager.instances_dir.join(name)).minecraft();
    prepare(root, options, Some(lock)).await
}

async fn prepare(
    root: PathBuf,
    options: ExportOptions,
    lock: Option<Arc<super::runtime::InstanceLock>>,
) -> ExportResult<ExportPlan> {
    let progress = ProgressTask::start("Scanning export files");
    let handle = progress.handle();
    let mut plan =
        tokio::task::spawn_blocking(move || inventory(root, options, lock, &handle)).await??;
    progress.set_action(format!("Preparing {} export", plan.options.format.label()));
    plan.archive = match plan.options.format {
        PackFormat::CurseForge => curseforge::prepare(&plan, &progress).await?,
        PackFormat::Mrpack => mrpack::prepare(&plan, &progress).await?,
        PackFormat::Mmc => mmc::prepare(&plan.options)?,
    };
    if plan.options.format != PackFormat::Mmc {
        plan.unmatched = plan
            .files
            .keys()
            .filter(|path| {
                content_path(path, plan.options.format) && !plan.archive.referenced.contains(*path)
            })
            .cloned()
            .collect();
    }
    Ok(plan)
}

pub fn validate_output(root: &Path, output: &Path) -> ExportResult<()> {
    validate_output_path(root, output)?;
    if output.try_exists()? {
        return Err(format!("Output already exists: {}", output.display()).into());
    }
    Ok(())
}

pub fn validate_output_path(root: &Path, output: &Path) -> ExportResult<()> {
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    if parent.canonicalize()?.starts_with(root.canonicalize()?) {
        return Err("Export archive must be outside the game folder".into());
    }
    match std::fs::symlink_metadata(output) {
        Ok(metadata) if !metadata.is_file() => {
            return Err(format!("Output is not a regular file: {}", output.display()).into());
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
        _ => {}
    }
    Ok(())
}

pub async fn write_export(plan: ExportPlan, output: &Path) -> ExportResult<ExportReport> {
    write_export_with_overwrite(plan, output, false).await
}

pub async fn write_export_with_overwrite(
    plan: ExportPlan,
    output: &Path,
    overwrite: bool,
) -> ExportResult<ExportReport> {
    let output = output.to_owned();
    tokio::task::spawn_blocking(move || write_archive(plan, &output, overwrite)).await?
}

pub async fn export_pack(
    root: &Path,
    output: &Path,
    options: &ExportOptions,
) -> ExportResult<ExportReport> {
    validate_output(root, output)?;
    write_export(prepare_export(root, options).await?, output).await
}

fn write_archive(plan: ExportPlan, output: &Path, overwrite: bool) -> ExportResult<ExportReport> {
    if overwrite {
        validate_output_path(&plan.root, output)?;
    } else {
        validate_output(&plan.root, output)?;
    }
    if !plan.options.bundle_unmatched && !plan.unmatched.is_empty() {
        return Err(format!(
            "Content without {} matches requires explicit approval to bundle:\n{}",
            plan.options.format.label(),
            plan.unmatched
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("\n")
        )
        .into());
    }
    let progress = ProgressTask::start("Writing export archive");
    validate_inventory(&plan)?;
    let parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    {
        let mut zip = zip::ZipWriter::new(temporary.as_file_mut());
        let zip_options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in &plan.archive.manifests {
            zip.start_file(*name, zip_options)?;
            zip.write_all(bytes)?;
        }
        for (index, path) in plan.files.keys().enumerate() {
            progress.set_progress(index as u64, plan.files.len() as u64);
            progress.set_sub_action(path.display().to_string());
            if plan.archive.referenced.contains(path) {
                continue;
            }
            let name = path
                .to_str()
                .ok_or("Export paths must be UTF-8")?
                .replace('\\', "/");
            zip.start_file(format!("{}/{name}", plan.archive.prefix), zip_options)?;
            std::io::copy(&mut std::fs::File::open(plan.root.join(path))?, &mut zip)?;
        }
        zip.finish()?;
    }
    // Standalone folders can change while the archive is being compressed.
    validate_inventory(&plan)?;
    temporary.as_file().sync_all()?;
    if overwrite {
        validate_output_path(&plan.root, output)?;
        temporary.persist(output)?;
    } else {
        temporary.persist_noclobber(output)?;
    }
    Ok(ExportReport {
        output: output.to_owned(),
        referenced_count: plan.referenced_count(),
        bundled_count: plan.file_count() - plan.referenced_count(),
        unmatched: plan.unmatched,
    })
}

fn validate_inventory(plan: &ExportPlan) -> ExportResult<()> {
    let current = if plan.explicit_selection {
        let mut current = BTreeSet::new();
        for path in plan.files.keys() {
            collect(&plan.root, path, &mut current)?;
        }
        current
    } else {
        selected_files(&plan.root, &plan.options.include)?
    };
    if current != plan.files.keys().cloned().collect() {
        return Err("Export files changed after preparation; prepare the export again".into());
    }
    for (path, expected) in &plan.files {
        let actual = current_fingerprint(&plan.root.join(path), expected)?;
        if actual != *expected {
            if let Some(manifest_path) = &plan.manifest_path
                && manifest_path.try_exists()?
            {
                let lock = match &plan._lock {
                    Some(guard) => {
                        ContentLock::acquire_with_held_instance_lock(manifest_path, guard)
                    }
                    None => ContentLock::acquire(manifest_path),
                }?;
                let mut manifest = lock.load()?;
                if let Some(record) = manifest
                    .files
                    .iter_mut()
                    .find(|record| record.relative_path == *path && record.fingerprint == *expected)
                {
                    // Force a rescan without discarding ownership or the hashes used for safe mutations.
                    record.fingerprint.modified_ns = u128::MAX;
                    lock.save(&manifest)?;
                }
            }
            return Err(
                format!("Export file changed after preparation: {}", path.display()).into(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "tests/export.rs"]
mod tests;
