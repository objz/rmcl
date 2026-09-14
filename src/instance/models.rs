// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// core data types for an instance: what loader it uses, what version, memory
// settings, etc. this is what gets persisted to instance.json

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModLoader {
    Vanilla,
    Fabric,
    Forge,
    NeoForge,
    Quilt,
}

impl fmt::Display for ModLoader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ModLoader::Vanilla => write!(f, "Vanilla"),
            ModLoader::Fabric => write!(f, "Fabric"),
            ModLoader::Forge => write!(f, "Forge"),
            ModLoader::NeoForge => write!(f, "NeoForge"),
            ModLoader::Quilt => write!(f, "Quilt"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowMode {
    #[default]
    Windowed,
    Fullscreen,
}

impl WindowMode {
    fn is_windowed(&self) -> bool {
        *self == Self::Windowed
    }
}

impl fmt::Display for WindowMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Windowed => write!(f, "windowed"),
            Self::Fullscreen => write!(f, "fullscreen"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchCommand {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub command: String,
}

impl LaunchCommand {
    fn is_default(&self) -> bool {
        !self.enabled && self.command.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceConfig {
    pub name: String,
    pub game_version: String,
    pub loader: ModLoader,
    pub loader_version: Option<String>,
    pub created: DateTime<Utc>,
    #[serde(default)]
    pub last_played: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "deserialize_optional_non_empty_string")]
    pub java_path: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_memory")]
    pub memory_max: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_memory")]
    pub memory_min: Option<String>,
    #[serde(default)]
    pub jvm_args: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub environment: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "WindowMode::is_windowed")]
    pub window_mode: WindowMode,
    #[serde(default, skip_serializing_if = "is_false")]
    pub inherit_window_mode: bool,
    #[serde(default, deserialize_with = "deserialize_optional_resolution")]
    pub resolution: Option<(u32, u32)>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub inherit_resolution: bool,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_non_empty_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub preferred_account: Option<String>,
    #[serde(default, skip_serializing_if = "LaunchCommand::is_default")]
    pub pre_launch_command: LaunchCommand,
    #[serde(default, skip_serializing_if = "LaunchCommand::is_default")]
    pub post_exit_command: LaunchCommand,
    #[serde(
        default,
        deserialize_with = "deserialize_optional_non_empty_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub glfw_path: Option<String>,
    #[serde(default)]
    pub config_sync_profile: Option<String>,
    #[serde(default)]
    pub modpack_source: Option<crate::instance::ProviderProject>,
}

impl InstanceConfig {
    pub fn effective_window_mode(&self, global: WindowMode) -> WindowMode {
        if self.inherit_window_mode {
            global
        } else {
            self.window_mode
        }
    }

    pub fn effective_resolution(&self, global: Option<(u32, u32)>) -> Option<(u32, u32)> {
        if self.inherit_resolution {
            global
        } else {
            self.resolution
        }
    }
}

fn is_false(value: &bool) -> bool {
    !value
}

fn deserialize_optional_resolution<'de, D>(deserializer: D) -> Result<Option<(u32, u32)>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<(u32, u32)>::deserialize(deserializer)?
        .filter(|(width, height)| *width > 0 && *height > 0))
}

fn deserialize_optional_non_empty_string<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?
        .and_then(|value| (!value.trim().is_empty()).then(|| value.trim().to_owned())))
}

pub fn parse_resolution(input: &str) -> Result<(u32, u32), String> {
    let (width, height) = input
        .trim()
        .split_once(['x', 'X'])
        .ok_or_else(|| "resolution must be in WxH format".to_string())?;
    let width = width
        .parse::<u32>()
        .map_err(|_| "resolution width must be a positive integer".to_string())?;
    let height = height
        .parse::<u32>()
        .map_err(|_| "resolution height must be a positive integer".to_string())?;

    if width == 0 || height == 0 {
        return Err("resolution values must be greater than zero".to_string());
    }

    Ok((width, height))
}

pub fn normalize_memory_value(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (digits, suffix) = match trimmed.chars().last() {
        Some(c) if c.is_ascii_alphabetic() => (&trimmed[..trimmed.len() - c.len_utf8()], Some(c)),
        Some(_) => (trimmed, None),
        None => return None,
    };

    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    let value = digits.parse::<u64>().ok()?;
    if value == 0 {
        return None;
    }

    match suffix.map(|c| c.to_ascii_uppercase()) {
        Some(unit @ ('K' | 'M' | 'G')) => Some(format!("{value}{unit}")),
        Some(_) => None,
        None => memory_number_to_string(value),
    }
}

pub fn memory_kib(value: &str) -> Option<u64> {
    let normalized = normalize_memory_value(value)?;
    let (number, suffix) = normalized.split_at(normalized.len().saturating_sub(1));
    let number = number.parse::<u64>().ok()?;
    match suffix {
        "K" => Some(number),
        "M" => number.checked_mul(1024),
        "G" => number.checked_mul(1024 * 1024),
        _ => None,
    }
}

fn deserialize_optional_memory<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(raw)) => Ok(normalize_memory_value(&raw)),
        Some(Value::Number(number)) => {
            let number = if let Some(value) = number.as_u64() {
                value
            } else if let Some(value) = number.as_i64() {
                let Ok(value) = u64::try_from(value) else {
                    return Ok(None);
                };
                value
            } else {
                let Some(value) = number.as_f64() else {
                    return Ok(None);
                };
                if !value.is_finite() || value.fract() != 0.0 || value < 0.0 {
                    return Ok(None);
                }
                value as u64
            };
            Ok(memory_number_to_string(number))
        }
        Some(_) => Ok(None),
    }
}

fn memory_number_to_string(value: u64) -> Option<String> {
    if value == 0 {
        return None;
    }
    if value < 128 {
        Some(format!("{value}G"))
    } else {
        Some(format!("{value}M"))
    }
}

#[cfg(test)]
#[path = "tests/models.rs"]
mod tests;
