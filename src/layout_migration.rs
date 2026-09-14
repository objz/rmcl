// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::storage::{InstancePaths, LAYOUT_VERSION, MetadataPaths};

const LEGACY_MINECRAFT: &str = ".minecraft";
const LEGACY_STATE: &str = ".rmcl";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationProgress {
    pub phase: String,
    pub item: String,
    pub current: u64,
    pub total: u64,
    pub item_current: Option<u64>,
    pub item_total: Option<u64>,
    pub backup_dir: Option<PathBuf>,
}

impl MigrationProgress {
    fn new(phase: impl Into<String>, item: impl Into<String>, current: u64, total: u64) -> Self {
        Self {
            phase: phase.into(),
            item: item.into(),
            current,
            total,
            item_current: None,
            item_total: None,
            backup_dir: None,
        }
    }

    fn with_item_progress(mut self, current: u64, total: u64) -> Self {
        self.item_current = Some(current);
        self.item_total = Some(total);
        self
    }

    fn with_backup(mut self, backup_dir: &Path) -> Self {
        self.backup_dir = Some(backup_dir.to_owned());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MigrationJournal {
    version: u32,
    backup_dir: PathBuf,
    completed: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LayoutMarker {
    version: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Cannot migrate {instance}: both {old} and {new} exist")]
    PathConflict {
        instance: String,
        old: String,
        new: String,
    },
    #[error("Cannot merge migration data because both paths contain {path}")]
    MergeConflict { path: String },
    #[error("Migration backup would be created inside the data being backed up: {0}")]
    BackupOverlap(String),
    #[error(
        "Not enough free space for migration backup: need {required} bytes, have {available} bytes"
    )]
    InsufficientSpace { required: u64, available: u64 },
}

pub fn is_needed(instances_dir: &Path, meta_dir: &Path) -> bool {
    let metadata = MetadataPaths::new(meta_dir);
    if metadata.cache_rebuild_pending().exists() {
        return true;
    }
    if marker_version(&metadata.layout_marker()) == Some(LAYOUT_VERSION) {
        return false;
    }
    has_legacy_instances(instances_dir)
        || [
            "versions",
            "libraries",
            "assets",
            "loader-profiles",
            "config-sync",
        ]
        .iter()
        .any(|path| meta_dir.join(path).exists())
}

pub fn initialize_new_layout(meta_dir: &Path) -> Result<(), MigrationError> {
    let metadata = MetadataPaths::new(meta_dir);
    for directory in [
        metadata.profiles(),
        metadata.backups(),
        metadata.versions(),
        metadata.libraries(),
        metadata.assets(),
        metadata.loader_profiles(),
        metadata.provider_projects("modrinth"),
        metadata.provider_versions("modrinth"),
        metadata.provider_icons("modrinth"),
        metadata.temporary(),
    ] {
        fs::create_dir_all(directory)?;
    }
    write_json_atomic(
        &metadata.layout_marker(),
        &LayoutMarker {
            version: LAYOUT_VERSION,
        },
    )
}

pub fn run(
    instances_dir: &Path,
    meta_dir: &Path,
    config_file: &Path,
    mut report: impl FnMut(MigrationProgress),
) -> Result<PathBuf, MigrationError> {
    fs::create_dir_all(instances_dir)?;
    fs::create_dir_all(meta_dir)?;
    let metadata = MetadataPaths::new(meta_dir);
    if marker_version(&metadata.layout_marker()) == Some(LAYOUT_VERSION)
        && metadata.cache_rebuild_pending().exists()
    {
        crate::config::upgrade_config_file(config_file)?;
        return Ok(latest_layout_backup(&metadata).unwrap_or_else(|| metadata.backups()));
    }
    if !is_needed(instances_dir, meta_dir) {
        crate::config::upgrade_config_file(config_file)?;
        initialize_new_layout(meta_dir)?;
        return Ok(metadata.backups());
    }
    fs::create_dir_all(metadata.state())?;
    fs::write(metadata.cache_rebuild_pending(), LAYOUT_VERSION.to_string())?;

    let mut journal = load_or_create_journal(instances_dir, &metadata)?;
    let instances = instance_directories(instances_dir)?;
    let total = instances.len() as u64 + 8;
    let mut current = journal.completed.len() as u64;

    if !is_complete(&journal, "backup") {
        let backup_dir = journal.backup_dir.clone();
        backup_user_data(
            instances_dir,
            config_file,
            meta_dir,
            &backup_dir,
            |copied, bytes, path| {
                report(
                    MigrationProgress::new(
                        "Backing up user data",
                        path.display().to_string(),
                        current,
                        total,
                    )
                    .with_item_progress(copied, bytes)
                    .with_backup(&backup_dir),
                );
            },
        )?;
        complete(&mut journal, &metadata, "backup")?;
        current += 1;
    }

    if !is_complete(&journal, "config") {
        report(
            MigrationProgress::new(
                "Upgrading launcher config",
                config_file.display().to_string(),
                current,
                total,
            )
            .with_backup(&journal.backup_dir),
        );
        crate::config::upgrade_config_file(config_file)?;
        complete(&mut journal, &metadata, "config")?;
        current += 1;
    }

    validate_migration_conflicts(&instances, meta_dir)?;

    for instance in &instances {
        let name = instance
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("instance")
            .to_owned();
        let key = format!("instance:{name}");
        if is_complete(&journal, &key) {
            continue;
        }
        report(
            MigrationProgress::new("Migrating instances", name.clone(), current, total)
                .with_backup(&journal.backup_dir),
        );
        migrate_instance(instance, &name)?;
        complete(&mut journal, &metadata, &key)?;
        current += 1;
    }

    let moves = [
        (
            "profiles",
            meta_dir.join("config-sync").join("profiles"),
            metadata.profiles(),
        ),
        ("versions", meta_dir.join("versions"), metadata.versions()),
        (
            "libraries",
            meta_dir.join("libraries"),
            metadata.libraries(),
        ),
        ("assets", meta_dir.join("assets"), metadata.assets()),
        (
            "loader-profiles",
            meta_dir.join("loader-profiles"),
            metadata.loader_profiles(),
        ),
    ];
    for (key, source, destination) in moves {
        let journal_key = format!("shared:{key}");
        if is_complete(&journal, &journal_key) {
            continue;
        }
        report(
            MigrationProgress::new("Migrating shared data", key, current, total)
                .with_backup(&journal.backup_dir),
        );
        move_or_merge(&source, &destination)?;
        complete(&mut journal, &metadata, &journal_key)?;
        current += 1;
    }
    let legacy_config_sync = meta_dir.join("config-sync");
    if legacy_config_sync.exists() && fs::read_dir(&legacy_config_sync)?.next().is_none() {
        fs::remove_dir(legacy_config_sync)?;
    }

    report(
        MigrationProgress::new(
            "Finalizing migration",
            "Writing layout marker",
            total.saturating_sub(1),
            total,
        )
        .with_backup(&journal.backup_dir),
    );
    initialize_new_layout(meta_dir)?;
    if metadata.migration_journal().exists() {
        fs::remove_file(metadata.migration_journal())?;
    }
    report(
        MigrationProgress::new("Migration complete", "Layout updated", total, total)
            .with_backup(&journal.backup_dir),
    );
    Ok(journal.backup_dir)
}

pub fn cache_rebuild_pending(meta_dir: &Path) -> bool {
    MetadataPaths::new(meta_dir)
        .cache_rebuild_pending()
        .exists()
}

pub fn finish_cache_rebuild(meta_dir: &Path) -> Result<(), MigrationError> {
    let marker = MetadataPaths::new(meta_dir).cache_rebuild_pending();
    if marker.exists() {
        fs::remove_file(marker)?;
    }
    Ok(())
}

fn migrate_instance(instance: &Path, name: &str) -> Result<(), MigrationError> {
    let paths = InstancePaths::new(instance);
    rename_visible_directory(instance, LEGACY_MINECRAFT, paths.minecraft(), name)?;
    rename_visible_directory(instance, LEGACY_STATE, paths.state(), name)?;

    let old_config = paths.state().join("config-sync").join("local-config");
    move_or_merge(&old_config, &paths.local_config())?;
    let old_config_root = paths.state().join("config-sync");
    if old_config_root.exists() && fs::read_dir(&old_config_root)?.next().is_none() {
        fs::remove_dir(old_config_root)?;
    }
    fs::create_dir_all(paths.content())?;
    Ok(())
}

fn rename_visible_directory(
    instance: &Path,
    legacy_name: &str,
    destination: PathBuf,
    instance_name: &str,
) -> Result<(), MigrationError> {
    let source = instance.join(legacy_name);
    if source.exists() && destination.exists() {
        return Err(MigrationError::PathConflict {
            instance: instance_name.to_owned(),
            old: source.display().to_string(),
            new: destination.display().to_string(),
        });
    }
    if source.exists() {
        fs::rename(source, destination)?;
    }
    Ok(())
}

fn backup_user_data(
    instances_dir: &Path,
    config_file: &Path,
    meta_dir: &Path,
    backup_dir: &Path,
    mut report: impl FnMut(u64, u64, &Path),
) -> Result<(), MigrationError> {
    if backup_dir.exists() {
        return Ok(());
    }
    let legacy_profiles = meta_dir.join("config-sync").join("profiles");
    let current_profiles = MetadataPaths::new(meta_dir).profiles();
    let total = tree_size(instances_dir)?
        .saturating_add(file_size(config_file)?)
        .saturating_add(tree_size(&legacy_profiles)?)
        .saturating_add(tree_size(&current_profiles)?);
    validate_backup_destination(instances_dir, backup_dir, total)?;
    let partial = backup_dir.with_extension("partial");
    if partial.exists() {
        fs::remove_dir_all(&partial)?;
    }
    let mut copied = 0;
    report(copied, total, instances_dir);
    fs::create_dir_all(&partial)?;
    copy_dir_recursive_with_progress(
        instances_dir,
        &partial.join("instances"),
        &mut |bytes, path| {
            copied = copied.saturating_add(bytes);
            report(copied, total, path);
        },
    )?;
    if config_file.exists() {
        let destination = partial.join("config").join("config.toml");
        fs::create_dir_all(destination.parent().unwrap())?;
        let bytes = fs::copy(config_file, destination)?;
        copied = copied.saturating_add(bytes);
        report(copied, total, config_file);
    }
    if legacy_profiles.exists() {
        copy_dir_recursive_with_progress(
            &legacy_profiles,
            &partial.join("profiles/legacy"),
            &mut |bytes, path| {
                copied = copied.saturating_add(bytes);
                report(copied, total, path);
            },
        )?;
    }
    if current_profiles.exists() {
        copy_dir_recursive_with_progress(
            &current_profiles,
            &partial.join("profiles/current"),
            &mut |bytes, path| {
                copied = copied.saturating_add(bytes);
                report(copied, total, path);
            },
        )?;
    }
    fs::rename(partial, backup_dir)?;
    Ok(())
}

fn validate_migration_conflicts(
    instances: &[PathBuf],
    meta_dir: &Path,
) -> Result<(), MigrationError> {
    for instance in instances {
        let name = instance
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("instance");
        let paths = InstancePaths::new(instance);
        for (legacy, destination) in [
            (instance.join(LEGACY_MINECRAFT), paths.minecraft()),
            (instance.join(LEGACY_STATE), paths.state()),
        ] {
            if legacy.exists() && destination.exists() {
                return Err(MigrationError::PathConflict {
                    instance: name.to_owned(),
                    old: legacy.display().to_string(),
                    new: destination.display().to_string(),
                });
            }
        }
        validate_merge(
            &paths.state().join("config-sync").join("local-config"),
            &paths.local_config(),
        )?;
    }
    let metadata = MetadataPaths::new(meta_dir);
    for (source, destination) in [
        (
            meta_dir.join("config-sync").join("profiles"),
            metadata.profiles(),
        ),
        (meta_dir.join("versions"), metadata.versions()),
        (meta_dir.join("libraries"), metadata.libraries()),
        (meta_dir.join("assets"), metadata.assets()),
        (meta_dir.join("loader-profiles"), metadata.loader_profiles()),
    ] {
        validate_merge(&source, &destination)?;
    }
    Ok(())
}

fn validate_merge(source: &Path, destination: &Path) -> Result<(), MigrationError> {
    if !source.exists() || !destination.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if !target.exists() {
            continue;
        }
        if entry.file_type()?.is_dir() && target.is_dir() {
            validate_merge(&entry.path(), &target)?;
        } else {
            return Err(MigrationError::MergeConflict {
                path: target.display().to_string(),
            });
        }
    }
    Ok(())
}

