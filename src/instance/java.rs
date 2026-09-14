// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// java runtime discovery shared by launching, loader installation, and settings.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JavaInstallation {
    pub path: PathBuf,
    pub version: Option<String>,
}

#[must_use]
pub fn detect_java_path() -> String {
    if let Ok(java_home) = std::env::var("JAVA_HOME") {
        let java_name = if cfg!(windows) { "java.exe" } else { "java" };
        let bin = std::path::Path::new(&java_home).join("bin").join(java_name);
        if bin.exists() {
            tracing::trace!("Detected Java from JAVA_HOME: {}", bin.display());
            return bin.to_string_lossy().to_string();
        }
        tracing::warn!(
            "JAVA_HOME is set to {}, but {} does not exist",
            java_home,
            bin.display()
        );
    }
    match which::which("java") {
        Ok(path) => {
            tracing::trace!("Detected Java from PATH: {}", path.display());
            path.to_string_lossy().to_string()
        }
        Err(e) => {
            tracing::warn!(
                "Could not find java on PATH, falling back to literal 'java': {}",
                e
            );
            "java".to_string()
        }
    }
}

/// Finds Java executables in the environment and the conventional installation
/// directories for the current platform. The search is deliberately bounded so
/// opening the selector never walks an entire drive.
#[must_use]
pub fn discover_installations() -> Vec<JavaInstallation> {
    let mut candidates = Vec::new();
    for variable in ["JAVA_HOME", "JDK_HOME"] {
        if let Ok(java_home) = std::env::var(variable) {
            add_java_home(Path::new(&java_home), &mut candidates);
        }
    }
    if let Ok(path) = which::which("java") {
        candidates.push(path);
    }
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            let executable = if cfg!(target_os = "windows") {
                "java.exe"
            } else {
                "java"
            };
            candidates.push(directory.join(executable));
        }
    }

    for root in java_roots() {
        collect_java_executables(&root, 3, &mut candidates);
    }

    let mut seen = HashSet::new();
    let mut installations = candidates
        .into_iter()
        .filter(|path| path.is_file())
        .filter_map(|path| {
            let identity = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            seen.insert(identity)
                .then(|| inspect_installation(&path))
                .flatten()
        })
        .collect::<Vec<_>>();
    installations.sort_by(|a, b| {
        java_major(b.version.as_deref())
            .cmp(&java_major(a.version.as_deref()))
            .then_with(|| a.path.cmp(&b.path))
    });
    installations
}

/// Reads the version reported by one Java executable.
#[must_use]
pub fn inspect_installation(path: &Path) -> Option<JavaInstallation> {
    path.is_file().then(|| JavaInstallation {
        version: java_version(path),
        path: path.to_path_buf(),
    })
}

#[must_use]
pub fn load_installation_cache(path: &Path) -> Option<Vec<JavaInstallation>> {
    let mut installations =
        serde_json::from_slice::<Vec<JavaInstallation>>(&std::fs::read(path).ok()?).ok()?;
    installations.retain(|installation| installation.path.is_file());
    (!installations.is_empty()).then_some(installations)
}

pub fn save_installation_cache(
    path: &Path,
    installations: &[JavaInstallation],
) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(installations).map_err(std::io::Error::other)?;
    crate::storage::write_atomic(path, &bytes)
}

