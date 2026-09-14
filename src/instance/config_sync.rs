// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigSyncError {
    #[error("Invalid config sync profile: {0}")]
    InvalidProfile(String),
    #[error("Cannot switch config profiles while '{instance}' is running")]
    InstanceRunning { instance: String },
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug)]
pub struct ConfigSyncLock {
    file: std::fs::File,
}

impl Drop for ConfigSyncLock {
    fn drop(&mut self) {
        if let Err(e) = self.file.unlock() {
            tracing::warn!("Failed to release config sync lock: {}", e);
        }
    }
}

pub fn prepare(
    profile: Option<&str>,
    meta_dir: &Path,
    minecraft_dir: &Path,
) -> Result<bool, ConfigSyncError> {
    let Some(profile) = profile.and_then(normalize_profile) else {
        return Ok(false);
    };
    validate_profile(profile)?;

    let profile_dir = profile_dir(meta_dir, profile);
    if !profile_dir.exists() {
        return Ok(false);
    }
    let _lock = acquire_lock(&profile_dir)?;

    if !profile_payload_exists(&profile_dir)? {
        sync_to_profile(minecraft_dir, &profile_dir)?;
    } else {
        sync_from_profile(&profile_dir, minecraft_dir)?;
    }

    Ok(true)
}

pub fn finish(
    profile: Option<&str>,
    meta_dir: &Path,
    minecraft_dir: &Path,
) -> Result<(), ConfigSyncError> {
    let Some(profile) = profile.and_then(normalize_profile) else {
        return Ok(());
    };
    validate_profile(profile)?;

    let profile_dir = profile_dir(meta_dir, profile);
    let _lock = acquire_lock(&profile_dir)?;
    sync_to_profile(minecraft_dir, &profile_dir)
}

pub fn list_profiles(meta_dir: &Path) -> Result<Vec<String>, ConfigSyncError> {
    let root = profiles_dir(meta_dir);
    let mut profiles = Vec::new();
    if !root.exists() {
        return Ok(profiles);
    }

    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if validate_profile(&name).is_ok() {
                profiles.push(name);
            }
        }
    }
    profiles.sort_unstable();
    Ok(profiles)
}

pub fn create_profile(meta_dir: &Path, profile: &str) -> Result<String, ConfigSyncError> {
    let Some(profile) = normalize_profile(profile) else {
        return Err(ConfigSyncError::InvalidProfile(profile.to_string()));
    };
    validate_profile(profile)?;
    std::fs::create_dir_all(profile_dir(meta_dir, profile))?;
    Ok(profile.to_string())
}

pub fn delete_profile(meta_dir: &Path, profile: &str) -> Result<(), ConfigSyncError> {
    validate_profile(profile)?;
    let dir = profile_dir(meta_dir, profile);
    if dir.exists() {
        remove_path(&dir)?;
    }
    Ok(())
}

pub fn switch_profile(
    instance_name: &str,
    current_profile: Option<&str>,
    target_profile: Option<&str>,
    meta_dir: &Path,
    instance_dir: &Path,
) -> Result<Option<String>, ConfigSyncError> {
    if crate::instance::runtime::is_active(instance_name) {
        return Err(ConfigSyncError::InstanceRunning {
            instance: instance_name.to_string(),
        });
    }

    let current_profile = current_profile.and_then(normalize_profile);
    let target_profile = target_profile.and_then(normalize_profile);
    if let Some(profile) = current_profile {
        validate_profile(profile)?;
    }
    if let Some(profile) = target_profile {
        validate_profile(profile)?;
    }

    if current_profile == target_profile {
        return Ok(current_profile.map(str::to_string));
    }

    if let Some(profile) = current_profile {
        let profile_dir = profile_dir(meta_dir, profile);
        if profile_dir.exists() {
            let _lock = acquire_lock(&profile_dir)?;
            sync_to_profile(&minecraft_dir(instance_dir), &profile_dir)?;
        }
    }

    match (current_profile, target_profile) {
        (None, Some(_)) => {
            sync_to_profile(
                &minecraft_dir(instance_dir),
                &local_backup_dir(instance_dir),
            )?;
        }
        (Some(_), None) => {
            let backup = local_backup_dir(instance_dir);
            if backup.exists() {
                sync_from_profile(&backup, &minecraft_dir(instance_dir))?;
            }
            return Ok(None);
        }
        _ => {}
    }

    let Some(profile) = target_profile else {
        return Ok(None);
    };

    let profile_dir = profile_dir(meta_dir, profile);
    let _lock = acquire_lock(&profile_dir)?;

    if !profile_payload_exists(&profile_dir)? {
        sync_to_profile(&minecraft_dir(instance_dir), &profile_dir)?;
    }
    sync_from_profile(&profile_dir, &minecraft_dir(instance_dir))?;

    Ok(Some(profile.to_string()))
}