fn validate_backup_destination(
    instances_dir: &Path,
    backup_dir: &Path,
    required: u64,
) -> Result<(), MigrationError> {
    let instances = instances_dir.canonicalize()?;
    let backup_parent = backup_dir
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "backup has no parent"))?;
    fs::create_dir_all(backup_parent)?;
    let backup_parent = backup_parent.canonicalize()?;
    let backup_name = backup_dir
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "backup has no name"))?;
    let canonical_backup = backup_parent.join(backup_name);
    if canonical_backup.starts_with(&instances) {
        return Err(MigrationError::BackupOverlap(
            canonical_backup.display().to_string(),
        ));
    }
    let available = fs2::available_space(&backup_parent)?;
    let margin = required / 20;
    let required_with_margin = required
        .saturating_add(margin)
        .saturating_add(16 * 1024 * 1024);
    if available < required_with_margin {
        return Err(MigrationError::InsufficientSpace {
            required: required_with_margin,
            available,
        });
    }
    Ok(())
}

fn tree_size(path: &Path) -> io::Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0_u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            total = total.saturating_add(tree_size(&entry.path())?);
        } else if file_type.is_file() {
            total = total.saturating_add(entry.metadata()?.len());
        }
    }
    Ok(total)
}

fn file_size(path: &Path) -> io::Result<u64> {
    if path.exists() {
        Ok(fs::metadata(path)?.len())
    } else {
        Ok(0)
    }
}