fn java_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if cfg!(target_os = "windows") {
        for variable in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Some(root) = std::env::var_os(variable) {
                let root = PathBuf::from(root);
                roots.extend([
                    root.join("Java"),
                    root.join("Eclipse Adoptium"),
                    root.join("Programs/Eclipse Adoptium"),
                    root.join("Programs/Java"),
                    root.join("Microsoft"),
                    root.join("BellSoft"),
                    root.join("Zulu"),
                    root.join("Amazon Corretto"),
                ]);
            }
        }
    } else if cfg!(target_os = "macos") {
        roots.push(PathBuf::from("/Library/Java/JavaVirtualMachines"));
        for prefix in ["/opt/homebrew/opt", "/usr/local/opt"] {
            for formula in [
                "openjdk",
                "openjdk@8",
                "openjdk@11",
                "openjdk@17",
                "openjdk@21",
            ] {
                roots.push(PathBuf::from(prefix).join(formula));
            }
        }
        if let Some(home) = dirs_next::home_dir() {
            roots.push(home.join("Library/Java/JavaVirtualMachines"));
            roots.push(home.join(".sdkman/candidates/java"));
            roots.push(home.join(".asdf/installs/java"));
            roots.push(home.join(".local/share/mise/installs/java"));
        }
    } else {
        roots.extend([
            PathBuf::from("/usr/lib/jvm"),
            PathBuf::from("/usr/java"),
            PathBuf::from("/opt/java"),
            PathBuf::from("/opt/jdk"),
        ]);
        if let Some(home) = dirs_next::home_dir() {
            roots.push(home.join(".sdkman/candidates/java"));
            roots.push(home.join(".jdks"));
            roots.push(home.join(".asdf/installs/java"));
            roots.push(home.join(".local/share/mise/installs/java"));
        }
    }
    roots
}

fn collect_java_executables(directory: &Path, depth: usize, candidates: &mut Vec<PathBuf>) {
    if depth == 0 || !directory.is_dir() {
        return;
    }
    add_java_home(directory, candidates);
    if let Ok(entries) = std::fs::read_dir(directory) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_java_executables(&path, depth - 1, candidates);
            }
        }
    }
}

fn add_java_home(home: &Path, candidates: &mut Vec<PathBuf>) {
    let executable = if cfg!(target_os = "windows") {
        "java.exe"
    } else {
        "java"
    };
    for path in [
        home.join("bin").join(executable),
        home.join("Contents/Home/bin").join(executable),
    ] {
        if path.is_file() {
            candidates.push(path);
        }
    }
}

pub(crate) fn java_version(path: &Path) -> Option<String> {
    let output = Command::new(path).arg("-version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    parse_java_version(&text)
}

fn parse_java_version(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        line.split_once('"')
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(version, _)| version.to_owned())
    })
}

pub(crate) fn java_major(version: Option<&str>) -> u32 {
    let Some(version) = version else {
        return 0;
    };
    let parts = version
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<u32>().ok())
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [1, legacy_major, ..] => *legacy_major,
        [major, ..] => *major,
        [] => 0,
    }
}

pub(crate) fn parse_java_major_version(output: &str) -> Option<u32> {
    if let Some(version) = parse_java_version(output) {
        return Some(java_major(Some(&version))).filter(|major| *major > 0);
    }

    let start = output.find(|character: char| character.is_ascii_digit())?;
    let parts = output[start..]
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .filter_map(|part| part.parse::<u32>().ok())
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [1, legacy_major, ..] => Some(*legacy_major),
        [major, ..] => Some(*major),
        [] => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modern_and_legacy_java_versions() {
        assert_eq!(
            parse_java_version("openjdk version \"21.0.4\" 2024-07-16"),
            Some("21.0.4".to_owned())
        );
        assert_eq!(
            parse_java_version("java version \"1.8.0_412\""),
            Some("1.8.0_412".to_owned())
        );
        assert_eq!(java_major(Some("21.0.4")), 21);
        assert_eq!(java_major(Some("1.8.0_412")), 8);
        assert_eq!(java_major(Some("21-ea")), 21);
        assert_eq!(
            parse_java_major_version("openjdk version \"21-ea\" 2026-03-17"),
            Some(21)
        );
        assert_eq!(parse_java_major_version("openjdk 17"), Some(17));
    }

    #[test]
    fn installation_cache_round_trips_existing_paths() {
        let temp = tempfile::tempdir().unwrap();
        let executable = temp.path().join("java");
        std::fs::write(&executable, b"java").unwrap();
        let cache = temp.path().join("cache/java/installations.json");
        let installations = vec![JavaInstallation {
            path: executable.clone(),
            version: Some("25.0.1".to_owned()),
        }];

        save_installation_cache(&cache, &installations).unwrap();
        assert_eq!(load_installation_cache(&cache), Some(installations));

        std::fs::remove_file(executable).unwrap();
        assert_eq!(load_installation_cache(&cache), None);
    }
}
