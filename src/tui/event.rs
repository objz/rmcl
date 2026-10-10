// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use color_eyre::eyre::Context;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
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
    pub async fn run(&mut self, terminal: &mut Tui) -> color_eyre::Result<()> {
        let mut last_draw = std::time::Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(std::time::Instant::now);
        let mut drawn_overlay_count = self.overlay_count();
        let mut drawn_image_skips = Vec::new();
        let mut image_redraw_marker = false;
        while !self.exit {
            self.sync_instance_content();
            let redraw_requested = crate::feedback::take_redraw_request();
            let edited_config_changed = self.drain_edited_configs();
            if let Some(params) = new_instance::take_result() {
                self.spawn_create(params);
            }

            if let Some(result) = import_modpack::take_result() {
                self.spawn_import(result);
            }
            import_modpack::drain(&self.picker);

            self.dismiss_expired_errors();

            self.drain_pending_instances();
            self.drain_completed_instance_settings_updates();
            self.drain_failed_instance_settings_updates();
            self.instances_state.drain_modpack_updates();
            self.drain_pending_last_played();
            if let Some(state) = self.modpack_versions_state.as_mut() {
                state.drain_pending();
            }
            if self.content_update_popup.as_ref().is_some_and(|update| {
                update.phase != widgets::content::update::Phase::Applying
                    && self
                        .instances_state
                        .selected_instance()
                        .map(|instance| instance.name.as_str())
                        != Some(update.instance_name.as_str())
            }) && let Some(popup) = self.content_update_popup.take()
            {
                popup.cancel();
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
                self.forget_instance_content(&name);
            }
            self.sync_instance_content();
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
                discovery.drain_list(&self.picker);
            }
            self.datapacks_discovery_state.drain_pending();
            if self.focused == FocusedArea::Content {
                orphan_cleanup =
                    orphan_cleanup.or_else(|| self.datapacks_discovery_state.take_orphan_cleanup());
            }
            self.datapacks_discovery_state.drain_list(&self.picker);
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
            if self.content_update_check_ready() {
                self.content_update_check_pending = false;
                if crate::config::SETTINGS.read().general.check_content_updates {
                    self.spawn_selected_content_update_check();
                }
            }
            self.ensure_provider_conflict_popup();
            self.ensure_active_discovery_loaded();
            let progress_active = progress::is_active();
            let spinner_active = progress_active
                || crate::instance::runtime::has_active()
                || self.discovery_activity().is_some();
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
                || edited_config_changed
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

            if let Some(path) = self.pending_editor.take() {
                self.run_editor(terminal, &path);
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
                    complete &= crate::instance::content::local::record_toggle(
                        manifest,
                        &minecraft_dir,
                        &toggle.old_path,
                        &toggle.new_path,
                        toggle.enabled,
                    )?;
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

    pub(super) fn sync_instance_content(&mut self) {
        let key = self.instances_state.selected_instance().map(|instance| {
            super::app::InstanceContentKey {
                root: self.instance_manager.instances_dir.clone(),
                name: instance.name.clone(),
                created: instance.created,
                game_version: instance.game_version.clone(),
                loader: instance.loader,
            }
        });
        if self.content_for == key {
            return;
        }
        if let Some(previous) = self.content_for.take() {
            let mut cached = super::app::CachedInstanceContent::default();
            cached.swap(self);
            self.cached_instance_content.insert(previous, cached);
        }
        self.cached_instance_content.retain(|cached, _| {
            cached.root == self.instance_manager.instances_dir
                && self.instances_state.instances.iter().any(|instance| {
                    instance.name == cached.name
                        && instance.created == cached.created
                        && instance.game_version == cached.game_version
                        && instance.loader == cached.loader
                })
        });
        let cached = key
            .as_ref()
            .and_then(|key| self.cached_instance_content.remove(key));
        if let Some(mut cached) = cached {
            cached.swap(self);
        } else if key.is_some() {
            let client = crate::net::HttpClient::new();
            for state in [
                &mut self.mods_state,
                &mut self.resource_packs_state,
                &mut self.shaders_state,
                &mut self.world_datapacks_state,
            ] {
                state.enable_provider_icons(self.instance_manager.meta_dir.clone(), client.clone());
            }
            let font_size = self.picker.font_size();
            self.screenshots_state.font_size = (font_size.width, font_size.height);
        }
        self.content_for = key;
        self.provider_conflict = None;
        if let Some(instance) = self.instances_state.selected_instance() {
            let minecraft = crate::storage::InstancePaths::new(
                self.instance_manager.instances_dir.join(&instance.name),
            )
            .minecraft();
            let empty = crate::instance::ContentManifest::default();
            let manifest = self
                .content_manifest
                .as_ref()
                .map_or(&empty, |(_, manifest)| manifest);
            for discovery in [
                &mut self.mods_discovery_state,
                &mut self.resource_packs_discovery_state,
                &mut self.shaders_discovery_state,
                &mut self.datapacks_discovery_state,
            ] {
                discovery.refresh_installed_manifest(manifest, &minecraft);
            }
        }
        self.apply_cached_content_manifest();
    }

    fn ensure_content_reconciliation(&mut self, changed: bool) {
        self.sync_instance_content();
        let Some(instance) = self.instances_state.selected_instance().cloned() else {
            self.reconciliation_for = None;
            self.content_manifest = None;
            return;
        };
        let instance_id = (instance.name.clone(), instance.created);
        if !changed && self.reconciliation_for.as_ref() == Some(&instance_id) {
            return;
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
        let Some(mut result) = result else {
            return;
        };
        if result.complete {
            self.reconciliation_for = Some((result.instance_name.clone(), result.instance_created));
        }
        if let Some(error) = &result.error {
            tracing::warn!(
                "Content reconciliation for {} was incomplete: {}",
                result.instance_name,
                error
            );
            let path = crate::storage::InstancePaths::new(
                self.instance_manager.instances_dir.join(&selected.name),
            )
            .content_manifest();
            result.manifest = match crate::instance::ContentManifest::load(&path) {
                Ok(manifest) => manifest,
                Err(error) => {
                    tracing::error!("Could not reload content inventory: {error}");
                    let Some((name, manifest)) = &self.content_manifest else {
                        return;
                    };
                    if name != &selected.name {
                        return;
                    }
                    manifest.clone()
                }
            };
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
        if self.content_update_snapshot.is_none() {
            self.content_update_snapshot =
                crate::instance::content::updates::UpdateSnapshot::load(&paths.content_updates())
                    .filter(|snapshot| snapshot.applies_to(&selected))
                    .map(|snapshot| (result.instance_name.clone(), snapshot));
        }
        let stale = self
            .content_update_snapshot
            .as_ref()
            .is_none_or(|(_, snapshot)| snapshot.is_stale(&result.manifest));
        let running = crate::instance::content::updates::is_running(
            &selected,
            &result.manifest,
            &paths.content_updates(),
        );
        self.content_manifest = Some((result.instance_name.clone(), result.manifest.clone()));
        self.apply_content_update_snapshot();
        self.content_update_check_pending |=
            stale && !running && crate::config::SETTINGS.read().general.check_content_updates;
    }

    fn content_update_check_ready(&self) -> bool {
        let active = self.active_installed_content_state();
        self.content_update_check_pending && !(active.is_scanning() && active.entries.is_empty())
    }

    pub(super) fn apply_cached_content_manifest(&mut self) {
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
        for discovery in [
            &mut self.mods_discovery_state,
            &mut self.resource_packs_discovery_state,
            &mut self.shaders_discovery_state,
            &mut self.datapacks_discovery_state,
        ] {
            discovery.refresh_installed_manifest(manifest, &minecraft_dir);
        }
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
                            && pending.snapshot.applies_to(selected)
                    })
                    .map(|index| pending.remove(index));
                pending.retain(|pending| pending.instance_name != selected.name);
                latest
            }
            Err(_) => return,
        };
        let Some(snapshot) = snapshot else {
            return;
        };
        if self
            .content_manifest
            .as_ref()
            .is_some_and(|(name, manifest)| {
                name == &snapshot.instance_name && !snapshot.snapshot.is_stale(manifest)
            })
        {
            self.content_update_check_pending = false;
        }
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

    fn handle_events(&mut self) -> color_eyre::Result<bool> {
        match crossterm::event::poll(Duration::from_millis(16)) {
            Ok(true) => match event::read() {
                Ok(event) => self.dispatch_event(event),
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

    pub(super) fn dispatch_event(&mut self, event: Event) -> color_eyre::Result<bool> {
        match event {
            Event::Key(key)
                if key.kind == KeyEventKind::Press
                    || (key.kind == KeyEventKind::Repeat && self.key_repeat_allowed(&key)) =>
            {
                self.handle_key_event(key)
                    .wrap_err_with(|| format!("handling key event failed:\n{key:#?}"))?;
                Ok(true)
            }
            Event::Key(_) => Ok(false),
            Event::Mouse(mouse) => {
                self.handle_mouse_event(mouse);
                Ok(true)
            }
            _ => Ok(true),
        }
    }

    fn key_repeat_allowed(&self, key: &KeyEvent) -> bool {
        let vertical_navigation = matches!(
            key.code,
            KeyCode::Up | KeyCode::Down | KeyCode::Char('j' | 'k')
        );
        // These overlays take input before the focused area underneath them.
        if self
            .content_update_popup
            .as_ref()
            .is_some_and(widgets::content::update::State::visible)
            || self.modpack_update_popup.is_some()
            || self.provider_conflict.is_some()
        {
            return vertical_navigation;
        }
        if let Some(state) = &self.modpack_versions_state {
            return vertical_navigation || (state.text_input_active() && repeatable_text_key(key));
        }
        if self.focused == FocusedArea::ConfirmDelete {
            return false;
        }

        let text_input_active = match self.focused {
            FocusedArea::Instances => {
                self.instances_state.renaming.is_some() || self.instances_state.search.active
            }
            FocusedArea::Popup => new_instance::text_input_active(),
            FocusedArea::ImportPopup => import_modpack::text_input_active(),
            FocusedArea::Account => matches!(
                self.account_state.add_mode,
                widgets::account::AddMode::OfflineNameInput(_)
            ),
            FocusedArea::Settings => matches!(
                self.settings_state.add_mode,
                widgets::settings::AddMode::ProfileName(_)
            ),
            FocusedArea::InstanceSettings => self
                .instance_settings
                .as_ref()
                .is_some_and(widgets::popups::instance_settings::State::text_input_active),
            FocusedArea::GlobalSettings => self
                .global_settings
                .as_ref()
                .is_some_and(widgets::popups::global_settings::State::text_input_active),
            FocusedArea::OverviewExpanded => self.log_overlay_search.active,
            FocusedArea::Content => {
                let discovery = self.active_discovery_state();
                if let Some(state) = discovery
                    && (self.content_mode == widgets::content::ContentMode::Discover
                        || state.version_popup.is_some()
                        || state.project_page_open()
                        || state.sort_panel_open)
                {
                    state.text_input_active()
                } else {
                    match self.content_tab {
                        widgets::content::ContentTab::Mods => self.mods_state.search.active,
                        widgets::content::ContentTab::ResourcePacks => {
                            self.resource_packs_state.search.active
                        }
                        widgets::content::ContentTab::Shaders => self.shaders_state.search.active,
                        widgets::content::ContentTab::Worlds
                            if self.open_world_datapacks.is_some() =>
                        {
                            self.world_datapacks_state.search.active
                        }
                        widgets::content::ContentTab::Worlds => self.worlds_state.search.active,
                        widgets::content::ContentTab::Screenshots => {
                            self.screenshots_state.search.active
                        }
                        widgets::content::ContentTab::Logs if self.logs_state.viewer_focused => {
                            self.logs_state.viewer_search.active
                        }
                        widgets::content::ContentTab::Logs => self.logs_state.search.active,
                        _ => false,
                    }
                }
            }
            _ => false,
        };
        if text_input_active {
            return repeatable_text_key(key);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Left | KeyCode::Right)
            && matches!(
                self.focused,
                FocusedArea::Instances
                    | FocusedArea::Content
                    | FocusedArea::Account
                    | FocusedArea::Settings
                    | FocusedArea::Overview
            )
        {
            return true;
        }
        if vertical_navigation {
            return true;
        }
        match self.focused {
            FocusedArea::OverviewExpanded => matches!(key.code, KeyCode::Char('g' | 'G')),
            FocusedArea::Settings => matches!(
                key.code,
                KeyCode::Left | KeyCode::Right | KeyCode::Char('h' | 'l')
            ),
            FocusedArea::Content => {
                if let Some(state) = self.active_discovery_state() {
                    if state.project_page_open() {
                        return matches!(
                            key.code,
                            KeyCode::PageUp
                                | KeyCode::PageDown
                                | KeyCode::Home
                                | KeyCode::End
                                | KeyCode::Char('g' | 'G' | 'd' | 'u')
                        );
                    }
                    if state.version_popup.is_some() || state.sort_panel_open {
                        return false;
                    }
                    if self.content_mode == widgets::content::ContentMode::Discover
                        && widgets::content::discovery::page_key_direction(key).is_some()
                    {
                        return true;
                    }
                }
                if self.content_tab == widgets::content::ContentTab::Logs
                    && self.logs_state.viewer_focused
                    && matches!(key.code, KeyCode::Char('g' | 'G'))
                {
                    return true;
                }
                matches!(
                    key.code,
                    KeyCode::Left | KeyCode::Right | KeyCode::Char('h' | 'l')
                ) || (self.content_tab == widgets::content::ContentTab::Screenshots
                    && key.modifiers.contains(KeyModifiers::SHIFT)
                    && matches!(key.code, KeyCode::Char('H' | 'J' | 'K' | 'L')))
            }
            _ => false,
        }
    }

    fn spawn_create(&self, params: new_instance::WizardParams) {
        let instances_dir = self.instance_manager.instances_dir.clone();
        let meta_dir = crate::config::SETTINGS.read().paths.resolve_meta_dir();
        let pending_instances = PENDING_INSTANCES.clone();

        tokio::spawn(async move {
            progress::set_action("Creating instance...");
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
            progress::set_action("Updating instance...");
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

    // Terminal editors need the normal screen and input mode while they run.
    fn run_editor(&mut self, terminal: &mut ratatui::DefaultTerminal, path: &std::path::Path) {
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
        let parts = match editor_parts(&editor) {
            Ok(parts) => parts,
            Err(error) => {
                tracing::error!("Cannot parse editor command: {error}");
                return;
            }
        };
        let Some((program, args)) = parts.split_first() else {
            tracing::error!("Editor command is empty");
            return;
        };

        let is_tui_editor = editor_runs_in_terminal(program);

        if is_tui_editor {
            if let Err(error) = self.watch_edited_config(path) {
                error_buffer::push_message(
                    tracing::Level::ERROR,
                    format!("Cannot watch edited file {}: {error}", path.display()),
                );
                return;
            }
            let _ = stdout().execute(DisableMouseCapture);
            let _ = stdout().execute(LeaveAlternateScreen);
            let _ = disable_raw_mode();

            let result = std::process::Command::new(program)
                .args(args)
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
            }
        } else {
            if let Err(e) = self.launch_gui_editor(
                std::process::Command::new(program).args(args).arg(path),
                path,
            ) {
                tracing::error!("Failed to open editor: {}", e);
            }
        }
    }

    fn watch_edited_config(&mut self, path: &std::path::Path) -> color_eyre::Result<()> {
        let path = std::path::absolute(path)?;
        if !self
            .edited_config_watches
            .iter()
            .any(|watch| watch.path == path)
        {
            self.edited_config_watches
                .push(EditedConfigWatch::new(path)?);
        }
        Ok(())
    }

    fn launch_gui_editor(
        &mut self,
        command: &mut std::process::Command,
        path: &std::path::Path,
    ) -> color_eyre::Result<std::thread::JoinHandle<()>> {
        self.watch_edited_config(path)?;
        let mut child = command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()?;
        // A GUI launcher can exit after handing the file to an existing editor.
        // Reap it without tying the file watch or Tokio shutdown to its lifetime.
        Ok(std::thread::spawn(move || match child.wait() {
            Ok(status) if !status.success() => tracing::warn!("Editor exited with {status}"),
            Err(error) => tracing::error!("Failed to wait for editor: {error}"),
            _ => {}
        }))
    }

    fn drain_edited_configs(&mut self) -> bool {
        let changed = self
            .edited_config_watches
            .iter_mut()
            .filter_map(|watch| watch.changed().then(|| watch.path.clone()))
            .collect::<Vec<_>>();
        for path in &changed {
            self.reload_edited_config(path);
        }
        !changed.is_empty()
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
                            self.queue_content_update_checks();
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
        crate::instance::content::updates::cancel(Some(
            &crate::storage::InstancePaths::new(
                self.instance_manager.instances_dir.join(instance_name),
            )
            .content_updates(),
        ));
        self.settings_state
            .invalidate_java_cache(Some(instance_name));
        self.cached_instance_content
            .retain(|key, _| key.name != instance_name);
        if self
            .content_for
            .as_ref()
            .is_some_and(|key| key.name != instance_name)
        {
            return;
        }
        super::app::CachedInstanceContent::default().swap(self);
        self.content_for = None;
        if let Some(popup) = self.content_update_popup.take() {
            popup.cancel();
        }
        self.provider_conflict = None;
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

pub(super) struct EditedConfigWatch {
    path: std::path::PathBuf,
    contents: Option<Vec<u8>>,
    events: std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
    pending: Option<std::time::Instant>,
    read_error: Option<std::io::ErrorKind>,
    _watcher: notify::RecommendedWatcher,
}

impl EditedConfigWatch {
    fn new(path: std::path::PathBuf) -> color_eyre::Result<Self> {
        use notify::Watcher;

        let (tx, events) = std::sync::mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = tx.send(event);
        })?;
        let parent = path
            .parent()
            .ok_or_else(|| color_eyre::eyre::eyre!("File has no parent directory"))?;
        watcher.watch(parent, notify::RecursiveMode::NonRecursive)?;
        let contents = match std::fs::read(&path) {
            Ok(contents) => Some(contents),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path,
            contents,
            events,
            pending: None,
            read_error: None,
            _watcher: watcher,
        })
    }

    fn changed(&mut self) -> bool {
        for result in self.events.try_iter() {
            match result {
                Ok(event)
                    if matches!(event.kind, notify::EventKind::Access(_))
                        && !matches!(
                            event.kind,
                            notify::EventKind::Access(notify::event::AccessKind::Close(
                                notify::event::AccessMode::Write
                            ))
                        ) => {}
                Ok(event)
                    if event.need_rescan()
                        || event.paths.is_empty()
                        || event.paths.iter().any(|path| {
                            path.file_name().zip(self.path.file_name()).is_some_and(
                                |(changed, watched)| changed.eq_ignore_ascii_case(watched),
                            ) || Some(path.as_path()) == self.path.parent()
                        }) =>
                {
                    self.pending = Some(std::time::Instant::now());
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::error!(
                        "Edited file watch for {} failed: {error}",
                        self.path.display()
                    );
                    self.pending = Some(std::time::Instant::now());
                }
            }
        }
        if self
            .pending
            .is_none_or(|pending| pending.elapsed() < Duration::from_millis(100))
        {
            return false;
        }
        let contents = match std::fs::read(&self.path) {
            Ok(contents) => contents,
            Err(error) => {
                if error.kind() != std::io::ErrorKind::NotFound
                    && self.read_error != Some(error.kind())
                {
                    tracing::warn!("Cannot read edited file {}: {error}", self.path.display());
                }
                self.read_error = Some(error.kind());
                self.pending = Some(std::time::Instant::now());
                return false;
            }
        };
        self.pending = None;
        self.read_error = None;
        if self.contents.as_ref() == Some(&contents) {
            return false;
        }
        self.contents = Some(contents);
        true
    }
}

fn repeatable_text_key(key: &KeyEvent) -> bool {
    matches!(
        key.code,
        KeyCode::Char(_)
            | KeyCode::Backspace
            | KeyCode::Delete
            | KeyCode::Left
            | KeyCode::Right
            | KeyCode::Up
            | KeyCode::Down
            | KeyCode::Home
            | KeyCode::End
            | KeyCode::PageUp
            | KeyCode::PageDown
    )
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

fn editor_parts(editor: &str) -> std::io::Result<Vec<String>> {
    if std::path::Path::new(editor).is_file() {
        Ok(vec![editor.to_owned()])
    } else {
        split_editor_command(editor)
    }
}

#[cfg(unix)]
fn split_editor_command(editor: &str) -> std::io::Result<Vec<String>> {
    shlex::split(editor).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Unterminated editor quote or escape",
        )
    })
}

#[cfg(windows)]
fn split_editor_command(editor: &str) -> std::io::Result<Vec<String>> {
    use windows_sys::Win32::{Foundation::LocalFree, UI::Shell::CommandLineToArgvW};
    if editor.trim().is_empty() {
        return Ok(Vec::new());
    }
    if editor.contains('\0') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Editor command contains a NUL",
        ));
    }
    let command = editor
        .trim()
        .encode_utf16()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let mut count = 0;
    // The OS allocates the complete argument array; it remains valid until LocalFree below.
    let args = unsafe { CommandLineToArgvW(command.as_ptr(), &mut count) };
    if args.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    let mut parts = Vec::new();
    for index in 0..count as usize {
        let arg = unsafe { *args.add(index) };
        let mut length = 0;
        while unsafe { *arg.add(length) } != 0 {
            length += 1;
        }
        parts.push(String::from_utf16_lossy(unsafe {
            std::slice::from_raw_parts(arg, length)
        }));
    }
    unsafe {
        LocalFree(args.cast());
    }
    Ok(parts)
}

#[cfg(test)]
#[path = "tests/event.rs"]
mod tests;
