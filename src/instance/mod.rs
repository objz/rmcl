// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// instance management: creation, launching, importing modpacks, and all the
// bookkeeping that comes with pretending to be a real launcher

pub mod config_sync;
pub mod content;
pub mod desktop;
pub mod import;
pub mod java;
pub mod launch;
pub mod loader;
pub mod logs;
pub mod manager;
pub mod models;
pub mod runtime;
pub mod screenshots;

pub use content::manifest::{
    ContentFileRecord, ContentKind, ContentManifest, FileFingerprint, ProviderProject, Resolution,
};
pub use content::{
    scan_mods, scan_one_datapack, scan_one_mod, scan_one_resource_pack, scan_one_shader,
    scan_one_world, scan_resource_packs, scan_shaders, scan_worlds,
};
pub use launch::LaunchError;
pub use loader::{GameVersion, ModLoaderInstaller, VanillaInstaller, get_installer};
pub use manager::{InstanceError, InstanceManager};
pub use models::{InstanceConfig, ModLoader, WindowMode, normalize_memory_value};
