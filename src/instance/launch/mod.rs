// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// builds the full java command line and spawns minecraft as a child process.
// handles classpath assembly, auth token injection, and log capture.
// loader-specific patches live in submodules (e.g. patches.rs for lwjgl3ify).

pub(crate) mod parser;
mod patches;

use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::auth::AccountType;
use crate::instance::models::{InstanceConfig, LaunchCommand, ModLoader, WindowMode};
use crate::launch_profile::model::{Argument, LaunchProfile};
use crate::launch_profile::rules::{self, FeatureSet, RuleAction, RuleContext};
use crate::launch_profile::templates::TemplateContext;
use crate::launch_profile::{render, resolve, system};

#[derive(Debug, Error)]
pub enum LaunchError {
    #[error("Version metadata not found: {0}. Re-create the instance to fix this.")]
    MetaNotFound(String),
    #[error("Profile error: {0}")]
    Parse(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0} launch is not yet supported")]
    NotSupported(String),
    #[error("This instance requires Java {required}, but rmcl is using Java {detected}: {java}")]
    JavaTooOld {
        java: String,
        required: u32,
        detected: u32,
    },
    #[error("This instance requires Java {required}, but rmcl could not check {java}: {reason}")]
    JavaCheckFailed {
        java: String,
        required: u32,
        reason: String,
    },
    #[error("{0}")]
    Auth(String),
    #[error("{phase} command failed: {reason}")]
    Command { phase: &'static str, reason: String },
    #[error("Config sync error: {0}")]
    ConfigSync(#[from] crate::instance::config_sync::ConfigSyncError),
}

fn build_game_args(
    profile: &LaunchProfile,
    rule_ctx: &RuleContext<'_>,
    template_ctx: &TemplateContext<'_>,
) -> Result<(Vec<String>, Vec<String>), LaunchError> {
    let rendered = render::render_args(profile, rule_ctx, template_ctx)
        .map_err(|e| LaunchError::Parse(format!("Failed to render args: {e}")))?;
    Ok((rendered.jvm, rendered.game))
}

fn apply_custom_resolution(game_args: &mut Vec<String>, resolution: Option<(u32, u32)>) {
    let Some((width, height)) = resolution else {
        return;
    };
    if !game_args.iter().any(|arg| arg == "--width") {
        game_args.extend(["--width".to_owned(), width.to_string()]);
    }
    if !game_args.iter().any(|arg| arg == "--height") {
        game_args.extend(["--height".to_owned(), height.to_string()]);
    }
}

fn apply_window_mode(game_args: &mut Vec<String>, window_mode: WindowMode) {
    if window_mode == WindowMode::Windowed {
        game_args.retain(|argument| argument != "--fullscreen");
    } else if !game_args.iter().any(|arg| arg == "--fullscreen") {
        game_args.push("--fullscreen".to_owned());
    }
}

async fn check_java_version(java: &str, required: Option<u32>) -> Result<(), LaunchError> {
    let Some(required) = required.filter(|major| *major > 0) else {
        return Ok(());
    };

    let mut command = tokio::process::Command::new(java);
    command.arg("-version").kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(10), command.output())
        .await
        .map_err(|_| LaunchError::JavaCheckFailed {
            java: java.to_owned(),
            required,
            reason: "version check timed out".to_owned(),
        })?
        .map_err(|e| LaunchError::JavaCheckFailed {
            java: java.to_owned(),
            required,
            reason: e.to_string(),
        })?;

    if !output.status.success() {
        return Err(LaunchError::JavaCheckFailed {
            java: java.to_owned(),
            required,
            reason: format!("`java -version` exited with {}", output.status),
        });
    }

    let version_text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let detected =
        crate::instance::java::parse_java_major_version(&version_text).ok_or_else(|| {
            LaunchError::JavaCheckFailed {
                java: java.to_owned(),
                required,
                reason: format!("could not parse `java -version` output: {version_text:?}"),
            }
        })?;

    if detected < required {
        return Err(LaunchError::JavaTooOld {
            java: java.to_owned(),
            required,
            detected,
        });
    }

    Ok(())
}

// existing installs from rmcl <= 0.3.0 have meta.json files in the
// stripped legacy format (no `arguments`, no `minecraftArguments`). every
// real upstream profile has at least one of those fields. on detecting the
// stripped format, re-fetch the version metadata from mojang's manifest
// and overwrite the file with the raw upstream bytes.
async fn migrate_legacy_meta_if_needed(
    meta_path: &Path,
    profile: &LaunchProfile,
    game_version: &str,
) -> Result<Option<LaunchProfile>, LaunchError> {
    if profile.arguments.is_some() || profile.minecraft_arguments.is_some() {
        return Ok(None);
    }

    tracing::warn!(
        "Cached meta.json for {game_version} is missing arguments; re-fetching from Mojang"
    );

    let client = crate::net::HttpClient::new();
    let manifest = match crate::net::mojang::fetch_version_manifest(&client).await {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(
                "Could not reach Mojang manifest ({e}); proceeding with the cached legacy profile. \
                 Modern features like Forge's --add-opens flags may be missing until the next online launch."
            );
            return Ok(None);
        }
    };

