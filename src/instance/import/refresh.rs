// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::instance::{InstanceConfig, InstanceManager, ProviderProject};
use crate::net::modrinth::VersionInfo;
use crate::storage::InstancePaths;

use super::ImportSummary;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackState {
    pub source: ProviderProject,
    pub files: Vec<PathBuf>,
}

impl PackState {
    pub fn load(paths: &InstancePaths) -> Option<Self> {
        std::fs::read(paths.modpack_state())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
    }

    pub fn save(&self, paths: &InstancePaths) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|error| error.to_string())?;
        crate::storage::write_atomic(&paths.modpack_state(), &bytes)
            .map_err(|error| error.to_string())
    }
}

pub struct RefreshPlan {
    pub instance: InstanceConfig,
    pub summary: ImportSummary,
    pub current_version: String,
    pub target_version: String,
    pub conflicts: Vec<PathBuf>,
    stage_root: PathBuf,
    staged_instance: PathBuf,
    old_owned: HashSet<PathBuf>,
    new_owned: HashSet<PathBuf>,
}

impl Drop for RefreshPlan {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.stage_root);
    }
}

pub async fn prepare(
    manager: &InstanceManager,
    instance: &InstanceConfig,
    target: VersionInfo,
) -> Result<RefreshPlan, String> {
    if crate::instance::runtime::is_active(&instance.name) {
        return Err("Stop the instance before changing its modpack".to_owned());
    }
    let source = instance
        .modpack_source
        .clone()
        .ok_or_else(|| "This instance is not linked to a modpack provider".to_owned())?;
    let registry = crate::instance::content::provider::ProviderRegistry::configured(
        crate::net::HttpClient::new(),
    );
    let provider = registry
        .get(&source.provider)
        .ok_or_else(|| format!("{} content provider is unavailable", source.provider))?;
    let current = provider
        .version(&source.version_id)
        .await
        .map_err(|error| error.to_string())?;
    let live = manager.instances_dir.join(&instance.name);
    let needed = directory_size(&live)
        .saturating_add(target.files.iter().map(|file| file.size).sum::<u64>())
        .saturating_add(64 * 1024 * 1024);
    let available = fs2::available_space(&manager.instances_dir).map_err(|e| e.to_string())?;
    if available < needed {
        return Err(format!(
            "Not enough free space to stage this modpack change (need about {}, available {})",
            format_bytes(needed),
            format_bytes(available)
        ));
    }

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let stage_root = manager.instances_dir.join(format!(".rmcl-refresh-{nonce}"));
    let archives = stage_root.join("archives");
    let result = async {
        let summary = super::download_provider_summary(&source, &target, &archives.join("target"))
            .await
            .map_err(|error| error.to_string())?;
        let old_owned = match PackState::load(&InstancePaths::new(&live))
            .filter(|state| state.source == source)
        {
            Some(state) => state.files.into_iter().collect(),
            None => reconstruct_owned_files(&source, &current, &archives.join("current")).await?,
        };

        let staging_manager = InstanceManager::new(&stage_root, &manager.meta_dir);
        let staged_config = super::execute_import(&summary, &staging_manager)
            .await
            .map_err(|error| error.to_string())?;
        let imported = stage_root.join(&staged_config.name);
        let new_owned = PackState::load(&InstancePaths::new(&imported))
            .ok_or_else(|| "The staged pack did not record its owned files".to_owned())?
            .files
            .into_iter()
            .collect::<HashSet<_>>();
        let staged_instance = stage_root.join(&instance.name);
        if imported != staged_instance {
            std::fs::rename(&imported, &staged_instance).map_err(|error| error.to_string())?;
        }

        let mut config = instance.clone();
        config.game_version = summary.game_version.clone();
        config.loader = summary.loader;
        config.loader_version = summary.loader_version.clone();
        config.modpack_source = summary.source.clone();
        let staged_manager = InstanceManager::new(&stage_root, &manager.meta_dir);
        staged_manager
            .save(&config)
            .map_err(|error| error.to_string())?;

        let conflicts = user_file_collisions(
            &InstancePaths::new(&live).minecraft(),
            &old_owned,
            &new_owned,
        )?;
        Ok::<_, String>(RefreshPlan {
            instance: config,
            current_version: current.version_number,
            target_version: target.version_number.clone(),
            summary,
            conflicts,
            stage_root: stage_root.clone(),
            staged_instance,
            old_owned,
            new_owned,
        })
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&stage_root);
    }
    result
}

pub fn apply(
    plan: RefreshPlan,
    replace_conflicts: &HashSet<PathBuf>,
) -> Result<InstanceConfig, String> {
    let live = plan
        .stage_root
        .parent()
        .ok_or_else(|| "Invalid refresh staging path".to_owned())?
        .join(&plan.instance.name);
    let old_minecraft = InstancePaths::new(&live).minecraft();
    let staged_minecraft = InstancePaths::new(&plan.staged_instance).minecraft();
    preserve_user_files(
        &old_minecraft,
        &staged_minecraft,
        &old_minecraft,
        &plan.old_owned,
        &plan.new_owned,
        replace_conflicts,
    )?;

    let backup = live.with_file_name(format!(".{}.rmcl-backup", plan.instance.name));
    if backup.exists() {
        return Err(format!(
            "A previous update backup still exists at '{}'",
            backup.display()
        ));
    }
    std::fs::rename(&live, &backup).map_err(|error| error.to_string())?;
    if let Err(error) = std::fs::rename(&plan.staged_instance, &live) {
        let _ = std::fs::rename(&backup, &live);
        return Err(format!("Could not activate the staged update: {error}"));
    }
    if let Err(error) = std::fs::remove_dir_all(&backup) {
        tracing::warn!("Could not remove successful modpack update backup: {error}");
    }
    Ok(plan.instance.clone())
}

