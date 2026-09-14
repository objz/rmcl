// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use color_eyre::eyre::Context;
use crossterm::event::{self, Event};
use ratatui::{
    buffer::{Buffer, CellDiffOption},
    crossterm::event::KeyEventKind,
};
use std::time::Duration;

use super::Tui;
use super::app::{
    App, COMPLETED_INSTANCE_SETTINGS_UPDATES, FAILED_INSTANCE_SETTINGS_UPDATES, FocusedArea,
    PENDING_INSTANCES, RUNTIME_UPDATE_PENDING_MESSAGE,
};
use super::widgets::{self, popups::import_modpack, popups::new_instance};
use crate::feedback::errors as error_buffer;
use crate::feedback::progress;
use crate::instance::InstanceManager;

impl App {
    /// main loop: poll async results and input at ~60Hz, drawing only when state changes
    pub async fn run(&mut self, terminal: &mut Tui) -> color_eyre::Result<()> {
        let mut last_draw = std::time::Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(std::time::Instant::now);
        let mut drawn_overlay_count = self.overlay_count();
        let mut drawn_image_skips = Vec::new();
        let mut image_redraw_marker = false;
        while !self.exit {
            let redraw_requested = crate::feedback::take_redraw_request();
            // check if any popup wizard finished and wants to create/import
            if let Some(params) = new_instance::take_result() {
                self.spawn_create(params);
            }

            if let Some(result) = import_modpack::take_result() {
                self.spawn_import(result);
            }
            import_modpack::drain(&self.picker);

            self.dismiss_expired_errors();

            // drain all the channels from background tasks.
            // every content type has its own pending queue because they each
            // get scanned/loaded on separate tokio tasks
            self.drain_pending_instances();
            self.drain_completed_instance_settings_updates();
            self.drain_failed_instance_settings_updates();
            self.instances_state.drain_modpack_updates();
            self.drain_pending_last_played();
            if let Some(state) = self.modpack_versions_state.as_mut() {
                state.drain_pending();
            }
            let content_update_completed = self.content_update_popup.as_mut().and_then(|update| {
                update.drain();
                update.list.request_image_loads(&self.picker);
                update.list.drain_image_loads(&self.picker);
                update.completed.then_some(update.applied)
            });
            if let Some(applied) = content_update_completed {
                self.content_update_popup = None;
                if applied {
                    self.content_update_snapshot = None;
                    self.apply_content_update_snapshot();
                }
            }
            let completed_modpack = self.modpack_update_popup.as_mut().and_then(|update| {
                update.drain();
                update.completed.take()
            });
            if let Some(instance) = completed_modpack {
                let name = instance.name.clone();
                self.instances_state
                    .replace_instance(&name, instance.clone());
                self.instances_state.modpack_updates.remove(&name);
                widgets::instances::spawn_modpack_update_check(&instance);
                self.modpack_update_popup = None;
                self.reconciliation_for = None;
                self.content_manifest = None;
                self.content_update_snapshot = None;
            }
            let mut local_streamed = false;
            let mut content_changed = false;
            let mut toggles = Vec::new();
            let mut orphan_cleanup = None;
            for (local, discovery) in [
                (&mut self.mods_state, &mut self.mods_discovery_state),
                (
                    &mut self.resource_packs_state,
                    &mut self.resource_packs_discovery_state,
                ),
                (&mut self.shaders_state, &mut self.shaders_discovery_state),
            ] {
                local_streamed |= local.drain_pending();
                let update = local.drain_watcher();
                content_changed |= update.requires_reconcile;
                toggles.extend(update.toggles);
                local.drain_provider_icons();
                local.request_image_loads(&self.picker);
                local.drain_image_loads(&self.picker);
                discovery.drain_pending();
                if self.focused == FocusedArea::Content {
                    orphan_cleanup = orphan_cleanup.or_else(|| discovery.take_orphan_cleanup());
                }
                discovery.list.drain_pending();
                discovery.list.request_image_loads(&self.picker);
                discovery.list.drain_image_loads(&self.picker);
            }
            self.datapacks_discovery_state.drain_pending();
            if self.focused == FocusedArea::Content {
                orphan_cleanup =
                    orphan_cleanup.or_else(|| self.datapacks_discovery_state.take_orphan_cleanup());
            }
            self.datapacks_discovery_state.list.drain_pending();
            self.datapacks_discovery_state
                .list
                .request_image_loads(&self.picker);
            self.datapacks_discovery_state
                .list
                .drain_image_loads(&self.picker);
            if let Some(popup) = self.datapacks_discovery_state.version_popup.as_mut() {
                popup.worlds.request_image_loads(&self.picker);
                popup.worlds.drain_image_loads(&self.picker);
            }
            local_streamed |= self.world_datapacks_state.drain_pending();
            let update = self.world_datapacks_state.drain_watcher();
            content_changed |= update.requires_reconcile;
            toggles.extend(update.toggles);
            self.world_datapacks_state.drain_provider_icons();
            self.world_datapacks_state.request_image_loads(&self.picker);
            self.world_datapacks_state.drain_image_loads(&self.picker);
            if let Some(paths) = orphan_cleanup {
                widgets::popups::confirm::set_pending_orphan_dependencies(paths);
                self.focused = FocusedArea::ConfirmDelete;
            }
            self.worlds_state.drain_pending();
            self.worlds_state.drain_watcher();
            self.worlds_state.request_image_loads(&self.picker);
            self.worlds_state.drain_image_loads(&self.picker);
            self.logs_state.drain_pending();
            self.logs_state.try_rescan();
            self.account_state.drain_auth_result();
            widgets::account::drain_device_code(&mut self.account_state);
            self.screenshots_state.drain_pending_entries();
            self.screenshots_state.request_visible_loads();
            self.create_screenshot_protocols();
            if !toggles.is_empty() {
                content_changed |= self.persist_content_toggles(&toggles);
            }
            if content_changed
                && let Some(instance) = self.instances_state.selected_instance()
                && let Ok(mut results) =
                    crate::instance::content::reconcile::PENDING_RECONCILIATIONS.lock()
            {
                results.retain(|result| result.instance_name != instance.name);
            }
            self.drain_content_reconciliation();
            self.drain_content_update_snapshots();
            self.ensure_content_reconciliation(content_changed);
            if local_streamed {
                self.apply_cached_content_manifest();
            }
            self.ensure_provider_conflict_popup();
            self.ensure_active_discovery_loaded();
            let progress_active = progress::is_active();
            let spinner_active = progress_active || crate::instance::runtime::has_active();
            if spinner_active {
                // only advance the spinner every 8 ticks to keep it readable
                self.throbber_tick = self.throbber_tick.wrapping_add(1);
                if self.throbber_tick.is_multiple_of(8) {
                    self.throbber_state.calc_next();
                }
            }

            let input_changed = self.handle_events().wrap_err("handle events failed")?;
            let overlay_count = self.overlay_count();
            let overlay_closed = overlay_count < drawn_overlay_count;
            let continuously_animated = spinner_active || error_buffer::has_errors();
            let safety_refresh = last_draw.elapsed() >= Duration::from_secs(1);
            if input_changed
                || continuously_animated
                || safety_refresh
                || redraw_requested
                || overlay_closed
            {
                let mut image_skips = Vec::new();
                terminal.draw(|frame| {
                    self.render_frame(frame);
                    image_skips = terminal_image_skips(frame.buffer_mut());
                    if terminal_image_cells_changed(&drawn_image_skips, &image_skips) {
                        image_redraw_marker = !image_redraw_marker;
                    }
                    mark_terminal_images(frame.buffer_mut(), image_redraw_marker);
                })?;
                last_draw = std::time::Instant::now();
                drawn_overlay_count = overlay_count;
                drawn_image_skips = image_skips;
            }

            if let Some(path) = self.pending_editor.take()
                && Self::run_editor(terminal, &path)
            {
                self.reload_edited_config(&path);
            }
        }
        Ok(())
    }