fn normalize_profile(profile: &str) -> Option<&str> {
    let trimmed = profile.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

pub fn validate_profile(profile: &str) -> Result<(), ConfigSyncError> {
    if profile.is_empty()
        || profile.len() > 64
        || profile.starts_with('.')
        || profile.contains('/')
        || profile.contains('\\')
        || profile.eq_ignore_ascii_case("default")
        || profile.eq_ignore_ascii_case("none")
        || profile.eq_ignore_ascii_case("local")
        || profile.eq_ignore_ascii_case("instance default")
        || profile.eq_ignore_ascii_case("local default")
        || profile
            .chars()
            .any(|c| c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
    {
        return Err(ConfigSyncError::InvalidProfile(profile.to_string()));
    }
    Ok(())
}

fn profile_dir(meta_dir: &Path, profile: &str) -> PathBuf {
    profiles_dir(meta_dir).join(profile)
}

fn profiles_dir(meta_dir: &Path) -> PathBuf {
    crate::storage::MetadataPaths::new(meta_dir).profiles()
}

fn minecraft_dir(instance_dir: &Path) -> PathBuf {
    instance_dir.join(crate::storage::MINECRAFT_DIR_NAME)
}

fn local_backup_dir(instance_dir: &Path) -> PathBuf {
    crate::storage::InstancePaths::new(instance_dir).local_config()
}

fn acquire_lock(profile_dir: &Path) -> Result<ConfigSyncLock, ConfigSyncError> {
    std::fs::create_dir_all(profile_dir)?;
    let path = profile_dir.join(".lock");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    file.lock()?;
    Ok(ConfigSyncLock { file })
}

fn mirror_dir(src: &Path, dst: &Path) -> Result<(), ConfigSyncError> {
    // stage the copy in a sibling temp dir and swap it in only when complete.
    // deleting dst up-front (the old behaviour) meant a failure mid-copy
    // destroyed the current config tree with nothing to fall back to.
    let parent = dst.parent().filter(|p| !p.as_os_str().is_empty());
    let Some(parent) = parent else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("no parent directory for {}", dst.display()),
        )
        .into());
    };
    std::fs::create_dir_all(parent)?;
    let name = dst.file_name().and_then(|s| s.to_str()).unwrap_or("mirror");
    let staging = parent.join(format!(".{name}.mirror-tmp"));
    if staging.exists() {
        remove_path(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;

    let result = (|| {
        if src.exists() {
            copy_dir_contents(src, &staging)?;
        }
        if dst.exists() {
            remove_path(dst)?;
        }
        std::fs::rename(&staging, dst)?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

fn sync_to_profile(minecraft_dir: &Path, profile_dir: &Path) -> Result<(), ConfigSyncError> {
    mirror_dir(&minecraft_dir.join("config"), &profile_dir.join("config"))?;
    mirror_options(minecraft_dir, profile_dir)
}

fn sync_from_profile(profile_dir: &Path, minecraft_dir: &Path) -> Result<(), ConfigSyncError> {
    mirror_dir(&profile_dir.join("config"), &minecraft_dir.join("config"))?;
    mirror_options(profile_dir, minecraft_dir)
}

fn profile_payload_exists(profile_dir: &Path) -> Result<bool, ConfigSyncError> {
    if profile_dir.join("config").exists() {
        return Ok(true);
    }
    if !profile_dir.exists() {
        return Ok(false);
    }
    for entry in std::fs::read_dir(profile_dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && is_options_file(&entry.file_name().to_string_lossy()) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn mirror_options(src: &Path, dst: &Path) -> Result<(), ConfigSyncError> {
    remove_options(dst)?;
    std::fs::create_dir_all(dst)?;
    if !src.exists() {
        return Ok(());
    }

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if entry.file_type()?.is_file() && is_options_file(&name.to_string_lossy()) {
            std::fs::copy(entry.path(), dst.join(name))?;
        }
    }
    Ok(())
}

fn remove_options(dir: &Path) -> Result<(), ConfigSyncError> {
    if !dir.exists() {
        return Ok(());
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if is_options_file(&entry.file_name().to_string_lossy()) {
            remove_path(&entry.path())?;
        }
    }
    Ok(())
}

fn is_options_file(name: &str) -> bool {
    name == "options.txt" || name.starts_with("options") && name.ends_with(".txt")
}

fn remove_path(path: &Path) -> Result<(), ConfigSyncError> {
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn copy_dir_contents(src: &Path, dst: &Path) -> Result<(), ConfigSyncError> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let source = entry.path();
        let target = dst.join(entry.file_name());
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            std::fs::create_dir_all(&target)?;
            copy_dir_contents(&source, &target)?;
        } else if file_type.is_file() {
            std::fs::copy(&source, &target)?;
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "tests/config_sync.rs"]
mod tests;