fn move_or_merge(source: &Path, destination: &Path) -> Result<(), MigrationError> {
    if !source.exists() {
        return Ok(());
    }
    if !destination.exists() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(source, destination)?;
        return Ok(());
    }
    merge_dir_without_overwrite(source, destination)?;
    fs::remove_dir_all(source)?;
    Ok(())
}

fn merge_dir_without_overwrite(source: &Path, destination: &Path) -> Result<(), MigrationError> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        let file_type = entry.file_type()?;
        if !target.exists() {
            fs::rename(entry.path(), target)?;
        } else if file_type.is_dir() && target.is_dir() {
            merge_dir_without_overwrite(&entry.path(), &target)?;
            fs::remove_dir(entry.path())?;
        } else {
            return Err(MigrationError::MergeConflict {
                path: target.display().to_string(),
            });
        }
    }
    Ok(())
}

fn copy_dir_recursive_with_progress(
    source: &Path,
    destination: &Path,
    report: &mut impl FnMut(u64, &Path),
) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if file_type.is_symlink() {
            copy_symlink(&entry.path(), &target)?;
        } else if file_type.is_dir() {
            copy_dir_recursive_with_progress(&entry.path(), &target, report)?;
        } else {
            let path = entry.path();
            let copied = fs::copy(&path, target)?;
            report(copied, &path);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn copy_symlink(source: &Path, destination: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(fs::read_link(source)?, destination)
}

#[cfg(windows)]
fn copy_symlink(source: &Path, destination: &Path) -> io::Result<()> {
    let target = fs::read_link(source)?;
    if source.is_dir() {
        std::os::windows::fs::symlink_dir(target, destination)
    } else {
        std::os::windows::fs::symlink_file(target, destination)
    }
}

fn load_or_create_journal(
    _instances_dir: &Path,
    metadata: &MetadataPaths,
) -> Result<MigrationJournal, MigrationError> {
    let legacy_journal = metadata.state().join("migration-v2.json");
    if metadata.migration_journal().exists() {
        return Ok(serde_json::from_slice(&fs::read(
            metadata.migration_journal(),
        )?)?);
    }
    if legacy_journal.exists() {
        let journal = serde_json::from_slice(&fs::read(&legacy_journal)?)?;
        write_json_atomic(&metadata.migration_journal(), &journal)?;
        fs::remove_file(legacy_journal)?;
        return Ok(journal);
    }
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let journal = MigrationJournal {
        version: LAYOUT_VERSION,
        backup_dir: metadata.backups().join(format!("backup-{timestamp}")),
        completed: Vec::new(),
    };
    write_json_atomic(&metadata.migration_journal(), &journal)?;
    Ok(journal)
}

fn latest_layout_backup(metadata: &MetadataPaths) -> Option<PathBuf> {
    let mut backups = fs::read_dir(metadata.backups())
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let file_type = entry.file_type().ok()?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            (file_type.is_dir() && (name.starts_with("backup-") || name.starts_with("layout-v2-")))
                .then(|| entry.path())
        })
        .collect::<Vec<_>>();
    backups.sort();
    backups.pop()
}