    fn overlay_count(&self) -> usize {
        error_buffer::peek_all_errors().len()
            + usize::from(self.instances_state.show_popup)
            + usize::from(self.instances_state.show_import_popup)
            + usize::from(self.focused == super::app::FocusedArea::OverviewExpanded)
            + usize::from(self.focused == super::app::FocusedArea::ConfirmDelete)
            + usize::from(self.focused == super::app::FocusedArea::InstanceSettings)
            + usize::from(self.focused == super::app::FocusedArea::GlobalSettings)
            + usize::from(self.provider_conflict.is_some())
            + usize::from(
                self.content_update_popup
                    .as_ref()
                    .is_some_and(widgets::content::update::State::visible),
            )
            + usize::from(self.modpack_update_popup.is_some())
            + usize::from(self.modpack_versions_state.is_some())
            + usize::from(!matches!(
                &self.account_state.add_mode,
                widgets::account::AddMode::None
            ))
            + usize::from(!matches!(
                &self.settings_state.add_mode,
                widgets::settings::AddMode::None
            ))
            + [
                &self.mods_discovery_state,
                &self.resource_packs_discovery_state,
                &self.shaders_discovery_state,
                &self.datapacks_discovery_state,
            ]
            .iter()
            .filter(|state| state.version_popup.is_some())
            .count()
            + usize::from(import_modpack::has_version_popup())
    }

