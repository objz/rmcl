// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

//! Legacy mcl→rmcl path migration. Runs once before any other init in main().
//! Idempotent: absence of the old dir is the sentinel.

use std::fs;
use std::io;
use std::path::Path;

const OLD_NAME: &str = "mcl";
const NEW_NAME: &str = "rmcl";

pub fn run_legacy_rename() {
    if let Some(dir) = dirs_next::config_dir() {
        rename_top_level(&dir.join(OLD_NAME), &dir.join(NEW_NAME));
    }
    if let Some(dir) = dirs_next::data_dir() {
        let new_data = dir.join(NEW_NAME);
        rename_top_level(&dir.join(OLD_NAME), &new_data);
        cleanup_instance_leftovers(&new_data.join("instances"));
        rewrite_linux_desktop_entries(&dir, &new_data.join("instances"));
    }
    if let Some(dir) = dirs_next::cache_dir() {
        rename_top_level(&dir.join(OLD_NAME), &dir.join(NEW_NAME));
    }
    if let (Some(desk), Some(data)) = (dirs::desktop_dir(), dirs_next::data_dir()) {
        rewrite_native_desktop_shortcuts(&desk, &data.join(NEW_NAME).join("instances"));
    }
    if let (Some(config_dir), Some(data_dir)) = (dirs_next::config_dir(), dirs_next::data_dir())
        && !data_dir.join(OLD_NAME).exists()
        && let Err(error) =
            crate::config::migrate_legacy_data_paths(&config_dir.join(NEW_NAME).join("config.toml"))
    {
        eprintln!("rmcl migration: failed to update legacy config paths: {error}");
    }
}

fn rename_top_level(old: &Path, new: &Path) {
    if !old.exists() {
        return;
    }
    if new.exists() {
        eprintln!(
            "rmcl migration: both {} and {} exist; leaving as-is, please merge manually",
            old.display(),
            new.display()
        );
        return;
    }
    match fs::rename(old, new) {
        Ok(_) => eprintln!(
            "rmcl migration: moved {} -> {}",
            old.display(),
            new.display()
        ),
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            if let Err(e2) = copy_dir_recursive(old, new) {
                eprintln!(
                    "rmcl migration: failed cross-device copy {} -> {}: {}",
                    old.display(),
                    new.display(),
                    e2
                );
                return;
            }
            if let Err(e3) = fs::remove_dir_all(old) {
                eprintln!(
                    "rmcl migration: copied but failed to remove {}: {}",
                    old.display(),
                    e3
                );
                return;
            }
            eprintln!(
                "rmcl migration: cross-device moved {} -> {}",
                old.display(),
                new.display()
            );
        }
        Err(e) => eprintln!(
            "rmcl migration: failed to rename {} -> {}: {}",
            old.display(),
            new.display(),
            e
        ),
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path)?;
        } else {
            fs::copy(entry.path(), &dst_path)?;
        }
    }
    Ok(())
}

fn cleanup_instance_leftovers(instances_dir: &Path) {
    let Ok(entries) = fs::read_dir(instances_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let mc = entry.path().join(crate::storage::MINECRAFT_DIR_NAME);
        for leftover in [".mcl-shim.jar", ".mcl-log4j2.xml"] {
            let p = mc.join(leftover);
            if p.exists() {
                let _ = fs::remove_file(&p);
            }
        }
    }
}

fn rewrite_linux_desktop_entries(_data_dir: &Path, _instances_dir: &Path) {
    #[cfg(target_os = "linux")]
    {
        let apps_dir = _data_dir.join("applications");
        let Ok(entries) = fs::read_dir(_instances_dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let sanitized = sanitize(&name.to_string_lossy());
            let old = apps_dir.join(format!("mcl-{sanitized}.desktop"));
            let new = apps_dir.join(format!("rmcl-{sanitized}.desktop"));
            if old.exists()
                && !new.exists()
                && let Ok(content) = fs::read_to_string(&old)
            {
                let new_content = content.replace("Exec=mcl ", "Exec=rmcl ");
                if fs::write(&new, new_content).is_ok() {
                    let _ = fs::remove_file(&old);
                }
            }
        }
    }
}

fn rewrite_native_desktop_shortcuts(_desktop_dir: &Path, _instances_dir: &Path) {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        // mcl-era windows shortcuts were .bat files; current rmcl writes
        // .vbs, so only the legacy extension is rewritten here.
        let ext = if cfg!(target_os = "windows") {
            "bat"
        } else {
            "command"
        };
        let Ok(entries) = fs::read_dir(_instances_dir) else {
            return;
        };
        for entry in entries.flatten() {
            let display = entry.file_name().to_string_lossy().into_owned();
            // mcl named shortcut files with the sanitized form of the
            // instance name ("My Pack" -> "My_Pack"), so look those up.
            let sanitized = sanitize(&display);
            let path = _desktop_dir.join(format!("Minecraft - {sanitized}.{ext}"));
            if !path.exists() {
                continue;
            }
            if let Ok(content) = fs::read_to_string(&path) {
                let new_content = content.replace("mcl instance launch", "rmcl instance launch");
                if new_content != content {
                    let _ = fs::write(&path, new_content);
                }
            }
        }
    }
}

#[allow(dead_code)]
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "tests/migrate.rs"]
mod tests;
