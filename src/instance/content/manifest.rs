// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha1::Digest as _;

const MANIFEST_VERSION: u32 = 1;

pub(crate) struct ContentLock {
    file: File,
    _instance_lock: Option<crate::instance::runtime::InstanceLock>,
    path: PathBuf,
}

impl ContentLock {
    pub(crate) fn acquire(path: &Path) -> Result<Self, ManifestError> {
        Self::acquire_with_instance_guard(path, false)
    }

    pub(crate) fn acquire_stopped(path: &Path) -> Result<Self, ManifestError> {
        Self::acquire_with_instance_guard(path, true)
    }

    fn acquire_with_instance_guard(
        path: &Path,
        require_stopped: bool,
    ) -> Result<Self, ManifestError> {
        let parent = path.parent().ok_or_else(|| {
            ManifestError::InvalidPath(format!("{} has no parent", path.display()))
        })?;
        let instance_dir = parent
            .parent()
            .filter(|state| {
                path.file_name().is_some_and(|name| name == "manifest.json")
                    && parent.file_name().is_some_and(|name| name == "content")
                    && state
                        .file_name()
                        .is_some_and(|name| name == crate::storage::INSTANCE_STATE_DIR_NAME)
            })
            .and_then(Path::parent);
        let instance_lock = if let Some(instance_dir) = instance_dir {
            let root = instance_dir.parent().ok_or_else(|| {
                ManifestError::InvalidPath(format!("{} has no instance root", path.display()))
            })?;
            let name = instance_dir
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    ManifestError::InvalidPath(format!("{} has no instance name", path.display()))
                })?;
            let lock = if require_stopped {
                crate::instance::runtime::lock_instance(root, name)
            } else {
                crate::instance::runtime::lock_content(root, name)
            };
            Some(lock.map_err(|error| match error {
                crate::instance::manager::InstanceError::InstanceRunning(_) => {
                    ManifestError::Busy(path.to_owned())
                }
                error => ManifestError::Io(std::io::Error::other(error)),
            })?)
        } else {
            None
        };
        Self::acquire_file(path, instance_lock)
    }

    pub(crate) fn acquire_with_held_instance_lock(
        path: &Path,
        _guard: &crate::instance::runtime::InstanceLock,
    ) -> Result<Self, ManifestError> {
        Self::acquire_file(path, None)
    }

    fn acquire_file(
        path: &Path,
        instance_lock: Option<crate::instance::runtime::InstanceLock>,
    ) -> Result<Self, ManifestError> {
        let parent = path.parent().ok_or_else(|| {
            ManifestError::InvalidPath(format!("{} has no parent", path.display()))
        })?;
        std::fs::create_dir_all(parent)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path.with_extension("lock"))?;
        if let Err(error) = fs2::FileExt::try_lock_exclusive(&file) {
            if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
                return Err(ManifestError::Busy(path.to_owned()));
            }
            return Err(error.into());
        }
        Ok(Self {
            file,
            _instance_lock: instance_lock,
            path: path.to_owned(),
        })
    }

    pub(crate) fn load(&self) -> Result<ContentManifest, ManifestError> {
        ContentManifest::load(&self.path)
    }

    pub(crate) fn save(&self, manifest: &ContentManifest) -> Result<(), ManifestError> {
        let bytes = serde_json::to_vec_pretty(manifest)?;
        crate::storage::write_atomic(&self.path, &bytes)?;
        Ok(())
    }

    pub(crate) fn save_if_unchanged(
        &self,
        manifest: &ContentManifest,
        previous: &ContentManifest,
    ) -> Result<(), ManifestError> {
        let current = self.load()?;
        if current.version != previous.version || current.files != previous.files {
            return Err(ManifestError::Changed(self.path.clone()));
        }
        if manifest.version != previous.version || manifest.files != previous.files {
            self.save(manifest)?;
        }
        Ok(())
    }
}