    fn persist_content_toggles(
        &mut self,
        toggles: &[widgets::content::list::ContentToggle],
    ) -> bool {
        let Some(instance) = self.instances_state.selected_instance() else {
            return true;
        };
        let instance_name = instance.name.clone();
        let paths = crate::storage::InstancePaths::new(
            self.instance_manager.instances_dir.join(&instance_name),
        );
        let minecraft_dir = paths.minecraft();
        let updated =
            crate::instance::ContentManifest::update(&paths.content_manifest(), |manifest| {
                let mut complete = true;
                for toggle in toggles {
                    let Ok(old_path) = toggle.old_path.strip_prefix(&minecraft_dir) else {
                        complete = false;
                        continue;
                    };
                    let Ok(new_path) = toggle.new_path.strip_prefix(&minecraft_dir) else {
                        complete = false;
                        continue;
                    };
                    complete &= manifest.rename_record(old_path, new_path, toggle.enabled);
                }
                Ok((manifest.clone(), complete))
            });
        let (manifest, complete) = match updated {
            Ok(updated) => updated,
            Err(error) => {
                tracing::warn!("Failed to update toggled content metadata: {error}");
                return true;
            }
        };

        self.mods_discovery_state
            .refresh_installed_manifest(&manifest, &minecraft_dir);
        self.resource_packs_discovery_state
            .refresh_installed_manifest(&manifest, &minecraft_dir);
        self.shaders_discovery_state
            .refresh_installed_manifest(&manifest, &minecraft_dir);
        self.datapacks_discovery_state
            .refresh_installed_manifest(&manifest, &minecraft_dir);
        self.content_manifest = Some((instance_name, manifest));
        !complete
    }

    fn ensure_content_reconciliation(&mut self, changed: bool) {
        let Some(instance) = self.instances_state.selected_instance().cloned() else {
            self.reconciliation_for = None;
            self.content_manifest = None;
            return;
        };
        let instance_id = (instance.name.clone(), instance.created);
        if !changed && self.reconciliation_for.as_ref() == Some(&instance_id) {
            return;
        }
        let instance_changed = self.reconciliation_for.as_ref() != Some(&instance_id);
        if instance_changed {
            self.provider_conflict = None;
            self.dismissed_provider_conflicts.clear();
            self.content_manifest = None;
            self.content_update_snapshot = None;
            for discovery in [
                &mut self.mods_discovery_state,
                &mut self.resource_packs_discovery_state,
                &mut self.shaders_discovery_state,
                &mut self.datapacks_discovery_state,
            ] {
                discovery.refresh_installed_manifest(
                    &crate::instance::ContentManifest::default(),
                    &crate::storage::InstancePaths::new(
                        self.instance_manager.instances_dir.join(&instance.name),
                    )
                    .minecraft(),
                );
            }
        }
        self.reconciliation_for = Some(instance_id);
        if changed {
            crate::instance::content::reconcile::spawn_after_change(
                instance,
                self.instance_manager.instances_dir.clone(),
                crate::net::HttpClient::new(),
            );
        } else {
            crate::instance::content::reconcile::spawn(
                instance,
                self.instance_manager.instances_dir.clone(),
                crate::net::HttpClient::new(),
            );
        }
    }

