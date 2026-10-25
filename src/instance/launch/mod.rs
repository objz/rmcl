// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

mod output;
pub(crate) mod parser;
mod patches;

use std::path::{Path, PathBuf};

use thiserror::Error;

use super::process;
use crate::auth::AccountType;
use crate::instance::java::JavaRuntime;
use crate::instance::models::{InstanceConfig, LaunchCommand, ModLoader, WindowMode};
use crate::launch_profile::model::{Argument, LaunchProfile};
use crate::launch_profile::rules::{self, FeatureSet, RuleAction, RuleContext};
use crate::launch_profile::templates::TemplateContext;
use crate::launch_profile::{render, resolve};

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
    #[error("Could not inspect Java runtime {java}: {reason}")]
    JavaCheckFailed {
        java: String,
        required: u32,
        reason: String,
    },
    #[error("Download error: {0}")]
    Download(#[from] crate::net::NetError),
    #[error("{0}")]
    Auth(String),
    #[error("{phase} command failed: {reason}")]
    Command { phase: &'static str, reason: String },
    #[error("Config sync error: {0}")]
    ConfigSync(#[from] crate::instance::config_sync::ConfigSyncError),
    #[error("Launch cancelled")]
    Cancelled,
}

struct LaunchCleanup(String);

impl Drop for LaunchCleanup {
    fn drop(&mut self) {
        crate::instance::runtime::cleanup_kill_sender(&self.0);
        if crate::instance::runtime::is_active(&self.0) {
            crate::instance::runtime::remove(&self.0);
        }
    }
}

async fn wait_for_kill(receiver: &mut Option<tokio::sync::oneshot::Receiver<()>>) {
    if let Some(pending) = receiver {
        let result = pending.await;
        *receiver = None;
        if result.is_ok() {
            return;
        }
    }
    std::future::pending::<()>().await;
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

fn check_java_version(
    java: &str,
    runtime: &JavaRuntime,
    required: Option<u32>,
) -> Result<(), LaunchError> {
    let Some(required) = required.filter(|major| *major > 0) else {
        return Ok(());
    };

    let detected = runtime.major_version;
    if detected < required {
        return Err(LaunchError::JavaTooOld {
            java: java.to_owned(),
            required,
            detected,
        });
    }

    Ok(())
}

// rmcl <= 0.3.0 stripped upstream arguments from cached metadata.
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

// Recover JVM arguments and rules stripped by rmcl <= 0.3.0 from the installer original.
async fn migrate_legacy_loader_profile_if_needed(
    profile_path: &Path,
    profile: &LaunchProfile,
    config: &InstanceConfig,
    instance_dir: &Path,
) -> Result<Option<LaunchProfile>, LaunchError> {
    // Fabric/Quilt profiles can omit these fields and have no installer original.
    if matches!(config.loader, ModLoader::Fabric | ModLoader::Quilt) {
        return Ok(None);
    }

    // gameArguments identifies rmcl's old shape, avoiding false positives on upstream profiles.
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

#[derive(Debug, Clone)]
pub struct LaunchAuth<'a> {
    pub username: &'a str,
    pub uuid: &'a str,
    pub token: &'a str,
    // "msa" for Microsoft, "legacy" for offline; mirrors Mojang's user_type.
    pub user_type: &'a str,
}

// Exposed so integration tests can inspect the invocation without spawning Java.
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

pub async fn build_launch_invocation(
    config: &InstanceConfig,
    instances_dir: &Path,
    meta_dir: &Path,
    auth: &LaunchAuth<'_>,
    quick_play_world: Option<&str>,
) -> Result<LaunchInvocation, LaunchError> {
    let settings = crate::config::SETTINGS.read().clone();
    build_launch_invocation_with_settings(
        config,
        instances_dir,
        meta_dir,
        auth,
        quick_play_world,
        &settings,
    )
    .await
}

pub async fn build_launch_invocation_with_settings(
    config: &InstanceConfig,
    instances_dir: &Path,
    meta_dir: &Path,
    auth: &LaunchAuth<'_>,
    quick_play_world: Option<&str>,
    settings: &crate::config::Config,
) -> Result<LaunchInvocation, LaunchError> {
    let meta_dir = std::path::absolute(meta_dir)?;
    let meta_dir = meta_dir.as_path();
    let instance_dir = std::path::absolute(instances_dir.join(&config.name))?;
    let minecraft_dir = instance_dir.join(crate::storage::MINECRAFT_DIR_NAME);
    let (window_mode, resolution) = {
        let defaults = &settings.defaults;
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
    let lib_dir = metadata_paths.libraries();

    let lv = config.loader_version.as_deref().unwrap_or("unknown");
    let profile_filename =
        crate::instance::loader::profile_filename(config.loader, &config.game_version, lv);

    // load the loader profile (if any), migrate from the old stripped format
    // if needed, and resolve `inheritsFrom` against the vanilla parent (which
    // the vanilla meta migration above ensured is fresh on disk). when no
    // loader is configured we use the already-loaded vanilla meta directly.
    let mut merged_profile: LaunchProfile = if let Some(filename) = &profile_filename {
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

    let mut main_class = merged_profile
        .main_class
        .clone()
        .ok_or_else(|| LaunchError::Parse("merged profile missing mainClass".into()))?;

    if quick_play_world.is_some() && !profile_supports_quick_play(&merged_profile) {
        return Err(LaunchError::NotSupported("Quick Play".to_owned()));
    }

    let environment = crate::instance::java::merge_environment(
        &settings.defaults.environment,
        &config.environment,
    );
    let java = crate::instance::java::resolve_java_path_in(
        config
            .java_path
            .as_deref()
            .or(settings.paths.effective_java_path()),
        &minecraft_dir,
        &environment,
    );
    let runtime =
        crate::instance::java::probe_java_in(Path::new(&java), &minecraft_dir, &environment)
            .await
            .map_err(|error| LaunchError::JavaCheckFailed {
                java: java.clone(),
                required: merged_profile
                    .java_version
                    .as_ref()
                    .map_or(0, |version| version.major_version),
                reason: error.to_string(),
            })?;
    let platform = &runtime.platform;
    let patches = if config.loader == ModLoader::Forge {
        patches::load(&minecraft_dir, platform)?
    } else {
        None
    };
    if let Some(patches) = &patches {
        merged_profile = resolve::merge_into(patches.profile.clone(), merged_profile);
        // The embedded version.json is a complete client profile. Appending
        // the old Forge arguments would run its tweakers a second time.
        merged_profile.arguments = patches.profile.arguments.clone();
        merged_profile.inherits_from = None;
        patches::strip_replaced_libs(&mut merged_profile);
        main_class = merged_profile
            .main_class
            .clone()
            .expect("validated patch main class");
    }
    check_java_version(
        &java,
        &runtime,
        merged_profile
            .java_version
            .as_ref()
            .map(|version| version.major_version),
    )?;
    let rule_ctx = RuleContext {
        os_name: platform.os_name,
        os_version: &platform.os_version,
        arch: &platform.arch,
        features: &current_features,
    };
    let asset_index_id = merged_profile
        .asset_index
        .as_ref()
        .map(|index| index.id.clone())
        .unwrap_or_default();
    let natives_dir = platform.natives_directory(meta_dir, &config.game_version);

    // Forge installers can place libraries in the instance instead of the shared cache.
    let has_local_libs =
        matches!(config.loader, ModLoader::Forge | ModLoader::NeoForge) && patches.is_none();
    let local_lib_dir = minecraft_dir.join("libraries");
    let library_directory = if has_local_libs {
        &local_lib_dir
    } else {
        &lib_dir
    };

    let mut to_download = Vec::new();
    for library in &merged_profile.libraries {
        if library
            .rules
            .as_ref()
            .is_some_and(|conditions| !rules::evaluate(conditions, &rule_ctx))
        {
            continue;
        }
        // Installer-owned artifacts are already in the instance's library directory.
        let local = has_local_libs
            && crate::net::mojang::library_artifact_path(library)?
                .is_some_and(|path| local_lib_dir.join(path).is_file());
        let mut library = library.clone();
        if local && let Some(downloads) = &mut library.downloads {
            downloads.artifact = None;
        }
        to_download.push(library);
    }
    crate::net::mojang::download_profile_libraries(
        &crate::net::HttpClient::new(),
        &to_download,
        &lib_dir,
        &natives_dir,
        platform,
        &current_features,
    )
    .await?;

    let mut classpath: Vec<PathBuf> = Vec::new();
    for lib in &merged_profile.libraries {
        if let Some(rules) = &lib.rules
            && !rules::evaluate(rules, &rule_ctx)
        {
            continue;
        }

        let Some(rel) = crate::net::mojang::library_artifact_path(lib)? else {
            continue;
        };

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

    let (main_class, extra_args, patch_jvm_args) = if let Some(patches) = patches {
        patches::apply(patches, &minecraft_dir, &lib_dir, &mut classpath).await?
    } else {
        (main_class, Vec::new(), Vec::new())
    };

    let sep = if cfg!(windows) { ";" } else { ":" };
    let cp_str = classpath
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(sep);

    let assets_root = metadata_paths.assets();
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

    let (memory_min, memory_max, global_jvm_args) = {
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
        )
    };
    let mut jvm_args: Vec<String> = vec![format!("-Xms{memory_min}"), format!("-Xmx{memory_max}")];
    if merged_profile.arguments.is_none() {
        jvm_args.push(format!("-Djava.library.path={}", natives_dir.display()));
    }
    jvm_args.extend(patch_jvm_args);
    jvm_args.extend(upstream_jvm_args);
    jvm_args.extend(global_jvm_args);
    jvm_args.extend(config.jvm_args.clone());
    if let Some(glfw_path) = config.glfw_path.as_deref() {
        jvm_args.push(format!("-Dorg.lwjgl.glfw.libname={glfw_path}"));
    }

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
        let mut process = tokio::process::Command::new("cmd.exe");
        // cmd parses shell text rather than CRT argv. /S removes this outer pair
        // of quotes; raw_arg preserves all quotes and metacharacters inside it.
        process
            .args(["/D", "/S", "/C"])
            .raw_arg(format!("\"{command}\""));
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
        .env("INST_DIR", std::path::absolute(instance_dir)?)
        .env("INST_MC_DIR", &invocation.working_dir)
        .env("INST_JAVA", &invocation.java)
        .env("INST_JAVA_ARGS", invocation.jvm_args.join(" "))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let (mut child, tree) =
        process::spawn_owned(&mut process).map_err(|error| LaunchError::Command {
            phase,
            reason: error.to_string(),
        })?;
    let (sender, mut receiver) = tokio::sync::mpsc::channel(64);
    let mut readers = tokio::task::JoinSet::new();
    if let Some(stdout) = child.stdout.take() {
        readers.spawn(output::capture(
            stdout,
            parser::LogStream::Stdout,
            sender.clone(),
            Default::default(),
        ));
    }
    if let Some(stderr) = child.stderr.take() {
        readers.spawn(output::capture(
            stderr,
            parser::LogStream::Stderr,
            sender.clone(),
            Default::default(),
        ));
    }
    drop(sender);
    let log = |stream, line| match stream {
        parser::LogStream::Stdout => tracing::info!("[{}] [{}] {}", config.name, phase, line),
        parser::LogStream::Stderr => tracing::warn!("[{}] [{}] {}", config.name, phase, line),
    };
    let status = loop {
        tokio::select! {
            status = child.wait() => break status?,
            Some((stream, line)) = receiver.recv() => log(stream, line),
        }
    };
    let drain_deadline = tokio::time::sleep(std::time::Duration::from_secs(1));
    tokio::pin!(drain_deadline);
    loop {
        tokio::select! {
            line = receiver.recv() => match line { Some((stream, line)) => log(stream, line), None => break },
            _ = &mut drain_deadline => { tracing::warn!("[{phase}] Output pipes remained open after command exit"); break; },
        }
    }
    // Normal completion can intentionally leave a background command running.
    // Cancellation before this point still drops the armed process-tree guard.
    tree.detach().map_err(|error| LaunchError::Command {
        phase,
        reason: error.to_string(),
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(LaunchError::Command {
            phase,
            reason: status.to_string(),
        })
    }
}

fn finish_config_sync(
    lock: Option<crate::instance::config_sync::ConfigSyncLock>,
    minecraft_dir: &Path,
    instance: &str,
) {
    if let Some(lock) = lock
        && let Err(error) = crate::instance::config_sync::finish_launch(lock, minecraft_dir)
    {
        tracing::warn!("Failed to sync config for '{}': {}", instance, error);
    }
}

pub async fn launch(
    config: &InstanceConfig,
    instances_dir: &Path,
    meta_dir: &Path,
    quick_play_world: Option<&str>,
) -> Result<(), LaunchError> {
    let name = config.name.clone();
    let mut instance_lock = crate::instance::runtime::lock_instance(instances_dir, &name)
        .map_err(|error| LaunchError::Parse(error.to_string()))?;
    let (kill_tx, kill_rx) = tokio::sync::oneshot::channel::<()>();
    let mut kill_rx = Some(kill_rx);
    crate::instance::runtime::register_kill(&name, kill_tx);
    let launch_cleanup = LaunchCleanup(name.clone());
    crate::instance::runtime::set_state(&name, crate::instance::runtime::RunState::Authenticating);

    let mut account_store = crate::auth::AccountStore::load();
    account_store
        .check_loaded()
        .map_err(|error| LaunchError::Auth(error.to_string()))?;
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
        AccountType::Microsoft => match tokio::select! {
            result = crate::auth::refresh_and_get_token(&acc) => result,
            _ = wait_for_kill(&mut kill_rx) => return Err(LaunchError::Cancelled),
        } {
            Ok(triple) => triple,
            Err(e) => return Err(LaunchError::Auth(format!("Authentication failed: {e}"))),
        },
        AccountType::Offline => ("0".to_string(), None, None),
    };

    account_store.update_credentials(&acc.uuid, new_refresh, &token, new_expires)?;

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

    let invocation = tokio::select! {
        result = build_launch_invocation(config, instances_dir, meta_dir, &auth, quick_play_world) => result?,
        _ = wait_for_kill(&mut kill_rx) => return Err(LaunchError::Cancelled),
    };
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
    let config_sync_lock = crate::instance::config_sync::prepare_for_launch(
        config.config_sync_profile.as_deref(),
        meta_dir,
        &invocation.working_dir,
    )?;
    if let Err(error) = tokio::select! {
        result = run_launch_command(
        "Pre-launch",
        &config.pre_launch_command,
        config,
        &invocation,
        &instance_dir,
        ) => result,
        _ = wait_for_kill(&mut kill_rx) => Err(LaunchError::Cancelled),
    } {
        finish_config_sync(config_sync_lock, &invocation.working_dir, &name);
        return Err(error);
    }

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
    cmd.kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            crate::instance::runtime::cleanup_kill_sender(&name);
            crate::instance::runtime::remove(&name);
            finish_config_sync(config_sync_lock, &invocation.working_dir, &name);
            tracing::error!("[{}] Failed to spawn Minecraft process: {}", name, e);
            return Err(LaunchError::Io(e));
        }
    };
    tracing::debug!("[{}] Spawned Minecraft process", name);

    if let Err(error) = instance_lock.allow_content_changes() {
        finish_config_sync(config_sync_lock, &invocation.working_dir, &name);
        return Err(error.into());
    }
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

    tokio::spawn(async move {
        let _instance_lock = instance_lock;
        let _launch_cleanup = launch_cleanup;
        use std::sync::{Arc, Mutex};
        use tokio::sync::mpsc;
        use tokio::time::Duration;

        use crate::instance::launch::parser::{LogStream, MinecraftLogParser};

        let log_writer: Arc<Mutex<Option<std::fs::File>>> = Arc::new(Mutex::new(
            log_file_path.and_then(|p| std::fs::File::create(p).ok()),
        ));

        let (log_tx, mut log_rx) = mpsc::channel::<(LogStream, String)>(1024);
        let parser_name = name_for_task.clone();
        let parser_task = tokio::spawn(async move {
            let mut parser = MinecraftLogParser::new();
            let idle_flush = Duration::from_millis(150);
            let mut flush_interval = tokio::time::interval(idle_flush);
            flush_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            flush_interval.tick().await;

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
                    _ = flush_interval.tick(), if parser.has_pending() => {
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

        let mut readers = tokio::task::JoinSet::new();
        if let Some(stdout) = child.stdout.take() {
            readers.spawn(output::capture(
                stdout,
                LogStream::Stdout,
                log_tx.clone(),
                log_writer.clone(),
            ));
        }

        if let Some(stderr) = child.stderr.take() {
            readers.spawn(output::capture(
                stderr,
                LogStream::Stderr,
                log_tx.clone(),
                log_writer.clone(),
            ));
        }
        drop(log_tx);

        let (code, killed_by_user) = tokio::select! {
            _ = wait_for_kill(&mut kill_rx) => {
                tracing::info!("[{}] Kill requested, terminating process", name_for_task);
                let _ = child.kill().await;
                let _ = child.wait().await;
                (None, true)
            }
            result = child.wait() => {
                (result.ok().and_then(|s| s.code()), false)
            }
        };
        if tokio::time::timeout(Duration::from_secs(1), async {
            while readers.join_next().await.is_some() {}
        })
        .await
        .is_err()
        {
            tracing::warn!("Minecraft output pipes remained open after process exit");
            readers.abort_all();
            while readers.join_next().await.is_some() {}
        }
        let _ = parser_task.await;
        tracing::info!("[{}] Exited with code {:?}", name_for_task, code);

        if !killed_by_user
            && let Err(error) = tokio::select! {
                result = run_launch_command(
                "Post-exit",
                &post_exit_command,
                &config_for_post_exit,
                &invocation_for_post_exit,
                &instance_dir_owned,
                ) => result,
                _ = wait_for_kill(&mut kill_rx) => Err(LaunchError::Cancelled),
            }
        {
            tracing::warn!("[{}] {}", name_for_task, error);
            crate::feedback::errors::push_message(tracing::Level::WARN, error.to_string());
        }

        finish_config_sync(config_sync_lock, &minecraft_dir_owned, &name_for_task);
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
