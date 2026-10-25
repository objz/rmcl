// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::entry::ContentEntry;
use super::manifest::{
    ContentFileRecord, ContentLock, ContentManifest, FileFingerprint, fingerprint,
    fingerprint_metadata,
};

pub(crate) struct StagingDirectory {
    path: PathBuf,
    retained: bool,
}

impl StagingDirectory {
    pub(crate) fn new(minecraft_dir: &Path) -> io::Result<Self> {
        std::fs::create_dir_all(minecraft_dir)?;
        Ok(Self {
            path: tempfile::Builder::new()
                .prefix(".rmcl-install-")
                .tempdir_in(minecraft_dir)?
                .keep(),
            retained: false,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        if !self.retained
            && !std::thread::panicking()
            && let Err(error) = std::fs::remove_dir_all(&self.path)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(
                "Could not remove content staging '{}': {error}",
                self.path.display()
            );
        }
    }
}

pub(crate) struct FileTransaction {
    staging: StagingDirectory,
    moves: Vec<(PathBuf, PathBuf)>,
}

impl FileTransaction {
    pub(crate) fn new(staging: StagingDirectory) -> Self {
        Self {
            staging,
            moves: Vec::new(),
        }
    }

    pub(crate) fn staging(&self) -> &Path {
        self.staging.path()
    }

    pub(crate) fn move_path(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        require_absent(to)?;
        std::fs::rename(from, to)?;
        self.moves.push((from.to_owned(), to.to_owned()));
        Ok(())
    }

    pub(crate) fn finish<T, E: From<io::Error> + std::fmt::Display>(
        &mut self,
        result: Result<T, E>,
    ) -> Result<T, E> {
        match result {
            Ok(value) => {
                self.moves.clear();
                Ok(value)
            }
            Err(error) => match self.rollback() {
                Ok(()) => Err(error),
                Err(rollback) => Err(io::Error::other(format!("{error}; {rollback}")).into()),
            },
        }
    }

    fn rollback(&mut self) -> io::Result<()> {
        let mut errors = Vec::new();
        for (from, to) in self.moves.iter().rev() {
            if let Err(error) = require_absent(from).and_then(|()| std::fs::rename(to, from)) {
                errors.push(format!(
                    "restore '{}' from '{}': {error}",
                    from.display(),
                    to.display()
                ));
            }
        }
        self.moves.clear();
        if errors.is_empty() {
            Ok(())
        } else {
            self.staging.retained = true;
            Err(io::Error::other(format!(
                "Content rollback failed: {}; recovery staging retained at '{}'",
                errors.join("; "),
                self.staging.path.display()
            )))
        }
    }
}

impl Drop for FileTransaction {
    fn drop(&mut self) {
        if !self.moves.is_empty()
            && let Err(error) = self.rollback()
        {
            tracing::error!("{error}");
        }
    }
}

pub(crate) fn require_absent(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("'{}' already exists", path.display()),
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) fn relative_path(minecraft_dir: &Path, path: &Path) -> io::Result<PathBuf> {
    let relative = path.strip_prefix(minecraft_dir).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("'{}' is outside the Minecraft directory", path.display()),
        )
    })?;
    if !crate::storage::safe_relative_path(relative) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Invalid content path '{}'", path.display()),
        ));
    }
    let root = minecraft_dir.canonicalize()?;
    let mut parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Content path has no parent"))?;
    loop {
        match parent.canonicalize() {
            Ok(parent) if parent.starts_with(&root) => break,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "Content path '{}' escapes through a symlink",
                        path.display()
                    ),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                parent = parent.parent().ok_or(error)?;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(relative.to_owned())
}

fn mutation_lock(manifest_path: &Path, relative: &Path) -> io::Result<ContentLock> {
    if relative.starts_with("saves") && relative.components().count() <= 2 {
        ContentLock::acquire_stopped(manifest_path)
    } else {
        ContentLock::acquire(manifest_path)
    }
    .map_err(io::Error::other)
}