    fn drain_content_reconciliation(&mut self) {
        let Some(selected) = self.instances_state.selected_instance().cloned() else {
            return;
        };
        let result = match crate::instance::content::reconcile::PENDING_RECONCILIATIONS.lock() {
            Ok(mut results) => {
                results.retain(|result| {
                    result.instance_name != selected.name
                        || result.instance_created == selected.created
                });
                results
                    .iter()
                    .position(|result| {
                        result.instance_name == selected.name
                            && result.instance_created == selected.created
                    })
                    .map(|index| results.remove(index))
            }
            Err(_) => return,
        };
        let Some(result) = result else {
            return;
        };
        self.reconciliation_for = Some((result.instance_name.clone(), result.instance_created));
        if let Some(error) = result.error {
            tracing::warn!(
                "Content reconciliation for {} was incomplete: {}",
                result.instance_name,
                error
            );
        }
        let minecraft_dir = crate::storage::InstancePaths::new(
            self.instance_manager.instances_dir.join(&selected.name),
        )
        .minecraft();
        self.mods_state.apply_manifest(
            &result.manifest,
            &minecraft_dir,
            crate::instance::ContentKind::Mod,
        );
        self.resource_packs_state.apply_manifest(
            &result.manifest,
            &minecraft_dir,
            crate::instance::ContentKind::ResourcePack,
        );
        self.shaders_state.apply_manifest(
            &result.manifest,
            &minecraft_dir,
            crate::instance::ContentKind::Shader,
        );
        self.world_datapacks_state.apply_manifest(
            &result.manifest,
            &minecraft_dir,
            crate::instance::ContentKind::DataPack,
        );
        self.mods_discovery_state
            .refresh_installed_manifest(&result.manifest, &minecraft_dir);
        self.resource_packs_discovery_state
            .refresh_installed_manifest(&result.manifest, &minecraft_dir);
        self.shaders_discovery_state
            .refresh_installed_manifest(&result.manifest, &minecraft_dir);
        self.datapacks_discovery_state
            .refresh_installed_manifest(&result.manifest, &minecraft_dir);
        for world in &mut self.worlds_state.entries {
            if let Some(details) = world.world_details.as_mut() {
                details.datapacks = crate::instance::content::worlds::datapack_names(&world.path);
            }
        }
        let paths = crate::storage::InstancePaths::new(
            self.instance_manager
                .instances_dir
                .join(&result.instance_name),
        );
        let cached =
            crate::instance::content::updates::UpdateSnapshot::load(&paths.content_updates())
                .filter(|snapshot| snapshot.applies_to(&selected));
        let stale = cached
            .as_ref()
            .is_none_or(|snapshot| snapshot.is_stale(&result.manifest));
        self.content_update_snapshot =
            cached.map(|snapshot| (result.instance_name.clone(), snapshot));
        self.content_manifest = Some((result.instance_name.clone(), result.manifest.clone()));
        self.apply_content_update_snapshot();
        if stale && crate::config::SETTINGS.read().general.check_content_updates {
            crate::instance::content::updates::spawn(
                selected,
                result.manifest,
                paths.content_updates(),
            );
        }
    }

    fn apply_cached_content_manifest(&mut self) {
        let Some((instance_name, manifest)) = &self.content_manifest else {
            return;
        };
        if self
            .instances_state
            .selected_instance()
            .is_none_or(|instance| instance.name != *instance_name)
        {
            return;
        }
        let minecraft_dir = crate::storage::InstancePaths::new(
            self.instance_manager.instances_dir.join(instance_name),
        )
        .minecraft();
        self.mods_state
            .apply_manifest(manifest, &minecraft_dir, crate::instance::ContentKind::Mod);
        self.resource_packs_state.apply_manifest(
            manifest,
            &minecraft_dir,
            crate::instance::ContentKind::ResourcePack,
        );
        self.shaders_state.apply_manifest(
            manifest,
            &minecraft_dir,
            crate::instance::ContentKind::Shader,
        );
        self.world_datapacks_state.apply_manifest(
            manifest,
            &minecraft_dir,
            crate::instance::ContentKind::DataPack,
        );
        self.apply_content_update_snapshot();
    }

    fn drain_content_update_snapshots(&mut self) {
        let Some(selected) = self.instances_state.selected_instance() else {
            return;
        };
        let snapshot = match crate::instance::content::updates::PENDING_UPDATE_SNAPSHOTS.lock() {
            Ok(mut pending) => {
                let latest = pending
                    .iter()
                    .rposition(|pending| {
                        pending.instance_name == selected.name
                            && pending.instance_created == selected.created
                    })
                    .map(|index| pending.remove(index));
                // every scan also writes its result to disk, so whatever is left
                // here is recovered on the next reconciliation of that instance
                pending.clear();
                latest
            }
            Err(_) => return,
        };
        let Some(snapshot) = snapshot else {
            return;
        };
        self.content_update_snapshot = Some((snapshot.instance_name, snapshot.snapshot));
        self.apply_content_update_snapshot();
    }

    fn apply_content_update_snapshot(&mut self) {
        // matched per entry by exact installed version, so a snapshot that no
        // longer covers the whole manifest still labels everything it does cover
        let snapshot = self
            .content_update_snapshot
            .as_ref()
            .and_then(|(name, snapshot)| {
                self.instances_state
                    .selected_instance()
                    .filter(|instance| instance.name == *name && snapshot.applies_to(instance))
                    .map(|_| snapshot)
            });
        self.mods_state.apply_update_snapshot(snapshot);
        self.resource_packs_state.apply_update_snapshot(snapshot);
        self.shaders_state.apply_update_snapshot(snapshot);
        self.world_datapacks_state.apply_update_snapshot(snapshot);
    }