fn complete(
    journal: &mut MigrationJournal,
    metadata: &MetadataPaths,
    key: &str,
) -> Result<(), MigrationError> {
    journal.completed.push(key.to_owned());
    write_json_atomic(&metadata.migration_journal(), journal)
}

fn is_complete(journal: &MigrationJournal, key: &str) -> bool {
    journal.completed.iter().any(|completed| completed == key)
}

fn instance_directories(instances_dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut instances = fs::read_dir(instances_dir)?
        .flatten()
        .filter_map(|entry| entry.file_type().ok()?.is_dir().then(|| entry.path()))
        .collect::<Vec<_>>();
    instances.sort();
    Ok(instances)
}

fn has_legacy_instances(instances_dir: &Path) -> bool {
    instance_directories(instances_dir).is_ok_and(|instances| {
        instances.iter().any(|instance| {
            instance.join(LEGACY_MINECRAFT).exists() || instance.join(LEGACY_STATE).exists()
        })
    })
}

fn marker_version(path: &Path) -> Option<u32> {
    serde_json::from_slice::<LayoutMarker>(&fs::read(path).ok()?)
        .ok()
        .map(|marker| marker.version)
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<(), MigrationError> {
    crate::storage::write_atomic(path, &serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

#[cfg(test)]
#[path = "tests/layout_migration.rs"]
mod tests;