impl Drop for ContentLock {
    fn drop(&mut self) {
        if let Err(error) = fs2::FileExt::unlock(&self.file) {
            tracing::warn!(
                "Could not release content lock '{}': {error}",
                self.path.display()
            );
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Mod,
    ResourcePack,
    Shader,
    DataPack,
}

impl ContentKind {
    pub fn directory(self) -> &'static str {
        match self {
            Self::Mod => "mods",
            Self::ResourcePack => "resourcepacks",
            Self::Shader => "shaderpacks",
            Self::DataPack => "datapacks",
        }
    }

    pub fn unavailable_message(self, loader: crate::instance::ModLoader) -> Option<&'static str> {
        if loader != crate::instance::ModLoader::Vanilla {
            return None;
        }
        match self {
            Self::Mod => Some("Vanilla does not support mods."),
            Self::Shader => Some("Vanilla does not support shaders."),
            Self::ResourcePack | Self::DataPack => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderProject {
    pub provider: String,
    pub project_id: String,
    pub version_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Resolution {
    #[default]
    Pending,
    Resolved {
        project: ProviderProject,
    },
    Unmatched {
        checked_at: i64,
        providers: Vec<String>,
    },
    Ambiguous {
        candidates: Vec<ProviderProject>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFingerprint {
    pub size: u64,
    pub modified_ns: u128,
    pub hashes: BTreeMap<String, String>,
}

impl FileFingerprint {
    pub fn hash(&self, algorithm: &str) -> Option<&str> {
        self.hashes.get(algorithm).map(String::as_str)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentFileRecord {
    pub relative_path: PathBuf,
    pub kind: ContentKind,
    pub enabled: bool,
    pub fingerprint: FileFingerprint,
    #[serde(default)]
    pub resolution: Resolution,
    #[serde(default)]
    pub provider_aliases: Vec<ProviderProject>,
    #[serde(default)]
    pub provider_checks: Vec<String>,
    #[serde(default)]
    pub required_dependencies: Vec<ProviderProject>,
    #[serde(default)]
    pub automatic_dependency: bool,
    #[serde(default)]
    pub cleanup_eligible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentManifest {
    pub version: u32,
    #[serde(default)]
    pub files: Vec<ContentFileRecord>,
}

impl Default for ContentManifest {
    fn default() -> Self {
        Self {
            version: MANIFEST_VERSION,
            files: Vec::new(),
        }
    }
}

impl ContentManifest {
    pub fn load(path: &Path) -> Result<Self, ManifestError> {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error.into()),
        };
        let mut manifest: Self = serde_json::from_slice(&bytes)?;
        if manifest.version != MANIFEST_VERSION {
            return Err(ManifestError::UnsupportedVersion(manifest.version));
        }
        // record()/upsert() rely on files being sorted by relative_path;
        // normalize here so hand-edited manifests can't break the lookups.
        manifest
            .files
            .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        for (index, record) in manifest.files.iter().enumerate() {
            if !crate::storage::safe_relative_path(&record.relative_path)
                || index > 0 && manifest.files[index - 1].relative_path == record.relative_path
            {
                return Err(ManifestError::InvalidPath(format!(
                    "Unsafe or duplicate content path '{}'",
                    record.relative_path.display()
                )));
            }
        }
        Ok(manifest)
    }

    pub fn save(&self, path: &Path) -> Result<(), ManifestError> {
        ContentLock::acquire(path)?.save(self)
    }

    pub fn update<T>(
        path: &Path,
        update: impl FnOnce(&mut Self) -> Result<T, ManifestError>,
    ) -> Result<T, ManifestError> {
        let lock = ContentLock::acquire(path)?;
        let mut manifest = lock.load()?;
        let previous = manifest.clone();
        let result = update(&mut manifest)?;
        lock.save_if_unchanged(&manifest, &previous)?;
        Ok(result)
    }

    // binary search: files is kept sorted by relative_path (see load/upsert),
    // so per-file lookups stay O(log n) even for large manifests.
    pub fn record(&self, relative_path: &Path) -> Option<&ContentFileRecord> {
        let index = self
            .files
            .partition_point(|record| record.relative_path < relative_path);
        self.files
            .get(index)
            .filter(|record| record.relative_path == relative_path)
    }

    pub fn upsert(&mut self, record: ContentFileRecord) {
        let index = self
            .files
            .partition_point(|existing| existing.relative_path < record.relative_path);
        if self
            .files
            .get(index)
            .is_some_and(|existing| existing.relative_path == record.relative_path)
        {
            self.files[index] = record;
        } else {
            // insert at the sorted position instead of re-sorting the whole
            // vec; bulk callers that push pre-sorted records get O(1) each.
            self.files.insert(index, record);
        }
    }

    pub fn remove(&mut self, relative_path: &Path) {
        self.files
            .retain(|record| record.relative_path != relative_path);
    }

    pub fn rename_record(&mut self, from: &Path, to: &Path, enabled: bool) -> bool {
        if from != to && self.record(to).is_some() {
            return false;
        }
        let Some(index) = self
            .files
            .iter()
            .position(|record| record.relative_path == from)
        else {
            return false;
        };
        let mut record = self.files.remove(index);
        record.relative_path = to.to_owned();
        record.enabled = enabled;
        self.upsert(record);
        true
    }

    pub fn resolved_project_path(
        &self,
        provider: &str,
        project_id: &str,
        minecraft_dir: &Path,
    ) -> Option<PathBuf> {
        self.files.iter().find_map(|record| {
            record
                .matches_project(provider, project_id)
                .then(|| minecraft_dir.join(&record.relative_path))
        })
    }

    pub fn resolved_project_path_under(
        &self,
        provider: &str,
        project_id: &str,
        minecraft_dir: &Path,
        directory: &Path,
    ) -> Option<PathBuf> {
        self.files.iter().find_map(|record| {
            let path = minecraft_dir.join(&record.relative_path);
            (record.matches_project(provider, project_id) && path.starts_with(directory))
                .then_some(path)
        })
    }

    pub fn resolved_project_record(
        &self,
        provider: &str,
        project_id: &str,
    ) -> Option<&ContentFileRecord> {
        self.files
            .iter()
            .find(|record| record.matches_project(provider, project_id))
    }

    pub fn dependent_paths(&self, relative_path: &Path) -> Vec<PathBuf> {
        let Some(target) = self.record(relative_path) else {
            return Vec::new();
        };
        self.files
            .iter()
            .filter(|record| record.relative_path != relative_path)
            .filter(|record| dependency_applies(target, record))
            .map(|record| record.relative_path.clone())
            .collect()
    }

    pub fn orphaned_dependencies_after_removing(&self, relative_path: &Path) -> Vec<PathBuf> {
        self.orphaned_dependencies_with_removed(std::iter::once(relative_path.to_owned()))
    }

    pub fn orphaned_dependencies(&self) -> Vec<PathBuf> {
        self.orphaned_dependencies_with_removed(std::iter::empty())
    }

    fn orphaned_dependencies_with_removed(
        &self,
        removed: impl IntoIterator<Item = PathBuf>,
    ) -> Vec<PathBuf> {
        let mut removed = removed
            .into_iter()
            .collect::<std::collections::HashSet<_>>();
        let mut orphans = Vec::new();
        loop {
            let mut found = false;
            for candidate in &self.files {
                if !candidate.automatic_dependency
                    || !candidate.cleanup_eligible
                    || removed.contains(&candidate.relative_path)
                {
                    continue;
                }
                let still_required = self
                    .files
                    .iter()
                    .filter(|record| !removed.contains(&record.relative_path))
                    .any(|record| dependency_applies(candidate, record));
                if !still_required {
                    removed.insert(candidate.relative_path.clone());
                    orphans.push(candidate.relative_path.clone());
                    found = true;
                }
            }
            if !found {
                break;
            }
        }
        orphans
    }
}

fn dependency_applies(candidate: &ContentFileRecord, dependent: &ContentFileRecord) -> bool {
    let matches = dependent
        .required_dependencies
        .iter()
        .any(|dependency| candidate.matches_project(&dependency.provider, &dependency.project_id));
    if !matches || candidate.kind != ContentKind::DataPack {
        return matches;
    }
    datapack_world(&candidate.relative_path) == datapack_world(&dependent.relative_path)
}

fn datapack_world(path: &Path) -> Option<&std::ffi::OsStr> {
    let mut components = path.components();
    (components.next()?.as_os_str() == "saves")
        .then(|| components.next().map(std::path::Component::as_os_str))
        .flatten()
}

impl ContentFileRecord {
    pub fn resolved_project(&self) -> Option<&ProviderProject> {
        match &self.resolution {
            Resolution::Resolved { project } => Some(project),
            _ => None,
        }
    }

    pub fn matches_project(&self, provider: &str, project_id: &str) -> bool {
        self.resolved_project()
            .into_iter()
            .chain(self.provider_aliases.iter())
            .any(|project| project.provider == provider && project.project_id == project_id)
    }

    pub fn project_for_provider(
        &self,
        provider: &str,
        project_id: &str,
    ) -> Option<&ProviderProject> {
        self.resolved_project()
            .into_iter()
            .chain(self.provider_aliases.iter())
            .find(|project| project.provider == provider && project.project_id == project_id)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Unsupported content manifest version {0}")]
    UnsupportedVersion(u32),
    #[error("Invalid manifest path: {0}")]
    InvalidPath(String),
    #[error("Content is being changed by another operation: {0}")]
    Busy(PathBuf),
    #[error("Content manifest changed during the operation; refresh before retrying: {0}")]
    Changed(PathBuf),
}

pub fn fingerprint(path: &Path) -> Result<FileFingerprint, std::io::Error> {
    fingerprint_with_check(path, || Ok(()))
}

pub(crate) fn fingerprint_with_check(
    path: &Path,
    check: impl Fn() -> std::io::Result<()>,
) -> Result<FileFingerprint, std::io::Error> {
    check()?;
    let metadata = std::fs::metadata(path)?;
    let modified_ns = metadata
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut file = std::fs::File::open(path)?;
    let mut sha1 = sha1::Sha1::new();
    let mut sha512 = sha2::Sha512::new();
    let mut curseforge_len = 0u32;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        check()?;
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        sha1.update(&buffer[..read]);
        sha512.update(&buffer[..read]);
        curseforge_len = curseforge_len.saturating_add(
            buffer[..read]
                .iter()
                .filter(|byte| !matches!(byte, b'\t' | b'\n' | b'\r' | b' '))
                .count() as u32,
        );
    }
    file.rewind()?;
    let hashes = BTreeMap::from([
        ("sha1".to_owned(), format!("{:x}", sha1.finalize())),
        ("sha512".to_owned(), format!("{:x}", sha512.finalize())),
        (
            "curseforge".to_owned(),
            curseforge_fingerprint(&mut file, curseforge_len, &check)?.to_string(),
        ),
    ]);
    Ok(FileFingerprint {
        size: metadata.len(),
        modified_ns,
        hashes,
    })
}

fn curseforge_fingerprint(
    reader: &mut impl Read,
    length: u32,
    check: &impl Fn() -> std::io::Result<()>,
) -> std::io::Result<u32> {
    const M: u32 = 0x5bd1e995;
    let mut hash = 1 ^ length;
    let mut block = [0u8; 4];
    let mut block_len = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        check()?;
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        for &byte in &buffer[..read] {
            if matches!(byte, b'\t' | b'\n' | b'\r' | b' ') {
                continue;
            }
            block[block_len] = byte;
            block_len += 1;
            if block_len == 4 {
                let mut value = u32::from_le_bytes(block);
                value = value.wrapping_mul(M);
                value ^= value >> 24;
                value = value.wrapping_mul(M);
                hash = hash.wrapping_mul(M) ^ value;
                block_len = 0;
            }
        }
    }
    match block_len {
        3 => {
            hash ^= u32::from(block[2]) << 16;
            hash ^= u32::from(block[1]) << 8;
            hash ^= u32::from(block[0]);
            hash = hash.wrapping_mul(M);
        }
        2 => {
            hash ^= u32::from(block[1]) << 8;
            hash ^= u32::from(block[0]);
            hash = hash.wrapping_mul(M);
        }
        1 => {
            hash ^= u32::from(block[0]);
            hash = hash.wrapping_mul(M);
        }
        _ => {}
    }
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(M);
    hash ^= hash >> 15;
    Ok(hash)
}

pub fn fingerprint_metadata(path: &Path) -> Result<FileFingerprint, std::io::Error> {
    let metadata = std::fs::metadata(path)?;
    let modified_ns = metadata
        .modified()
        .unwrap_or(SystemTime::UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Ok(FileFingerprint {
        size: metadata.len(),
        modified_ns,
        hashes: BTreeMap::new(),
    })
}

#[cfg(test)]
#[path = "../tests/content/manifest.rs"]
mod tests;