    fn ensure_provider_conflict_popup(&mut self) {
        if !crate::config::SETTINGS
            .read()
            .content
            .ask_on_provider_conflict
            || self.focused != super::app::FocusedArea::Content
            || self.provider_conflict.is_some()
        {
            return;
        }
        let Some((instance_name, manifest)) = &self.content_manifest else {
            return;
        };
        if self
            .instances_state
            .selected_instance()
            .is_none_or(|instance| instance.name != *instance_name)
        {
            return;
        }
        self.provider_conflict = manifest.files.iter().find_map(|record| {
            if self
                .dismissed_provider_conflicts
                .contains(&record.relative_path)
            {
                return None;
            }
            let crate::instance::Resolution::Ambiguous { candidates } = &record.resolution else {
                return None;
            };
            Some(super::app::ProviderConflictState {
                relative_path: record.relative_path.clone(),
                candidates: candidates.clone(),
                selected: 0,
            })
        });
    }

    // polls for input with a 16ms timeout (~60fps). only key presses are handled,
    // releases and repeats are ignored thanks to the enhanced keyboard protocol
    fn handle_events(&mut self) -> color_eyre::Result<bool> {
        match crossterm::event::poll(Duration::from_millis(16)) {
            Ok(true) => match event::read() {
                Ok(Event::Key(key_event)) if key_event.kind == KeyEventKind::Press => {
                    self.handle_key_event(key_event)
                        .wrap_err_with(|| format!("handling key event failed:\n{key_event:#?}"))?;
                    Ok(true)
                }
                Ok(Event::Mouse(mouse_event)) => {
                    self.handle_mouse_event(mouse_event);
                    Ok(true)
                }
                Ok(_) => Ok(true),
                Err(e) => {
                    tracing::error!("Event read error: {}", e);
                    Ok(false)
                }
            },
            Ok(false) => Ok(false),
            Err(e) => {
                tracing::error!("Event poll error: {}", e);
                Ok(false)
            }
        }
    }

    fn spawn_create(&self, params: new_instance::WizardParams) {
        let instances_dir = self.instance_manager.instances_dir.clone();
        let meta_dir = crate::config::SETTINGS.read().paths.resolve_meta_dir();
        let pending_instances = PENDING_INSTANCES.clone();

        tokio::spawn(async move {
            progress::set_action(format!("Creating instance '{}'...", params.name));
            progress::set_sub_action(format!("{} {}", params.game_version, params.loader));

            let manager = InstanceManager::new(instances_dir, meta_dir);
            match manager
                .create(
                    &params.name,
                    &params.game_version,
                    params.loader,
                    params.loader_version.as_deref(),
                )
                .await
            {
                Ok(config) => {
                    if let Ok(mut pending) = pending_instances.lock() {
                        pending.push(config);
                        crate::feedback::request_redraw();
                    }
                }
                Err(e) => {
                    progress::clear();
                    error_buffer::push_error(error_buffer::ErrorEvent {
                        id: 0,
                        level: tracing::Level::ERROR,
                        message: format!("Failed to create instance '{}': {e}", params.name),
                        pushed_at: std::time::Instant::now(),
                    });
                }
            }
        });
    }

    pub(super) fn spawn_instance_settings_update(
        &self,
        previous: crate::instance::InstanceConfig,
        mut updated: crate::instance::InstanceConfig,
        desktop: bool,
    ) {
        let instances_dir = self.instance_manager.instances_dir.clone();
        let meta_dir = self.instance_manager.meta_dir.clone();
        let completed_updates = COMPLETED_INSTANCE_SETTINGS_UPDATES.clone();

        tokio::spawn(async move {
            progress::set_action(format!("Updating instance '{}'...", updated.name));
            progress::set_sub_action(format!("{} {}", updated.game_version, updated.loader));
            let manager = InstanceManager::new(&instances_dir, &meta_dir);
            updated = match apply_instance_settings_update(&manager, &previous, updated).await {
                Ok(updated) => updated,
                Err(error) => {
                    progress::clear();
                    if let Ok(mut failed) = FAILED_INSTANCE_SETTINGS_UPDATES.lock() {
                        failed.push(previous.name.clone());
                    }
                    error_buffer::push_error(error_buffer::ErrorEvent {
                        id: 0,
                        level: tracing::Level::ERROR,
                        message: format!("Failed to update instance '{}': {error}", previous.name),
                        pushed_at: std::time::Instant::now(),
                    });
                    return;
                }
            };

            let shortcut_result = crate::instance::desktop::set_enabled(&updated, desktop);
            if let Err(error) = shortcut_result {
                error_buffer::push_error(error_buffer::ErrorEvent {
                    id: 0,
                    level: tracing::Level::ERROR,
                    message: format!("Instance saved, but shortcut update failed: {error}"),
                    pushed_at: std::time::Instant::now(),
                });
            }
            if let Ok(mut pending) = completed_updates.lock() {
                pending.push(updated);
            }
            progress::clear();
            error_buffer::push_error(error_buffer::ErrorEvent {
                id: 0,
                level: tracing::Level::INFO,
                message: format!("Updated instance '{}'", previous.name),
                pushed_at: std::time::Instant::now(),
            });
            crate::feedback::request_redraw();
        });
    }