    let entry = manifest
        .versions
        .iter()
        .find(|v| v.id == game_version)
        .ok_or_else(|| {
            LaunchError::Parse(format!(
                "Version {game_version} not found in Mojang manifest"
            ))
        })?;

    let (_meta, raw) = match crate::net::mojang::fetch_version_meta_with_raw(&client, entry).await {
        Ok(ok) => ok,
        Err(e) => {
            tracing::warn!(
                "Could not refetch version metadata from Mojang ({e}); proceeding with the cached legacy profile."
            );
            return Ok(None);
        }
    };

    let refreshed: LaunchProfile = serde_json::from_slice(&raw)
        .map_err(|e| LaunchError::Parse(format!("Failed to parse refreshed meta: {e}")))?;
    if refreshed.arguments.is_none() && refreshed.minecraft_arguments.is_none() {
        tracing::warn!(
            "Refetched metadata for {game_version} still has no launch arguments; keeping the cached profile"
        );
        return Ok(None);
    }
    crate::storage::write_atomic(meta_path, &raw)?;
    Ok(Some(refreshed))
}

// the forge/neoforge installer writes its version JSON to a path that's
// loader-specific. encode the naming convention here so migration code
// can find the original file when it needs to rebuild our cache.
fn installer_version_dir_name(
    loader: ModLoader,
    game_version: &str,
    loader_version: &str,
) -> Option<String> {
    match loader {
        ModLoader::Forge => Some(format!("{game_version}-forge-{loader_version}")),
        ModLoader::NeoForge => Some(format!("neoforge-{loader_version}")),
        ModLoader::Vanilla | ModLoader::Fabric | ModLoader::Quilt => None,
    }
}

// loader profiles installed by rmcl <= 0.3.0 are in our stripped
// `{mainClass, libraries[, gameArguments]}` format, which silently drops
// `inheritsFrom`, `arguments.jvm`, and conditional rules from upstream.
// detect that shape (no inheritsFrom AND no arguments AND no
// minecraftArguments - every real upstream profile has at least one) and
// rebuild from the installer's original JSON if it's still on disk.
async fn migrate_legacy_loader_profile_if_needed(
    profile_path: &Path,
    profile: &LaunchProfile,
    config: &InstanceConfig,
    instance_dir: &Path,
) -> Result<Option<LaunchProfile>, LaunchError> {
    // Fabric and Quilt fetch their profiles from a network endpoint at
    // install time; there's no installer-written JSON on disk to recover
    // from. their upstream profiles also happen to match the "legacy
    // stripped" predicate (no inheritsFrom, no arguments), so without this
    // early return every Fabric/Quilt launch would incorrectly fail
    // migration. resolve() handles their lack of inheritsFrom via the
    // implicit fallback in the launch flow.
    if matches!(config.loader, ModLoader::Fabric | ModLoader::Quilt) {
        return Ok(None);
    }

    // tightened predicate per the spec: only treat a profile as "legacy
    // stripped" when our old `gameArguments` field is present. that field
    // is unique to rmcl <= 0.3.0's custom shape; no upstream profile
    // emits it. without this gate, an upstream profile that happens to
    // omit inheritsFrom/arguments/minecraftArguments would be mistakenly
    // re-extracted from the installer JSON.
    let is_legacy = profile.inherits_from.is_none()
        && profile.arguments.is_none()
        && profile.minecraft_arguments.is_none()
        && profile.game_arguments.is_some();
    if !is_legacy {
        return Ok(None);
    }

    let Some(loader_version) = config.loader_version.as_deref() else {
        return Err(LaunchError::Parse(format!(
            "Loader profile at {} is in an outdated format and the instance config has no \
             loader_version recorded. Reinstall {} for this instance.",
            profile_path.display(),
            config.loader
        )));
    };
    let Some(version_dir) =
        installer_version_dir_name(config.loader, &config.game_version, loader_version)
    else {
        // unreachable today: only Vanilla/Fabric/Quilt return None, and
        // Vanilla doesn't pass this code path (no loader profile to
        // migrate) while Fabric/Quilt are filtered above.
        return Err(LaunchError::Parse(format!(
            "Loader profile at {} is in an outdated format. Reinstall {} for this instance.",
            profile_path.display(),
            config.loader
        )));
    };

    let installer_json_path = instance_dir
        .join(crate::storage::MINECRAFT_DIR_NAME)
        .join("versions")
        .join(&version_dir)
        .join(format!("{version_dir}.json"));

    if !installer_json_path.exists() {
        return Err(LaunchError::Parse(format!(
            "Loader profile at {} is in an outdated format and the installer JSON at {} \
             is missing. Reinstall {} for this instance.",
            profile_path.display(),
            installer_json_path.display(),
            config.loader
        )));
    }

    tracing::warn!(
        "Loader profile {} is in legacy format; rebuilding from {}",
        profile_path.display(),
        installer_json_path.display()
    );

    let raw = tokio::fs::read(&installer_json_path).await?;
    let refreshed: LaunchProfile = serde_json::from_slice(&raw).map_err(|e| {
        LaunchError::Parse(format!("Failed to parse refreshed loader profile: {e}"))
    })?;
    crate::storage::write_atomic(profile_path, &raw)?;
    Ok(Some(refreshed))
}