async fn reconstruct_owned_files(
    source: &ProviderProject,
    version: &VersionInfo,
    temporary_dir: &Path,
) -> Result<HashSet<PathBuf>, String> {
    let summary = super::download_provider_summary(source, version, temporary_dir)
        .await
        .map_err(|error| error.to_string())?;
    Ok(super::owned_files(&summary).await?.into_iter().collect())
}

pub fn recover_interrupted(instances_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(instances_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name.starts_with(".rmcl-refresh-") {
            let _ = std::fs::remove_dir_all(path);
            continue;
        }
        let Some(instance_name) = name
            .strip_prefix('.')
            .and_then(|name| name.strip_suffix(".rmcl-backup"))
        else {
            continue;
        };
        let live = instances_dir.join(instance_name);
        if live.join("instance.json").is_file() {
            let _ = std::fs::remove_dir_all(path);
        } else if let Err(error) = std::fs::rename(&path, &live) {
            tracing::warn!(
                "Could not restore interrupted modpack update backup '{}': {error}",
                path.display()
            );
        }
    }
}

fn user_file_collisions(
    minecraft: &Path,
    old_owned: &HashSet<PathBuf>,
    new_owned: &HashSet<PathBuf>,
) -> Result<Vec<PathBuf>, String> {
    let mut collisions = Vec::new();
    collect_files(minecraft, minecraft, &mut |relative, _| {
        if !old_owned.contains(relative) && new_owned.contains(relative) {
            collisions.push(relative.to_owned());
        }
        Ok(())
    })?;
    collisions.sort();
    Ok(collisions)
}

fn preserve_user_files(
    source: &Path,
    destination: &Path,
    root: &Path,
    old_owned: &HashSet<PathBuf>,
    new_owned: &HashSet<PathBuf>,
    replace_conflicts: &HashSet<PathBuf>,
) -> Result<(), String> {
    for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let relative = path.strip_prefix(root).map_err(|error| error.to_string())?;
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "Cannot safely preserve symbolic link '{}'",
                path.display()
            ));
        }
        if metadata.is_dir() {
            let target = destination.join(entry.file_name());
            std::fs::create_dir_all(&target).map_err(|error| error.to_string())?;
            preserve_user_files(
                &path,
                &target,
                root,
                old_owned,
                new_owned,
                replace_conflicts,
            )?;
        } else if !(old_owned.contains(relative)
            || new_owned.contains(relative) && replace_conflicts.contains(relative))
        {
            let target = destination.join(entry.file_name());
            std::fs::copy(&path, target).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn collect_files(
    directory: &Path,
    root: &Path,
    visit: &mut impl FnMut(&Path, &Path) -> Result<(), String>,
) -> Result<(), String> {
    if !directory.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.is_dir() {
            collect_files(&path, root, visit)?;
        } else if metadata.is_file() {
            visit(
                path.strip_prefix(root).map_err(|error| error.to_string())?,
                &path,
            )?;
        }
    }
    Ok(())
}

fn directory_size(path: &Path) -> u64 {
    let mut size = 0;
    let _ = collect_files(path, path, &mut |_, file| {
        size += file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        Ok(())
    });
    size
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GB", bytes as f64 / (1024 * 1024 * 1024) as f64)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024 * 1024) as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preservation_replaces_pack_files_and_keeps_user_files() {
        let temp = tempfile::tempdir().unwrap();
        let old = temp.path().join("old");
        let new = temp.path().join("new");
        std::fs::create_dir_all(old.join("mods")).unwrap();
        std::fs::create_dir_all(new.join("mods")).unwrap();
        std::fs::write(old.join("mods/pack.jar"), "old").unwrap();
        std::fs::write(old.join("mods/user.jar"), "user").unwrap();
        std::fs::write(new.join("mods/pack.jar"), "new").unwrap();
        std::fs::write(new.join("mods/user.jar"), "pack collision").unwrap();
        let owned = HashSet::from([PathBuf::from("mods/pack.jar")]);
        let target_owned = HashSet::from([
            PathBuf::from("mods/pack.jar"),
            PathBuf::from("mods/user.jar"),
        ]);

        preserve_user_files(&old, &new, &old, &owned, &target_owned, &HashSet::new()).unwrap();

        assert_eq!(
            std::fs::read_to_string(new.join("mods/pack.jar")).unwrap(),
            "new"
        );
        assert_eq!(
            std::fs::read_to_string(new.join("mods/user.jar")).unwrap(),
            "user"
        );

        std::fs::write(new.join("mods/user.jar"), "pack collision").unwrap();
        preserve_user_files(
            &old,
            &new,
            &old,
            &owned,
            &target_owned,
            &HashSet::from([PathBuf::from("mods/user.jar")]),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(new.join("mods/user.jar")).unwrap(),
            "pack collision"
        );
    }

    #[test]
    fn interrupted_swap_restores_the_backup_and_removes_staging() {
        let temp = tempfile::tempdir().unwrap();
        let backup = temp.path().join(".Pack.rmcl-backup");
        let staging = temp.path().join(".rmcl-refresh-1");
        std::fs::create_dir_all(&backup).unwrap();
        std::fs::write(backup.join("instance.json"), "{}").unwrap();
        std::fs::create_dir_all(&staging).unwrap();

        recover_interrupted(temp.path());

        assert!(temp.path().join("Pack/instance.json").is_file());
        assert!(!backup.exists());
        assert!(!staging.exists());
    }
}