    fn spawn_import(&self, result: import_modpack::ImportResult) {
        let instances_dir = self.instance_manager.instances_dir.clone();
        let meta_dir = crate::config::SETTINGS.read().paths.resolve_meta_dir();
        let pending_instances = PENDING_INSTANCES.clone();

        tokio::spawn(async move {
            let manager = InstanceManager::new(instances_dir, meta_dir);
            match crate::instance::import::execute_import(&result.summary, &manager).await {
                Ok(config) => {
                    if let Ok(mut pending) = pending_instances.lock() {
                        pending.push(config);
                        crate::feedback::request_redraw();
                    }
                }
                Err(e) => {
                    crate::feedback::progress::clear();
                    error_buffer::push_error(error_buffer::ErrorEvent {
                        id: 0,
                        level: tracing::Level::ERROR,
                        message: format!("Import failed: {e}"),
                        pushed_at: std::time::Instant::now(),
                    });
                }
            }
        });
    }

    // spawns $EDITOR/$VISUAL to edit a file. for terminal editors (vim, nano, etc)
    // gotta leave the alternate screen and restore it after, otherwise the
    // editor fights with ratatui for the terminal. GUI editors just get spawned detached.
    fn run_editor(terminal: &mut ratatui::DefaultTerminal, path: &std::path::Path) -> bool {
        use ratatui::crossterm::{
            ExecutableCommand,
            event::{DisableMouseCapture, EnableMouseCapture},
            terminal::{
                EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
            },
        };
        use std::io::stdout;

        let default_editor = if cfg!(windows) { "notepad" } else { "vi" };
        let editor = std::env::var("EDITOR")
            .or_else(|_| std::env::var("VISUAL"))
            .unwrap_or_else(|_| default_editor.to_owned());

        let is_tui_editor = editor_runs_in_terminal(&editor);

        if is_tui_editor {
            let _ = stdout().execute(DisableMouseCapture);
            let _ = stdout().execute(LeaveAlternateScreen);
            let _ = disable_raw_mode();

            let result = std::process::Command::new(&editor)
                .arg(path)
                .stdin(std::process::Stdio::inherit())
                .stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit())
                .status();

            let _ = stdout().execute(EnterAlternateScreen);
            let _ = stdout().execute(EnableMouseCapture);
            let _ = enable_raw_mode();
            let _ = terminal.clear();

            if let Err(e) = result {
                tracing::error!("Failed to open editor: {}", e);
                return false;
            }
            true
        } else {
            if let Err(e) = std::process::Command::new(&editor)
                .arg(path)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                tracing::error!("Failed to open editor: {}", e);
                return false;
            }
            false
        }
    }

    fn reload_edited_config(&mut self, path: &std::path::Path) {
        match path.file_name().and_then(|name| name.to_str()) {
            Some("config.toml") => {
                match crate::config::SETTINGS.reload() {
                    Ok(outcome) => {
                        if outcome.provider_changed {
                            self.reset_discovery_states();
                        }
                        if outcome.modpack_updates_enabled {
                            widgets::instances::spawn_modpack_update_checks(
                                &self.instances_state.instances,
                            );
                        }
                        if outcome.content_updates_enabled {
                            self.spawn_selected_content_update_check();
                        }
                        if outcome.restart_required {
                            error_buffer::push_error(error_buffer::ErrorEvent {
                                id: 0,
                                level: tracing::Level::INFO,
                                message: "Path and image protocol changes apply after restart"
                                    .to_owned(),
                                pushed_at: std::time::Instant::now(),
                            });
                        }
                    }
                    Err(error) => error_buffer::push_error(error_buffer::ErrorEvent {
                        id: 0,
                        level: tracing::Level::ERROR,
                        message: format!("Failed to reload config.toml: {error}"),
                        pushed_at: std::time::Instant::now(),
                    }),
                }
                return;
            }
            Some("theme.toml") => {
                if let Err(error) = crate::config::theme::reload_theme() {
                    error_buffer::push_error(error_buffer::ErrorEvent {
                        id: 0,
                        level: tracing::Level::ERROR,
                        message: format!("Failed to reload theme.toml: {error}"),
                        pushed_at: std::time::Instant::now(),
                    });
                }
                return;
            }
            Some("instance.json") => {}
            _ => return,
        }

        let Some(name) = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
        else {
            return;
        };

        match self.instance_manager.load_one(name) {
            Ok(config) => {
                self.instances_state.replace_instance(name, config);
            }
            Err(e) => {
                tracing::error!("Failed to reload edited instance '{}': {}", name, e);
                error_buffer::push_error(error_buffer::ErrorEvent {
                    id: 0,
                    level: tracing::Level::ERROR,
                    message: format!("Failed to reload edited instance '{name}': {e}"),
                    pushed_at: std::time::Instant::now(),
                });
            }
        }
    }

    pub(super) fn spawn_launch(
        &self,
        instance: crate::instance::InstanceConfig,
        quick_play_world: Option<String>,
    ) {
        use crate::instance::launch;
        use crate::instance::runtime;

        if self
            .pending_instance_settings_updates
            .contains(&instance.name)
        {
            error_buffer::push_message(tracing::Level::WARN, RUNTIME_UPDATE_PENDING_MESSAGE);
            return;
        }

        let instance = match self.instance_manager.load_one(&instance.name) {
            Ok(config) => config,
            Err(e) => {
                error_buffer::push_error(error_buffer::ErrorEvent {
                    id: 0,
                    level: tracing::Level::ERROR,
                    message: format!("Failed to load instance '{}': {e}", instance.name),
                    pushed_at: std::time::Instant::now(),
                });
                return;
            }
        };

        let can_launch = matches!(
            runtime::get(&instance.name),
            None | Some(runtime::RunState::Crashed(_))
        );
        if !can_launch {
            return;
        }
        runtime::remove(&instance.name);
        crate::instance::logs::live::clear(&instance.name);

        runtime::set_state(&instance.name, runtime::RunState::Authenticating);

        let instances_dir = self.instance_manager.instances_dir.clone();
        let meta_dir = self.instance_manager.meta_dir.clone();

        tokio::spawn(async move {
            if let Err(e) = launch::launch(
                &instance,
                &instances_dir,
                &meta_dir,
                quick_play_world.as_deref(),
            )
            .await
            {
                tracing::error!("Failed to launch '{}': {}", instance.name, e);
                runtime::remove(&instance.name);
            }
        });
    }

    // pops errors from the front of the queue once they've been visible long enough.
    // loops because multiple errors could expire in the same frame
    fn dismiss_expired_errors(&self) {
        use crate::config::SETTINGS;
        loop {
            match error_buffer::peek_error() {
                Some(event)
                    if event.pushed_at.elapsed().as_millis()
                        >= SETTINGS.read().ui.error_auto_dismiss_ms as u128 =>
                {
                    let _ = error_buffer::pop_error();
                }
                _ => break,
            }
        }
    }

    fn drain_pending_instances(&mut self) {
        let pending = PENDING_INSTANCES
            .lock()
            .map(|mut pending| pending.drain(..).collect::<Vec<_>>())
            .unwrap_or_default();
        for config in pending {
            self.apply_pending_instance(config, false);
        }
    }

    fn drain_completed_instance_settings_updates(&mut self) {
        let completed = COMPLETED_INSTANCE_SETTINGS_UPDATES
            .lock()
            .map(|mut pending| pending.drain(..).collect::<Vec<_>>())
            .unwrap_or_default();
        for config in completed {
            self.apply_pending_instance(config, true);
        }
    }

    fn apply_pending_instance(
        &mut self,
        config: crate::instance::InstanceConfig,
        settings_update: bool,
    ) {
        let config = self
            .instance_manager
            .load_one(&config.name)
            .unwrap_or(config);
        if settings_update {
            self.pending_instance_settings_updates.remove(&config.name);
            if let Some(state) = self.instance_settings.as_mut()
                && state.runtime_update_pending_for(&config.name)
            {
                let desktop = crate::instance::desktop::exists(&config.name);
                state.mark_saved(&config, desktop);
            }
        }
        self.forget_instance_content(&config.name);
        widgets::instances::spawn_modpack_update_check(&config);
        if self
            .instances_state
            .instances
            .iter()
            .any(|instance| instance.name == config.name)
        {
            let name = config.name.clone();
            self.instances_state.replace_instance(&name, config);
        } else {
            self.instances_state.add_instance(config);
        }
    }

    fn drain_failed_instance_settings_updates(&mut self) {
        let failed = FAILED_INSTANCE_SETTINGS_UPDATES
            .lock()
            .map(|mut failed| failed.drain(..).collect::<Vec<_>>())
            .unwrap_or_default();
        for name in failed {
            self.pending_instance_settings_updates.remove(&name);
            let remaining = self.instance_settings.as_mut().and_then(|state| {
                state
                    .runtime_update_pending_for(&name)
                    .then(|| state.cancel_runtime_change())
                    .flatten()
            });
            if let Some((updated, desktop)) = remaining {
                self.apply_instance_settings(*updated, desktop);
            }
        }
    }

    pub(super) fn forget_instance_content(&mut self, instance_name: &str) {
        for state in [
            &mut self.mods_state,
            &mut self.resource_packs_state,
            &mut self.shaders_state,
            &mut self.worlds_state,
            &mut self.world_datapacks_state,
        ] {
            state.forget_instance(instance_name);
        }
        if self.logs_state.loaded_for.as_deref() == Some(instance_name) {
            self.logs_state.loaded_for = None;
        }
        if self.screenshots_state.loaded_for.as_deref() == Some(instance_name) {
            self.screenshots_state.loaded_for = None;
        }
        self.reconciliation_for = None;
        self.content_manifest = None;
        self.content_update_snapshot = None;
        self.content_update_popup = None;
        self.provider_conflict = None;
        self.dismissed_provider_conflicts.clear();
        self.apply_content_update_snapshot();
    }

    fn drain_pending_last_played(&mut self) {
        for (name, time) in crate::instance::runtime::drain_last_played() {
            for inst in &mut self.instances_state.instances {
                if inst.name == name {
                    inst.last_played = Some(time);
                    break;
                }
            }
        }
    }

    pub(super) fn create_screenshot_protocols(&mut self) {
        let pending = self.screenshots_state.take_pending_images();
        for (idx, img) in pending {
            let proto = self.picker.new_resize_protocol(img);
            self.screenshots_state.set_protocol(idx, proto);
        }
    }
}