// resolved auth credentials passed into the launch-invocation builder.
// keeping these as borrowed strs lets callers pass owned strings or string
// slices without forcing allocation.
#[derive(Debug, Clone)]
pub struct LaunchAuth<'a> {
    pub username: &'a str,
    pub uuid: &'a str,
    pub token: &'a str,
    // "msa" for Microsoft, "legacy" for offline; mirrors Mojang's user_type.
    pub user_type: &'a str,
}

// everything the spawner needs to construct the java command. assembled by
// build_launch_invocation, consumed by launch(). exposed so integration tests
// can assert on the rendered invocation without spawning a real process.
#[derive(Debug, Clone)]
pub struct LaunchInvocation {
    pub java: String,
    pub jvm_args: Vec<String>,
    pub classpath: Vec<PathBuf>,
    pub classpath_string: String,
    pub main_class: String,
    pub extra_args: Vec<String>,
    pub game_args: Vec<String>,
    pub environment: std::collections::BTreeMap<String, String>,
    pub working_dir: PathBuf,
}

pub fn supports_quick_play(meta_dir: &Path, game_version: &str) -> bool {
    let path = crate::storage::MetadataPaths::new(meta_dir)
        .versions()
        .join(game_version)
        .join("meta.json");
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<LaunchProfile>(&bytes).ok())
        .is_some_and(|profile| profile_supports_quick_play(&profile))
}

fn profile_supports_quick_play(profile: &LaunchProfile) -> bool {
    profile.arguments.as_ref().is_some_and(|arguments| {
        arguments.game.iter().any(|argument| {
            let Argument::Conditional { rules, .. } = argument else {
                return false;
            };
            rules.iter().any(|rule| {
                rule.action == RuleAction::Allow
                    && rule
                        .features
                        .as_ref()
                        .is_some_and(|features| features.is_quick_play_singleplayer == Some(true))
            })
        })
    })
}

fn validate_quick_play_world(minecraft_dir: &Path, world: &str) -> Result<(), LaunchError> {
    let mut components = Path::new(world).components();
    let is_single_name = matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none();
    if !is_single_name || !minecraft_dir.join("saves").join(world).is_dir() {
        return Err(LaunchError::Parse(
            "Selected Quick Play world is not a valid save".to_owned(),
        ));
    }
    Ok(())
}