pub(crate) fn validate_record(minecraft_dir: &Path, record: &ContentFileRecord) -> io::Result<()> {
    let path = minecraft_dir.join(&record.relative_path);
    relative_path(minecraft_dir, &path)?;
    if current_fingerprint(&path, &record.fingerprint)? != record.fingerprint {
        return Err(io::Error::other(format!(
            "Content '{}' changed; refresh before retrying",
            path.display()
        )));
    }
    Ok(())
}

pub(crate) fn current_fingerprint(
    path: &Path,
    expected: &FileFingerprint,
) -> io::Result<FileFingerprint> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Content '{}' is a symlink", path.display()),
        ));
    }
    if metadata.is_dir() {
        directory_fingerprint_metadata(path)
    } else if metadata.is_file() && expected.hashes.is_empty() {
        fingerprint_metadata(path)
    } else if metadata.is_file() {
        fingerprint(path)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Content '{}' is not a file or directory", path.display()),
        ))
    }
}

pub(crate) fn directory_fingerprint_metadata(path: &Path) -> io::Result<FileFingerprint> {
    fn accumulate(path: &Path, size: &mut u64, modified_ns: &mut u128) -> io::Result<()> {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let metadata = std::fs::symlink_metadata(entry.path())?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            let modified = metadata
                .modified()
                .unwrap_or(SystemTime::UNIX_EPOCH)
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            *modified_ns = (*modified_ns).max(modified);
            if metadata.is_dir() {
                accumulate(&entry.path(), size, modified_ns)?;
            } else if metadata.is_file() {
                *size = size.saturating_add(metadata.len());
            }
        }
        Ok(())
    }
    let mut size = 0;
    let mut modified_ns = 0;
    accumulate(path, &mut size, &mut modified_ns)?;
    Ok(FileFingerprint {
        size,
        modified_ns,
        hashes: Default::default(),
    })
}

pub(crate) fn toggle(entry: &ContentEntry) -> io::Result<Option<PathBuf>> {
    let Some(name) = entry.path.file_name().and_then(|name| name.to_str()) else {
        return Ok(None);
    };
    let disabled = name.ends_with(".disabled");
    if disabled == entry.enabled {
        return Err(io::Error::other(format!(
            "Enabled state of '{}' changed; refresh before retrying",
            entry.path.display()
        )));
    }
    let new_name = if entry.enabled {
        format!("{name}.disabled")
    } else {
        name.trim_end_matches(".disabled").to_owned()
    };
    let new_path = entry.path.with_file_name(new_name);
    let instance_minecraft = entry.path.ancestors().find(|path| {
        path.file_name()
            .is_some_and(|name| name == crate::storage::MINECRAFT_DIR_NAME)
            && entry.path.strip_prefix(path).is_ok_and(|relative| {
                relative.components().next().is_some_and(|component| {
                    matches!(component, std::path::Component::Normal(name)
                        if name == "mods" || name == "resourcepacks" || name == "shaderpacks" || name == "saves")
                })
            })
    });
    let minecraft_dir = instance_minecraft
        .or_else(|| entry.path.parent())
        .ok_or_else(|| io::Error::other("Content path has no parent"))?;
    let manifest_path = if let Some(minecraft) = instance_minecraft {
        crate::storage::InstancePaths::new(
            minecraft
                .parent()
                .ok_or_else(|| io::Error::other("Minecraft directory has no instance parent"))?,
        )
        .content_manifest()
    } else {
        minecraft_dir.join(".rmcl-content.json")
    };
    let relative = relative_path(minecraft_dir, &entry.path)?;
    let lock = mutation_lock(&manifest_path, &relative)?;
    let mut manifest = if instance_minecraft.is_some() {
        lock.load().map_err(io::Error::other)?
    } else {
        ContentManifest::default()
    };
    let new_relative = relative_path(minecraft_dir, &new_path)?;
    require_absent(&new_path)?;
    if manifest.record(&new_relative).is_some() {
        return Err(io::Error::other(format!(
            "Content manifest already owns '{}'",
            new_path.display()
        )));
    }
    let record = manifest.record(&relative).cloned();
    let previous = manifest.clone();
    if let Some(project) = &entry.provider_project
        && record
            .as_ref()
            .and_then(|record| record.project_for_provider(&project.provider, &project.project_id))
            != Some(project)
    {
        return Err(io::Error::other(format!(
            "Ownership of '{}' changed; refresh before retrying",
            entry.path.display()
        )));
    }
    if let Some(record) = &record {
        if record.enabled != entry.enabled {
            return Err(io::Error::other(format!(
                "Enabled state of '{}' changed; refresh before retrying",
                entry.path.display()
            )));
        }
        validate_record(minecraft_dir, record)?;
    } else {
        current_fingerprint(
            &entry.path,
            &FileFingerprint {
                size: 0,
                modified_ns: 0,
                hashes: Default::default(),
            },
        )?;
    }
    let mut transaction = FileTransaction::new(StagingDirectory::new(minecraft_dir)?);
    let result = (|| {
        transaction.move_path(&entry.path, &new_path)?;
        if let Some(mut record) = record {
            manifest.remove(&relative);
            record.relative_path = new_relative;
            record.enabled = !entry.enabled;
            manifest.upsert(record);
            lock.save_if_unchanged(&manifest, &previous)
                .map_err(io::Error::other)?;
        }
        Ok(Some(new_path))
    })();
    transaction.finish(result)
}