async fn apply_instance_settings_update(
    manager: &InstanceManager,
    previous: &crate::instance::InstanceConfig,
    mut updated: crate::instance::InstanceConfig,
) -> color_eyre::Result<crate::instance::InstanceConfig> {
    manager.repair_runtime_cache(&updated).await?;

    if crate::instance::runtime::is_active(&previous.name) {
        color_eyre::eyre::bail!("instance started while its runtime was being updated");
    }

    let current = manager.load_one(&previous.name)?;
    updated = merge_instance_settings(previous, &updated, current);
    manager.save(&updated)?;

    Ok(updated)
}

pub(super) fn merge_instance_settings(
    previous: &crate::instance::InstanceConfig,
    updated: &crate::instance::InstanceConfig,
    mut current: crate::instance::InstanceConfig,
) -> crate::instance::InstanceConfig {
    macro_rules! apply_changed {
        ($field:ident) => {
            if previous.$field != updated.$field {
                current.$field.clone_from(&updated.$field);
            }
        };
    }

    apply_changed!(game_version);
    apply_changed!(loader);
    apply_changed!(loader_version);
    apply_changed!(java_path);
    apply_changed!(memory_max);
    apply_changed!(memory_min);
    apply_changed!(jvm_args);
    apply_changed!(environment);
    apply_changed!(window_mode);
    apply_changed!(inherit_window_mode);
    apply_changed!(resolution);
    apply_changed!(inherit_resolution);
    apply_changed!(preferred_account);
    apply_changed!(pre_launch_command);
    apply_changed!(post_exit_command);
    apply_changed!(glfw_path);
    current
}