// builds a fully-resolved java invocation for the given instance. reads
// meta.json and the loader profile from disk, migrates legacy formats if
// needed (may hit Mojang to refetch), resolves inheritsFrom, applies
// loader-specific patches, and renders all template arguments. all I/O
// except auth resolution and process spawning happens here.
pub async fn build_launch_invocation(
    config: &InstanceConfig,
    instances_dir: &Path,
    meta_dir: &Path,
    auth: &LaunchAuth<'_>,
    quick_play_world: Option<&str>,
) -> Result<LaunchInvocation, LaunchError> {
    let instance_dir = instances_dir.join(&config.name);
    let minecraft_dir = instance_dir.join(crate::storage::MINECRAFT_DIR_NAME);
    let (window_mode, resolution) = {
        let defaults = &crate::config::SETTINGS.read().defaults;
        (
            config.effective_window_mode(defaults.window_mode),
            config.effective_resolution(defaults.resolution),
        )
    };

    let metadata_paths = crate::storage::MetadataPaths::new(meta_dir);
    let meta_path = metadata_paths
        .versions()
        .join(&config.game_version)
        .join("meta.json");
    if !meta_path.exists() {
        return Err(LaunchError::MetaNotFound(meta_path.display().to_string()));
    }
    let meta: LaunchProfile = serde_json::from_slice(&tokio::fs::read(&meta_path).await?)?;
    let meta = match migrate_legacy_meta_if_needed(&meta_path, &meta, &config.game_version).await? {
        Some(refreshed) => refreshed,
        None => meta,
    };

    if let Some(world) = quick_play_world {
        validate_quick_play_world(&minecraft_dir, world)?;
    }

    let current_features = FeatureSet {
        is_quick_play_singleplayer: quick_play_world.map(|_| true),
        has_custom_resolution: resolution.map(|_| true),
        ..Default::default()
    };
    let host_os_version = system::mojang_os_version();
    let rule_ctx = RuleContext {
        os_name: system::mojang_os_name(),
        os_version: &host_os_version,
        arch: system::mojang_arch_name(),
        features: &current_features,
    };

    let asset_index_id = meta
        .asset_index
        .as_ref()
        .map(|ai| ai.id.clone())
        .unwrap_or_default();

    let lib_dir = metadata_paths.libraries();

    let lv = config.loader_version.as_deref().unwrap_or("unknown");
    let profile_filename =
        crate::instance::loader::profile_filename(config.loader, &config.game_version, lv);

    // load the loader profile (if any), migrate from the old stripped format
    // if needed, and resolve `inheritsFrom` against the vanilla parent (which
    // the vanilla meta migration above ensured is fresh on disk). when no
    // loader is configured we use the already-loaded vanilla meta directly.
    let merged_profile: LaunchProfile = if let Some(filename) = &profile_filename {
        let profile_path = metadata_paths.loader_profiles().join(filename);
        if !profile_path.exists() {
            return Err(LaunchError::MetaNotFound(
                profile_path.display().to_string(),
            ));
        }
        let mut loader_profile: LaunchProfile =
            serde_json::from_slice(&tokio::fs::read(&profile_path).await?)?;

        if let Some(refreshed) = migrate_legacy_loader_profile_if_needed(
            &profile_path,
            &loader_profile,
            config,
            &instance_dir,
        )
        .await?
        {
            loader_profile = refreshed;
        }

        // legacy installer-written profiles (and any loader profile that
        // omits inheritsFrom) still need to be layered over vanilla. set
        // the inherit explicitly so resolve() walks the chain.
        if loader_profile.inherits_from.is_none() {
            loader_profile.inherits_from = Some(config.game_version.clone());
        }

        resolve::resolve(loader_profile, meta_dir)
            .await
            .map_err(|e| LaunchError::Parse(format!("Failed to resolve loader profile: {e}")))?
    } else {
        meta.clone()
    };

    let main_class = merged_profile
        .main_class
        .clone()
        .ok_or_else(|| LaunchError::Parse("merged profile missing mainClass".into()))?;

    if quick_play_world.is_some() && !profile_supports_quick_play(&merged_profile) {
        return Err(LaunchError::NotSupported("Quick Play".to_owned()));
    }

    // rebuild the classpath from the merged profile. vanilla-style libraries
    // have `downloads.artifact.path` set and live in meta_dir/libraries/.
    // loader-style libraries only have a maven coordinate; for forge/neoforge,
    // the installer drops some of them into <instance>/.minecraft/libraries/
    // so we check there first.
    let has_local_libs = matches!(config.loader, ModLoader::Forge | ModLoader::NeoForge);
    let local_lib_dir = minecraft_dir.join("libraries");
    let library_directory = if has_local_libs {
        &local_lib_dir
    } else {
        &lib_dir
    };

    let mut classpath: Vec<PathBuf> = Vec::new();
    for lib in &merged_profile.libraries {
        if let Some(rules) = &lib.rules
            && !rules::evaluate(rules, &rule_ctx)
        {
            continue;
        }

        // resolve a relative path for this library. prefer downloads.artifact.path
        // when present (vanilla-style), fall back to maven_coord_to_path(name)
        // for loader-style entries that only have a coord.
        let rel: PathBuf = match lib
            .downloads
            .as_ref()
            .and_then(|d| d.artifact.as_ref())
            .map(|a| PathBuf::from(&a.path))
            .or_else(|| {
                crate::instance::loader::maven::maven_coord_to_path(&lib.name).map(PathBuf::from)
            }) {
            Some(p) => p,
            None => continue,
        };

        // for forge/neoforge, the installer drops some libs (notably the
        // bootstrap library) into <instance>/.minecraft/libraries/ rather
        // than the shared meta cache. check there first regardless of
        // whether the lib has a downloads.artifact entry.
        if has_local_libs {
            let in_local = local_lib_dir.join(&rel);
            if in_local.exists() {
                classpath.push(in_local);
                continue;
            }
        }
        classpath.push(lib_dir.join(rel));
    }

    classpath.push(
        metadata_paths
            .versions()
            .join(&config.game_version)
            .join(format!("{}.jar", config.game_version)),
    );

    // apply loader-specific patches (lwjgl3ify for old forge on java 9+)
    let (patch_jvm_args, main_class, extra_args) = if matches!(config.loader, ModLoader::Forge) {
        match patches::apply(&minecraft_dir, &lib_dir, &mut classpath).await {
            Some(p) => (p.jvm_args, p.main_class, p.extra_args),
            None => (Vec::new(), main_class, Vec::new()),
        }
    } else {
        (Vec::new(), main_class, Vec::new())
    };

    let sep = if cfg!(windows) { ";" } else { ":" };
    let cp_str = classpath
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(sep);

    // java resolution: instance override > global setting > auto-detect
    let java = config
        .java_path
        .clone()
        .or_else(|| {
            crate::config::SETTINGS
                .read()
                .paths
                .effective_java_path()
                .map(str::to_owned)
        })
        .unwrap_or_else(crate::instance::java::detect_java_path);

    check_java_version(
        &java,
        merged_profile
            .java_version
            .as_ref()
            .map(|version| version.major_version),
    )
    .await?;

    let assets_root = metadata_paths.assets();
    let natives_dir = metadata_paths
        .versions()
        .join(&config.game_version)
        .join("natives");
    let version_type = merged_profile.type_.as_deref().unwrap_or("release");
    let resolution_width = resolution.map(|(width, _)| width.to_string());
    let resolution_height = resolution.map(|(_, height)| height.to_string());
    let template_ctx = TemplateContext {
        library_directory,
        classpath_separator: sep,
        version_name: &config.game_version,
        version_type,
        natives_directory: &natives_dir,
        classpath: &cp_str,
        game_directory: &minecraft_dir,
        assets_root: &assets_root,
        assets_index_name: &asset_index_id,
        auth_player_name: auth.username,
        auth_uuid: auth.uuid,
        auth_access_token: auth.token,
        auth_xuid: "0",
        user_type: auth.user_type,
        user_properties: "{}",
        launcher_name: "rmcl",
        launcher_version: env!("CARGO_PKG_VERSION"),
        clientid: "0",
        quick_play_singleplayer: quick_play_world,
        resolution_width: resolution_width.as_deref(),
        resolution_height: resolution_height.as_deref(),
    };

    let (upstream_jvm_args, mut game_args) =
        build_game_args(&merged_profile, &rule_ctx, &template_ctx)?;
    // Modern Mojang profiles include feature-gated resolution arguments.
    // Older and third-party profiles may not, so add them when absent.
    apply_custom_resolution(&mut game_args, resolution);
    apply_window_mode(&mut game_args, window_mode);

    let (memory_min, memory_max, global_jvm_args, global_environment) = {
        let settings = crate::config::SETTINGS.read();
        (
            config
                .memory_min
                .clone()
                .unwrap_or_else(|| settings.defaults.memory_min.clone()),
            config
                .memory_max
                .clone()
                .unwrap_or_else(|| settings.defaults.memory_max.clone()),
            settings.defaults.jvm_args.clone(),
            settings.defaults.environment.clone(),
        )
    };
    let mut jvm_args: Vec<String> = vec![format!("-Xms{memory_min}"), format!("-Xmx{memory_max}")];
    jvm_args.extend(patch_jvm_args);
    jvm_args.extend(upstream_jvm_args);
    jvm_args.extend(global_jvm_args);
    jvm_args.extend(config.jvm_args.clone());
    if let Some(glfw_path) = config.glfw_path.as_deref() {
        jvm_args.push(format!("-Dorg.lwjgl.glfw.libname={glfw_path}"));
    }

    let mut environment = global_environment;
    environment.extend(config.environment.clone());

    Ok(LaunchInvocation {
        java,
        jvm_args,
        classpath,
        classpath_string: cp_str,
        main_class,
        extra_args,
        game_args,
        environment,
        working_dir: minecraft_dir,
    })
}

