// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// creates OS-native shortcuts for launching instances directly:
// .desktop files on linux, .bat on windows, .command on macos

use std::path::{Path, PathBuf};

use crate::instance::models::InstanceConfig;

const ICON_BYTES: &[u8] = include_bytes!("../../assets/icon.svg");

pub fn desktop_path(name: &str) -> Option<PathBuf> {
    let sanitized = sanitize(name);

    #[cfg(target_os = "linux")]
    {
        dirs_next::data_dir().map(|d| {
            d.join("applications")
                .join(format!("rmcl-{sanitized}.desktop"))
        })
    }

    #[cfg(target_os = "windows")]
    {
        dirs::desktop_dir().map(|d| d.join(format!("Minecraft - {sanitized}.vbs")))
    }

    #[cfg(target_os = "macos")]
    {
        dirs::desktop_dir().map(|d| d.join(format!("Minecraft - {sanitized}.command")))
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        let _ = sanitized;
        None
    }
}

pub fn icon_path() -> Option<PathBuf> {
    dirs_next::data_dir().map(|d| d.join("rmcl").join("icon.svg"))
}

// lazily writes the bundled svg icon to disk the first time a shortcut needs it
fn ensure_icon() -> Option<PathBuf> {
    let path = icon_path()?;
    if std::fs::read(&path).ok().as_deref() == Some(ICON_BYTES) {
        return Some(path);
    }
    let parent = path.parent()?;
    if let Err(e) = std::fs::create_dir_all(parent) {
        tracing::warn!("Failed to create icon directory: {}", e);
        return None;
    }
    if let Err(e) = crate::storage::write_atomic(&path, ICON_BYTES) {
        tracing::warn!("Failed to write bundled icon: {}", e);
        return None;
    }
    Some(path)
}

pub fn exists(name: &str) -> bool {
    desktop_path(name).map(|p| p.exists()).unwrap_or(false)
}

pub fn create(config: &InstanceConfig) -> std::io::Result<PathBuf> {
    let path = desktop_path(&config.name)
        .ok_or_else(|| std::io::Error::other("cannot resolve shortcut directory"))?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let icon = ensure_icon();
    let content = build_content(&config.name, icon.as_deref());
    if std::fs::read_to_string(&path).ok().as_deref() != Some(content.as_str()) {
        crate::storage::write_atomic(&path, content.as_bytes())?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        std::fs::set_permissions(&path, perms)?;
    }

    Ok(path)
}

pub fn remove(name: &str) -> std::io::Result<()> {
    let Some(path) = desktop_path(name) else {
        return Ok(());
    };
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

pub fn set_enabled(config: &InstanceConfig, enabled: bool) -> std::io::Result<()> {
    if enabled {
        create(config).map(|_| ())
    } else {
        remove(&config.name)
    }
}

pub fn toggle(config: &InstanceConfig) -> std::io::Result<bool> {
    let enabled = !exists(&config.name);
    set_enabled(config, enabled)?;
    Ok(enabled)
}

pub fn rename(old_name: &str, new_config: &InstanceConfig) -> std::io::Result<()> {
    let Some(old_path) = desktop_path(old_name).filter(|path| path.exists()) else {
        return Ok(());
    };
    create(new_config)?;
    if desktop_path(&new_config.name).as_ref() != Some(&old_path) {
        std::fs::remove_file(old_path)?;
    }
    Ok(())
}

fn build_content(name: &str, icon: Option<&Path>) -> String {
    #[cfg(target_os = "linux")]
    {
        build_linux_desktop(name, icon)
    }

    #[cfg(target_os = "windows")]
    {
        let _ = icon;
        build_windows_shortcut(name)
    }

    #[cfg(target_os = "macos")]
    {
        let _ = icon;
        build_macos_command(name)
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        let _ = (name, icon);
        String::new()
    }
}

#[cfg(target_os = "linux")]
fn build_linux_desktop(name: &str, icon: Option<&Path>) -> String {
    let mut out = String::new();
    out.push_str("[Desktop Entry]\n");
    out.push_str("Version=0.3.1\n");
    out.push_str("Type=Application\n");
    out.push_str(&format!("Name=Minecraft - {name}\n"));
    out.push_str(&format!("Comment=Launch {name} Minecraft instance\n"));
    out.push_str(&format!(
        "Exec=rmcl instance launch {}\n",
        quote_desktop_exec_arg(name)
    ));
    if let Some(icon) = icon {
        out.push_str(&format!("Icon={}\n", icon.display()));
    }
    out.push_str("Terminal=false\n");
    out.push_str("Categories=Game;\n");
    out
}

#[cfg(target_os = "windows")]
fn build_windows_shortcut(name: &str) -> String {
    let command = format!("rmcl instance launch {}", quote_windows_arg(name));
    let escaped_command = command.replace('"', "\"\"");

    let mut out = String::new();
    out.push_str("Set shell = CreateObject(\"WScript.Shell\")\r\n");
    out.push_str(&format!("shell.Run \"{escaped_command}\", 0, False\r\n"));
    out
}

#[cfg(target_os = "macos")]
fn build_macos_command(name: &str) -> String {
    let mut out = String::new();
    out.push_str("#!/bin/bash\n");
    out.push_str(&format!("# Launch Minecraft instance: {name}\n"));
    out.push_str(&format!("rmcl instance launch {}\n", quote_shell_arg(name)));
    out
}

fn quote_desktop_exec_arg(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for character in value.chars() {
        if matches!(character, '"' | '`' | '$' | '\\') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped.push('"');
    escaped
}

#[cfg(any(target_os = "macos", test))]
fn quote_shell_arg(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(any(target_os = "windows", test))]
fn quote_windows_arg(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    let mut backslashes = 0;
    for character in value.chars() {
        if character == '\\' {
            backslashes += 1;
        } else if character == '"' {
            quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
            quoted.push('"');
            backslashes = 0;
        } else {
            quoted.extend(std::iter::repeat_n('\\', backslashes));
            quoted.push(character);
            backslashes = 0;
        }
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

// replaces anything that isn't alphanumeric, dash, or underscore with _
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
#[path = "tests/desktop.rs"]
mod tests;