fn mark_terminal_images(buffer: &mut Buffer, alternate: bool) {
    // toggling an invisible suffix lets the normal cell diff redraw exposed
    // images before later popup cells, without clearing or repainting the screen
    let marker = if alternate {
        "\u{200b}\u{200b}"
    } else {
        "\u{200b}"
    };
    for cell in &mut buffer.content {
        if matches!(cell.diff_option, CellDiffOption::ForcedWidth(_))
            && cell.symbol().contains('\x1b')
        {
            let mut symbol = cell.symbol().to_owned();
            symbol.push_str(marker);
            cell.set_symbol(&symbol);
        }
    }
}

fn terminal_image_skips(buffer: &Buffer) -> Vec<bool> {
    buffer
        .content
        .iter()
        .map(|cell| matches!(cell.diff_option, CellDiffOption::Skip))
        .collect()
}

fn terminal_image_cells_changed(previous: &[bool], current: &[bool]) -> bool {
    !previous.is_empty() && previous != current
}

fn editor_runs_in_terminal(editor: &str) -> bool {
    let editor_name = std::path::Path::new(editor)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or(editor);
    matches!(
        editor_name,
        "vi" | "vim"
            | "nvim"
            | "neovim"
            | "nano"
            | "micro"
            | "helix"
            | "hx"
            | "emacs"
            | "ne"
            | "joe"
            | "mcedit"
    )
}

#[cfg(test)]
#[path = "tests/event.rs"]
mod tests;