fn command_process(command: &str) -> tokio::process::Command {
    #[cfg(windows)]
    {
        let mut process = tokio::process::Command::new("cmd");
        process.args(["/C", command]);
        process
    }
    #[cfg(not(windows))]
    {
        let mut process = tokio::process::Command::new("/bin/sh");
        process.args(["-c", command]);
        process
    }
}

fn select_launch_account<'a>(
    accounts: &'a [crate::auth::Account],
    preferred: Option<&str>,
) -> Option<&'a crate::auth::Account> {
    preferred
        .and_then(|uuid| accounts.iter().find(|account| account.uuid == uuid))
        .or_else(|| accounts.iter().find(|account| account.active))
}

async fn run_launch_command(
    phase: &'static str,
    command: &LaunchCommand,
    config: &InstanceConfig,
    invocation: &LaunchInvocation,
    instance_dir: &Path,
) -> Result<(), LaunchError> {
    if !command.enabled || command.command.trim().is_empty() {
        return Ok(());
    }

    tracing::info!("[{}] Running {} command", config.name, phase.to_lowercase());
    let mut process = command_process(&command.command);
    process
        .current_dir(&invocation.working_dir)
        .envs(&invocation.environment)
        .env("INST_NAME", &config.name)
        .env("INST_ID", &config.name)
        .env("INST_DIR", instance_dir)
        .env("INST_MC_DIR", &invocation.working_dir)
        .env("INST_JAVA", &invocation.java)
        .env("INST_JAVA_ARGS", invocation.jvm_args.join(" "));
    let output = process
        .output()
        .await
        .map_err(|error| LaunchError::Command {
            phase,
            reason: error.to_string(),
        })?;
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        tracing::info!("[{}] [{}] {}", config.name, phase, line);
    }
    for line in String::from_utf8_lossy(&output.stderr).lines() {
        tracing::warn!("[{}] [{}] {}", config.name, phase, line);
    }
    if output.status.success() {
        Ok(())
    } else {
        Err(LaunchError::Command {
            phase,
            reason: output.status.to_string(),
        })
    }
}