pub(crate) fn record_toggle(
    manifest: &mut ContentManifest,
    minecraft_dir: &Path,
    old_path: &Path,
    new_path: &Path,
    enabled: bool,
) -> io::Result<bool> {
    let old_relative = relative_path(minecraft_dir, old_path)?;
    let new_relative = relative_path(minecraft_dir, new_path)?;
    match std::fs::symlink_metadata(old_path) {
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    if let Some(record) = manifest.record(&new_relative) {
        if manifest.record(&old_relative).is_some() || record.enabled != enabled {
            return Ok(false);
        }
        validate_record(minecraft_dir, record)?;
        return Ok(true);
    }
    let Some(record) = manifest.record(&old_relative) else {
        return Ok(false);
    };
    let fresh = match current_fingerprint(new_path, &record.fingerprint) {
        Ok(fresh) => fresh,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    Ok(
        fresh == record.fingerprint
            && manifest.rename_record(&old_relative, &new_relative, enabled),
    )
}

pub(crate) fn remove(
    manifest_path: &Path,
    minecraft_dir: &Path,
    path: &Path,
    expected: &[ContentFileRecord],
    orphan_only: bool,
) -> io::Result<(ContentManifest, Vec<PathBuf>)> {
    let relative = relative_path(minecraft_dir, path)?;
    let lock = mutation_lock(manifest_path, &relative)?;
    let mut manifest = lock.load().map_err(io::Error::other)?;
    let previous = manifest.clone();
    if manifest
        .files
        .iter()
        .filter(|record| record.relative_path.starts_with(&relative))
        .ne(expected.iter())
    {
        return Err(io::Error::other(format!(
            "Ownership of '{}' changed; refresh before retrying",
            path.display()
        )));
    }
    if orphan_only && !manifest.orphaned_dependencies().contains(&relative) {
        return Err(io::Error::other(format!(
            "'{}' is no longer an unused dependency",
            path.display()
        )));
    }
    for record in expected {
        match validate_record(minecraft_dir, record) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    let mut transaction = FileTransaction::new(StagingDirectory::new(minecraft_dir)?);
    let result = (|| {
        let backup = transaction.staging().join("removed");
        match std::fs::symlink_metadata(path) {
            Ok(_) => transaction.move_path(path, &backup)?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        manifest
            .files
            .retain(|record| !record.relative_path.starts_with(&relative));
        let orphans = manifest
            .orphaned_dependencies()
            .into_iter()
            .map(|relative| minecraft_dir.join(relative))
            .collect();
        lock.save_if_unchanged(&manifest, &previous)
            .map_err(io::Error::other)?;
        Ok((manifest, orphans))
    })();
    transaction.finish(result)
}

#[cfg(test)]
#[path = "../tests/content/local.rs"]
mod tests;