fn finish_config_sync(
    active: bool,
    profile: Option<&str>,
    meta_dir: &Path,
    minecraft_dir: &Path,
    instance: &str,
) {
    if active
        && let Err(error) = crate::instance::config_sync::finish(profile, meta_dir, minecraft_dir)
    {
        tracing::warn!("Failed to sync config for '{}': {}", instance, error);
    }
}

// resolves auth credentials, then builds the launch invocation and spawns
// the java process. only thin wrapper logic lives here: token refresh,
// process spawn, child supervision. all the heavy lifting (profile loading,
// classpath assembly, template rendering) sits behind build_launch_invocation.
pub async fn launch(
    config: &InstanceConfig,
    instances_dir: &Path,
    meta_dir: &Path,
    quick_play_world: Option<&str>,
) -> Result<(), LaunchError> {
    let name = config.name.clone();

    // resolve auth credentials, refreshing the microsoft token if needed.
    let mut account_store = crate::auth::AccountStore::load();
    let account =
        select_launch_account(&account_store.accounts, config.preferred_account.as_deref());
    let Some(acc) = account.cloned() else {
        return Err(LaunchError::Auth("No account selected".to_owned()));
    };

    // offline accounts can only launch if a microsoft account exists
    // (proves the user owns minecraft).
    if acc.account_type != AccountType::Microsoft && !account_store.has_microsoft_account() {
        return Err(LaunchError::Auth(
            "Offline accounts require a Microsoft account that owns Minecraft".to_owned(),
        ));
    }

    let (token, new_refresh, new_expires) = match acc.account_type {
        AccountType::Microsoft => match crate::auth::refresh_and_get_token(&acc).await {
            Ok(triple) => triple,
            Err(e) => return Err(LaunchError::Auth(format!("Authentication failed: {e}"))),
        },
        AccountType::Offline => ("0".to_string(), None, None),
    };

    if let Some(stored) = account_store
        .accounts
        .iter_mut()
        .find(|a| a.uuid == acc.uuid)
    {
        let mut changed = false;
        if let Some(new_rt) = new_refresh {
            stored.refresh_token = Some(new_rt);
            changed = true;
        }
        if let Some(expires) = new_expires {
            stored.cached_mc_token = Some(token.clone());
            stored.cached_mc_token_expires_at = Some(expires);
            changed = true;
        }
        if changed {
            account_store.save();
        }
    }

    let user_type = match acc.account_type {
        AccountType::Microsoft => "msa",
        AccountType::Offline => "legacy",
    };

    let auth = LaunchAuth {
        username: &acc.username,
        uuid: &acc.uuid,
        token: &token,
        user_type,
    };

    let invocation =
        build_launch_invocation(config, instances_dir, meta_dir, &auth, quick_play_world).await?;
    let instance_dir = instances_dir.join(&config.name);
    tracing::debug!(
        "[{}] Prepared launch invocation: working_dir={} classpath_entries={} jvm_args={} extra_args={} game_args={} main_class={}",
        name,
        invocation.working_dir.display(),
        invocation.classpath.len(),
        invocation.jvm_args.len(),
        invocation.extra_args.len(),
        invocation.game_args.len(),
        invocation.main_class
    );
    let config_sync_profile = config.config_sync_profile.clone();
    let config_sync_active = crate::instance::config_sync::prepare(
        config_sync_profile.as_deref(),
        meta_dir,
        &invocation.working_dir,
    )?;
    if let Err(error) = run_launch_command(
        "Pre-launch",
        &config.pre_launch_command,
        config,
        &invocation,
        &instance_dir,
    )
    .await
    {
        finish_config_sync(
            config_sync_active,
            config_sync_profile.as_deref(),
            meta_dir,
            &invocation.working_dir,
            &name,
        );
        return Err(error);
    }

    let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();
    crate::instance::runtime::register_kill(&name, kill_tx);
    crate::instance::runtime::set_state(&name, crate::instance::runtime::RunState::Starting);
    tracing::info!(
        "[{}] Starting Minecraft ({} {})",
        name,
        config.game_version,
        config.loader
    );

    tracing::info!("[{}] Java: {}", name, invocation.java);
    tracing::info!("[{}] JVM args: {:?}", name, invocation.jvm_args);
    tracing::info!(
        "[{}] Classpath:\n{}",
        name,
        invocation
            .classpath
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    );
    tracing::info!("[{}] Main class: {}", name, invocation.main_class);

    let mut cmd = tokio::process::Command::new(&invocation.java);
    cmd.args(&invocation.jvm_args);
    cmd.arg("-cp").arg(&invocation.classpath_string);
    cmd.arg(&invocation.main_class);
    cmd.args(&invocation.extra_args);
    cmd.args(&invocation.game_args);
    cmd.current_dir(&invocation.working_dir);
    cmd.envs(&invocation.environment);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            crate::instance::runtime::cleanup_kill_sender(&name);
            crate::instance::runtime::remove(&name);
            finish_config_sync(
                config_sync_active,
                config_sync_profile.as_deref(),
                meta_dir,
                &invocation.working_dir,
                &name,
            );
            tracing::error!("[{}] Failed to spawn Minecraft process: {}", name, e);
            return Err(LaunchError::Io(e));
        }
    };
    tracing::debug!("[{}] Spawned Minecraft process", name);

    crate::instance::runtime::set_state(&name, crate::instance::runtime::RunState::Running);

    let log_file_path = crate::instance::logs::files::create_log_file(instances_dir, &name);
    match &log_file_path {
        Some(path) => tracing::debug!(
            "[{}] Writing Minecraft process log to {}",
            name,
            path.display()
        ),
        None => tracing::warn!("[{}] Could not create Minecraft process log file", name),
    }

    let name_for_task = name.clone();
    let instances_dir_owned = instances_dir.to_path_buf();
    let meta_dir_owned = meta_dir.to_path_buf();
    let minecraft_dir_owned = invocation.working_dir.clone();
    let instance_dir_owned = instances_dir.join(&config.name);
    let post_exit_command = config.post_exit_command.clone();
    let config_for_post_exit = config.clone();
    let invocation_for_post_exit = invocation.clone();

    // spawn a background task to babysit the child process: capture stdout/stderr
    // into both the TUI log viewer and a timestamped log file on disk
    tokio::spawn(async move {
        use std::io::Write;
        use std::sync::{Arc, Mutex};
        use tokio::io::AsyncBufReadExt;
        use tokio::sync::mpsc;
        use tokio::time::{Duration, sleep};

        use crate::instance::launch::parser::{LogStream, MinecraftLogParser};

        let log_writer: Arc<Mutex<Option<std::fs::File>>> = Arc::new(Mutex::new(
            log_file_path.and_then(|p| std::fs::File::create(p).ok()),
        ));

        let (log_tx, mut log_rx) = mpsc::channel::<(LogStream, String)>(1024);
        let parser_name = name_for_task.clone();
        let parser_task = tokio::spawn(async move {
            let mut parser = MinecraftLogParser::new();
            let idle_flush = Duration::from_millis(150);

            loop {
                tokio::select! {
                    maybe_line = log_rx.recv() => {
                        match maybe_line {
                            Some((stream, line)) => {
                                for event in parser.push_line(stream, line) {
                                    emit_parsed_instance_log(&parser_name, event);
                                }
                            }
                            None => break,
                        }
                    }
                    _ = sleep(idle_flush), if parser.has_pending() => {
                        if let Some(event) = parser.flush() {
                            emit_parsed_instance_log(&parser_name, event);
                        }
                    }
                }
            }

            if let Some(event) = parser.flush() {
                emit_parsed_instance_log(&parser_name, event);
            }
        });

        if let Some(stdout) = child.stdout.take() {
            let w = log_writer.clone();
            let tx = log_tx.clone();
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            tokio::spawn(async move {
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Ok(mut f) = w.lock()
                        && let Some(f) = f.as_mut()
                    {
                        let _ = writeln!(f, "{}", line);
                    }
                    if tx.send((LogStream::Stdout, line)).await.is_err() {
                        break;
                    }
                }
                tracing::trace!("Minecraft stdout capture task ended");
            });
        }

        if let Some(stderr) = child.stderr.take() {
            let w = log_writer.clone();
            let tx = log_tx.clone();
            let mut lines = tokio::io::BufReader::new(stderr).lines();
            tokio::spawn(async move {
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Ok(mut f) = w.lock()
                        && let Some(f) = f.as_mut()
                    {
                        let _ = writeln!(f, "{}", line);
                    }
                    if tx.send((LogStream::Stderr, line)).await.is_err() {
                        break;
                    }
                }
                tracing::trace!("Minecraft stderr capture task ended");
            });
        }
        drop(log_tx);

        // wait for either the process to exit naturally or a kill signal from the TUI
        let (code, killed_by_user) = tokio::select! {
            _ = kill_rx => {
                tracing::info!("[{}] Kill requested, terminating process", name_for_task);
                let _ = child.kill().await;
                let _ = child.wait().await;
                (None, true)
            }
            result = child.wait() => {
                (result.ok().and_then(|s| s.code()), false)
            }
        };
        let _ = parser_task.await;
        tracing::info!("[{}] Exited with code {:?}", name_for_task, code);

        if let Err(error) = run_launch_command(
            "Post-exit",
            &post_exit_command,
            &config_for_post_exit,
            &invocation_for_post_exit,
            &instance_dir_owned,
        )
        .await
        {
            tracing::warn!("[{}] {}", name_for_task, error);
            crate::feedback::errors::push_message(tracing::Level::WARN, error.to_string());
        }

        finish_config_sync(
            config_sync_active,
            config_sync_profile.as_deref(),
            &meta_dir_owned,
            &minecraft_dir_owned,
            &name_for_task,
        );
        if code == Some(0) || killed_by_user {
            crate::instance::runtime::remove(&name_for_task);
            tracing::debug!(
                "[{}] Cleared running state after normal exit (killed_by_user={})",
                name_for_task,
                killed_by_user
            );
        } else {
            crate::instance::runtime::set_state(
                &name_for_task,
                crate::instance::runtime::RunState::Crashed(code),
            );
            crate::feedback::errors::push_error(crate::feedback::errors::ErrorEvent {
                id: 0,
                level: tracing::Level::ERROR,
                message: match code {
                    Some(code) => {
                        format!("Minecraft '{name_for_task}' crashed with exit code {code}")
                    }
                    None => format!("Minecraft '{name_for_task}' crashed without an exit code"),
                },
                pushed_at: std::time::Instant::now(),
            });
        }

        let manager = crate::instance::InstanceManager::new(instances_dir_owned, meta_dir_owned);
        if let Err(e) = manager.touch_last_played(&name_for_task) {
            tracing::warn!(
                "Failed to update last_played for '{}': {}",
                name_for_task,
                e
            );
        }
        crate::instance::runtime::push_last_played(&name_for_task, chrono::Utc::now());
        crate::instance::runtime::cleanup_kill_sender(&name_for_task);
    });

    Ok(())
}

fn emit_parsed_instance_log(
    instance_name: &str,
    event: crate::instance::launch::parser::ParsedLogEvent,
) {
    let text = event.lines.join("\n");
    match event.level {
        crate::instance::launch::parser::LogLevel::Error => {
            tracing::error!(target: "mc_instance", "[{}] {}", instance_name, text);
        }
        crate::instance::launch::parser::LogLevel::Warn => {
            tracing::warn!(target: "mc_instance", "[{}] {}", instance_name, text);
        }
        crate::instance::launch::parser::LogLevel::Info => {
            tracing::info!(target: "mc_instance", "[{}] {}", instance_name, text);
        }
        crate::instance::launch::parser::LogLevel::Debug => {
            tracing::debug!(target: "mc_instance", "[{}] {}", instance_name, text);
        }
        crate::instance::launch::parser::LogLevel::Trace => {
            tracing::trace!(target: "mc_instance", "[{}] {}", instance_name, text);
        }
    }
    crate::instance::logs::live::push_event(instance_name, event);
}

#[cfg(test)]
#[path = "../tests/launch/pipeline.rs"]
mod tests;
