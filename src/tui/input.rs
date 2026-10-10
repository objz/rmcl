// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};

use super::app::{App, FocusedArea};
use super::widgets::{
    self, WidgetKey, popups::confirm as confirm_popup, popups::import_modpack, popups::new_instance,
};
use crate::feedback::errors as error_buffer;

impl App {
    pub(super) fn handle_mouse_event(&mut self, event: MouseEvent) {
        let scroll_key = match event.kind {
            MouseEventKind::ScrollUp => Some(KeyCode::Up),
            MouseEventKind::ScrollDown => Some(KeyCode::Down),
            _ => None,
        };
        if let Some(code) = scroll_key {
            if let Err(error) = self.handle_key_event(KeyEvent::new(code, event.modifiers)) {
                tracing::error!("Mouse scroll handling failed: {error}");
            }
            return;
        }
        if self.handle_log_selection_mouse(event) {
            return;
        }
        if event.kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }
        if self.focused == FocusedArea::ImportPopup
            && import_modpack::handle_discovery_click(event.column, event.row)
        {
            return;
        }
        if self.focused != FocusedArea::Content
            || self.content_mode != widgets::content::ContentMode::Discover
        {
            return;
        }
        let page_clicked = self.active_discovery_state_mut().is_some_and(|state| {
            !state.search.active
                && !state.project_page_open()
                && state.version_popup.is_none()
                && state.list.click_page(event.column, event.row)
        });
        if page_clicked {
            self.spawn_active_discovery_page();
            return;
        }
        let link = self
            .active_discovery_state_mut()
            .and_then(|state| state.project_link_at(event.column, event.row));
        if let Some(link) = link {
            let result = project_link_url(link).and_then(|url| open::that_detached(url.as_str()));
            if let Err(error) = result {
                tracing::warn!("Failed to open project link: {error}");
            }
        }
    }

    fn handle_log_selection_mouse(&mut self, event: MouseEvent) -> bool {
        if !matches!(
            event.kind,
            MouseEventKind::Down(MouseButton::Left)
                | MouseEventKind::Drag(MouseButton::Left)
                | MouseEventKind::Up(MouseButton::Left)
        ) {
            return false;
        }
        if self
            .content_update_popup
            .as_ref()
            .is_some_and(widgets::content::update::State::visible)
            || self.provider_conflict.is_some()
            || self.modpack_versions_state.is_some()
            || self.modpack_update_popup.is_some()
        {
            return false;
        }
        if self.focused == FocusedArea::OverviewExpanded && !self.log_filter_open {
            let lines = self.overlay_filtered_lines();
            self.log_selection.handle_mouse(
                event,
                self.log_overlay_inner,
                self.log_overlay_scroll,
                &lines,
            )
        } else if self.focused == FocusedArea::Content
            && self.content_tab == widgets::content::ContentTab::Logs
            && self.content_mode == widgets::content::ContentMode::Installed
            && !self.logs_state.filter_open
        {
            widgets::logs_viewer::handle_selection_mouse(event, &mut self.logs_state)
        } else {
            false
        }
    }

    pub(super) fn handle_key_event(&mut self, key_event: KeyEvent) -> color_eyre::Result<()> {
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
            return Ok(());
        }
        if self
            .content_update_popup
            .as_ref()
            .is_some_and(widgets::content::update::State::visible)
        {
            self.handle_content_update_key(key_event);
            return Ok(());
        }
        if self.modpack_versions_state.is_some() {
            self.handle_modpack_versions_key(key_event);
            return Ok(());
        }
        if self.modpack_update_popup.is_some() {
            self.handle_modpack_update_key(key_event);
            return Ok(());
        }
        if let Some(conflict) = self.provider_conflict.as_mut() {
            match key_event.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    conflict.selected =
                        (conflict.selected + 1).min(conflict.candidates.len().saturating_sub(1));
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    conflict.selected = conflict.selected.saturating_sub(1);
                }
                KeyCode::Enter => {
                    let relative_path = conflict.relative_path.clone();
                    let project = conflict.candidates.get(conflict.selected).cloned();
                    let aliases = conflict
                        .candidates
                        .iter()
                        .enumerate()
                        .filter(|(index, _)| *index != conflict.selected)
                        .map(|(_, candidate)| candidate.clone())
                        .collect::<Vec<_>>();
                    if let Some(project) = project
                        && let Some((instance_name, cached)) = &self.content_manifest
                    {
                        let paths = crate::storage::InstancePaths::new(
                            self.instance_manager.instances_dir.join(instance_name),
                        );
                        let manifest_path = paths.content_manifest();
                        let expected = cached.record(&relative_path).cloned();
                        let updated = crate::instance::ContentManifest::update(
                            &manifest_path,
                            |manifest| {
                                if expected.is_none()
                                    || manifest.record(&relative_path) != expected.as_ref()
                                {
                                    return Err(crate::instance::content::manifest::ManifestError::InvalidPath(
                                        format!("Ownership of '{}' changed; refresh before selecting a provider", relative_path.display()),
                                    ));
                                }
                                if let Some(record) = &expected {
                                    crate::instance::content::local::validate_record(
                                        &paths.minecraft(),
                                        record,
                                    )?;
                                }
                                if let Some(record) = manifest
                                    .files
                                    .iter_mut()
                                    .find(|record| record.relative_path == relative_path)
                                {
                                    record.resolution =
                                        crate::instance::Resolution::Resolved { project };
                                    record.provider_aliases = aliases;
                                }
                                Ok(manifest.clone())
                            },
                        );
                        match updated {
                            Ok(updated) => {
                                self.content_manifest = Some((instance_name.clone(), updated));
                                self.provider_conflict = None;
                                self.reconciliation_for = None;
                            }
                            Err(error) => {
                                tracing::error!("Could not save provider selection: {error}")
                            }
                        }
                    }
                }
                KeyCode::Esc => {
                    self.dismissed_provider_conflicts
                        .insert(conflict.relative_path.clone());
                    self.provider_conflict = None;
                }
                _ => {}
            }
            return Ok(());
        }

        // log overlay eats all input when open, including its own search sub-mode
        if self.focused == FocusedArea::OverviewExpanded {
            if self.log_filter_open {
                if widgets::logs_viewer::handle_level_filter_key(
                    &key_event,
                    &mut self.log_filter_open,
                    &mut self.log_filter_selected,
                    &mut self.log_level_filters,
                ) {
                    self.log_selection.clear();
                    self.log_overlay_scroll = 0;
                }
                return Ok(());
            }
            if self.log_overlay_search.active {
                self.log_selection.clear();
                match key_event.code {
                    KeyCode::Enter => {
                        self.log_overlay_search.confirm();
                        self.log_overlay_scroll = 0;
                    }
                    KeyCode::Esc => {
                        self.log_overlay_search.deactivate();
                    }
                    KeyCode::Backspace => {
                        self.log_overlay_search.backspace(key_event.modifiers);
                    }
                    KeyCode::Char(c) => {
                        self.log_overlay_search.push(c);
                    }
                    _ => {}
                }
                return Ok(());
            }
            match key_event.code {
                KeyCode::Char('O') | KeyCode::Esc => {
                    self.log_selection.clear();
                    self.focused = self.pre_overlay_focused;
                    self.log_overlay_search.deactivate();
                    return Ok(());
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    if self.log_overlay_scroll < self.log_overlay_max_scroll {
                        self.log_overlay_scroll += 1;
                    }
                    return Ok(());
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.log_overlay_scroll = self.log_overlay_scroll.saturating_sub(1);
                    return Ok(());
                }
                KeyCode::Char('G') | KeyCode::End => {
                    self.log_overlay_scroll = self.log_overlay_max_scroll;
                    return Ok(());
                }
                KeyCode::Char('g') | KeyCode::Home => {
                    self.log_overlay_scroll = 0;
                    return Ok(());
                }
                KeyCode::Char('/') => {
                    self.log_selection.clear();
                    self.log_overlay_search.activate();
                    return Ok(());
                }
                KeyCode::Char('f') => {
                    self.log_filter_open = true;
                    self.log_filter_selected = 0;
                    return Ok(());
                }
                KeyCode::Char('y') => {
                    self.log_selection.finish();
                    widgets::logs_viewer::yank_selection(
                        &self.log_selection,
                        &self.overlay_filtered_lines(),
                    );
                    return Ok(());
                }
                _ => {
                    return Ok(());
                }
            }
        }

        if self.focused == FocusedArea::ConfirmDelete {
            match key_event.code {
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    let focus_after = match confirm_popup::pending_target() {
                        Some(confirm_popup::ConfirmTarget::Instance { name }) => {
                            match self.instance_manager.delete(&name) {
                                Ok(_) => {
                                    self.instances_state.remove_instance(&name);
                                    self.forget_instance_content(&name);
                                }
                                Err(e) => {
                                    tracing::error!("Failed to delete instance '{}': {}", name, e);
                                }
                            }
                            FocusedArea::Instances
                        }
                        Some(confirm_popup::ConfirmTarget::Account { index, .. }) => {
                            let count = self.account_state.store.accounts.len();
                            let removed_uuid = self
                                .account_state
                                .store
                                .accounts
                                .get(index)
                                .map(|account| account.uuid.clone());
                            if let Err(error) = self.account_state.store.remove(index) {
                                error_buffer::push_message(
                                    tracing::Level::ERROR,
                                    error.to_string(),
                                );
                                self.focused = FocusedArea::Account;
                                confirm_popup::clear_pending();
                                return Ok(());
                            }
                            if let Some(removed_uuid) = removed_uuid {
                                for instance in &mut self.instances_state.instances {
                                    if instance.preferred_account.as_deref()
                                        == Some(removed_uuid.as_str())
                                    {
                                        let mut updated = instance.clone();
                                        updated.preferred_account = None;
                                        if let Err(error) = self.instance_manager.save(&updated) {
                                            error_buffer::push_message(
                                                tracing::Level::ERROR,
                                                format!(
                                                    "Failed to reset preferred account for '{}': {error}",
                                                    instance.name
                                                ),
                                            );
                                        } else {
                                            *instance = updated;
                                        }
                                    }
                                }
                            }
                            if count > 1 {
                                self.account_state.list_state.selected = Some(index.min(
                                    self.account_state.store.accounts.len().saturating_sub(1),
                                ));
                            } else {
                                self.account_state.list_state.selected = None;
                            }
                            FocusedArea::Account
                        }
                        Some(confirm_popup::ConfirmTarget::ConfigProfile { profile }) => {
                            if let Err(e) = self.delete_config_profile(&profile) {
                                error_buffer::push_error(error_buffer::ErrorEvent {
                                    id: 0,
                                    level: tracing::Level::ERROR,
                                    message: e.to_string(),
                                    pushed_at: std::time::Instant::now(),
                                });
                            } else {
                                self.settings_state.remove_profile(&profile);
                            }
                            FocusedArea::Settings
                        }
                        Some(confirm_popup::ConfirmTarget::Content { name, path, .. }) => {
                            match self.delete_content_path(&path, false) {
                                Ok(orphaned) => {
                                    self.remove_content_path_from_states(&path);
                                    if !orphaned.is_empty() {
                                        confirm_popup::set_pending_orphan_dependencies(orphaned);
                                        return Ok(());
                                    }
                                }
                                Err(e) => {
                                    tracing::error!("Failed to delete content '{}': {}", name, e);
                                }
                            }
                            FocusedArea::Content
                        }
                        Some(confirm_popup::ConfirmTarget::OrphanDependencies { paths }) => {
                            for path in paths {
                                match self.delete_content_path(&path, true) {
                                    Ok(_) => {
                                        self.remove_content_path_from_states(&path);
                                    }
                                    Err(error) => tracing::error!(
                                        "Failed to remove unused dependency '{}': {}",
                                        path.display(),
                                        error
                                    ),
                                }
                            }
                            FocusedArea::Content
                        }
                        Some(confirm_popup::ConfirmTarget::InstanceRuntime { .. }) => {
                            self.focused = FocusedArea::InstanceSettings;
                            let confirmed = self
                                .instance_settings
                                .as_mut()
                                .and_then(|state| state.confirmed_save());
                            if let Some((updated, desktop)) = confirmed {
                                self.apply_instance_settings(*updated, desktop);
                            }
                            self.focused
                        }
                        Some(confirm_popup::ConfirmTarget::LauncherCache) => {
                            let meta_dir = crate::config::SETTINGS.read().paths.resolve_meta_dir();
                            match crate::storage::clear_disposable_caches(&meta_dir) {
                                Ok(()) => {
                                    self.reset_discovery_states();
                                    self.settings_state.invalidate_java_cache(None);
                                    if let Some(state) = self.global_settings.as_mut() {
                                        state.invalidate_java_cache();
                                    }
                                    if let Some(state) = self.instance_settings.as_mut() {
                                        state.invalidate_java_cache();
                                    }
                                    error_buffer::push_message(
                                        tracing::Level::INFO,
                                        "Cleared launcher caches",
                                    );
                                }
                                Err(error) => error_buffer::push_message(
                                    tracing::Level::ERROR,
                                    format!("Failed to clear launcher caches: {error}"),
                                ),
                            }
                            FocusedArea::GlobalSettings
                        }
                        Some(confirm_popup::ConfirmTarget::AutomaticSelection {
                            setting,
                            instance,
                            ..
                        }) => {
                            match setting {
                                confirm_popup::AutomaticSetting::Java if instance.is_some() => {
                                    self.focused = FocusedArea::InstanceSettings;
                                    let confirmed =
                                        self.instance_settings.as_mut().and_then(|state| {
                                            state.enable_auto_java();
                                            state.confirmed_save()
                                        });
                                    if let Some((updated, desktop)) = confirmed {
                                        self.apply_instance_settings(*updated, desktop);
                                    }
                                }
                                confirm_popup::AutomaticSetting::Account => {
                                    self.focused = FocusedArea::InstanceSettings;
                                    let confirmed =
                                        self.instance_settings.as_mut().and_then(|state| {
                                            state.enable_auto_account();
                                            state.confirmed_save()
                                        });
                                    if let Some((updated, desktop)) = confirmed {
                                        self.apply_instance_settings(*updated, desktop);
                                    }
                                }
                                confirm_popup::AutomaticSetting::Java => {
                                    self.focused = FocusedArea::GlobalSettings;
                                    let action = self.global_settings.as_mut().map(
                                        widgets::popups::global_settings::State::confirm_auto_java,
                                    );
                                    if let Some(widgets::popups::global_settings::Action::Save(
                                        config,
                                    )) = action
                                    {
                                        self.apply_global_settings(*config);
                                    }
                                }
                            }
                            self.focused
                        }
                        None => FocusedArea::Instances,
                    };
                    confirm_popup::clear_pending();
                    self.focused = focus_after;
                    return Ok(());
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                    let focus_after = match confirm_popup::pending_target() {
                        Some(confirm_popup::ConfirmTarget::Content { .. }) => FocusedArea::Content,
                        Some(confirm_popup::ConfirmTarget::OrphanDependencies { .. }) => {
                            FocusedArea::Content
                        }
                        Some(confirm_popup::ConfirmTarget::Account { .. }) => FocusedArea::Account,
                        Some(confirm_popup::ConfirmTarget::ConfigProfile { .. }) => {
                            FocusedArea::Settings
                        }
                        Some(confirm_popup::ConfirmTarget::InstanceRuntime { .. }) => {
                            let remaining = self
                                .instance_settings
                                .as_mut()
                                .and_then(|state| state.cancel_runtime_change());
                            if let Some((updated, desktop)) = remaining {
                                self.apply_instance_settings(*updated, desktop);
                            }
                            FocusedArea::InstanceSettings
                        }
                        Some(confirm_popup::ConfirmTarget::LauncherCache) => {
                            FocusedArea::GlobalSettings
                        }
                        Some(confirm_popup::ConfirmTarget::AutomaticSelection {
                            instance, ..
                        }) => {
                            if instance.is_some() {
                                FocusedArea::InstanceSettings
                            } else {
                                FocusedArea::GlobalSettings
                            }
                        }
                        _ => FocusedArea::Instances,
                    };
                    confirm_popup::clear_pending();
                    self.focused = focus_after;
                    return Ok(());
                }
                _ => {
                    return Ok(());
                }
            }
        }

        if key_event.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(
                key_event.code,
                KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right
            )
            && match self.focused {
                FocusedArea::Instances => self.instances_state.renaming.is_none(),
                FocusedArea::Content => !self.active_discovery_state().is_some_and(|state| {
                    state.version_popup.is_some() || state.project_page_open()
                }),
                FocusedArea::Account => {
                    matches!(self.account_state.add_mode, widgets::account::AddMode::None)
                }
                FocusedArea::Settings => {
                    matches!(
                        self.settings_state.add_mode,
                        widgets::settings::AddMode::None
                    )
                }
                FocusedArea::Overview => true,
                _ => false,
            }
        {
            self.focused =
                match (self.focused, key_event.code) {
                    (FocusedArea::Instances, KeyCode::Right) => FocusedArea::Content,
                    (FocusedArea::Content, KeyCode::Left)
                    | (FocusedArea::Account, KeyCode::Left) => FocusedArea::Instances,
                    (FocusedArea::Content, KeyCode::Down)
                    | (FocusedArea::Settings, KeyCode::Left) => FocusedArea::Account,
                    (FocusedArea::Account, KeyCode::Right)
                    | (FocusedArea::Overview, KeyCode::Left) => FocusedArea::Settings,
                    (FocusedArea::Settings, KeyCode::Right) => FocusedArea::Overview,
                    (
                        FocusedArea::Account | FocusedArea::Settings | FocusedArea::Overview,
                        KeyCode::Up,
                    ) => FocusedArea::Content,
                    (focused, _) => focused,
                };
            return Ok(());
        }

        if self.focused == FocusedArea::Content
            && self.content_mode == widgets::content::ContentMode::Installed
            && let Some(state) = self.active_discovery_state_mut()
            && state.version_popup.is_none()
            && !state.project_page_open()
            && (state.sort_panel_open || key_event.code == KeyCode::Char('f'))
        {
            state.set_local_mode(true);
            if widgets::content::discovery::handle_key(&key_event, state) {
                return Ok(());
            }
        }
        let discovery_popup_open = self
            .active_discovery_state()
            .is_some_and(|state| state.version_popup.is_some() || state.project_page_open());
        if self.focused == FocusedArea::Content
            && (self.content_mode == widgets::content::ContentMode::Discover
                || discovery_popup_open)
        {
            let (search_active, popup_open, project_page_open, sort_panel_focused) = self
                .active_discovery_state_mut()
                .map(|state| {
                    (
                        state.search.active,
                        state.version_popup.is_some(),
                        state.project_page_open(),
                        state.sort_panel_focused,
                    )
                })
                .unwrap_or_default();
            if !search_active
                && !popup_open
                && !project_page_open
                && !sort_panel_focused
                && key_event.code == KeyCode::Enter
            {
                self.spawn_active_discovery_project_page();
                return Ok(());
            }
            if !search_active
                && !popup_open
                && !sort_panel_focused
                && key_event.code == KeyCode::Char('v')
            {
                self.spawn_active_discovery_versions();
                return Ok(());
            }
            if !search_active
                && !popup_open
                && !project_page_open
                && !sort_panel_focused
                && key_event.code == KeyCode::Char('d')
            {
                if let Some(pending) = self
                    .active_discovery_state_mut()
                    .and_then(|state| state.pending_installed_delete())
                {
                    self.confirm_content_delete(pending);
                }
                return Ok(());
            }
            if popup_open && key_event.code == KeyCode::Enter {
                if self
                    .active_discovery_state_mut()
                    .is_some_and(|state| state.select_minecraft_version())
                {
                    return Ok(());
                }
                let selecting_world = self
                    .active_discovery_state_mut()
                    .and_then(|state| state.version_popup.as_ref())
                    .is_some_and(|popup| popup.selecting_world);
                if selecting_world {
                    let minecraft_dir = self.instances_state.selected_instance().map(|instance| {
                        crate::storage::InstancePaths::new(
                            self.instance_manager.instances_dir.join(&instance.name),
                        )
                        .minecraft()
                    });
                    let manifest = self
                        .content_manifest
                        .as_ref()
                        .map(|(_, manifest)| manifest.clone());
                    if let Some(minecraft_dir) = minecraft_dir
                        && self.active_discovery_state_mut().is_some_and(|state| {
                            state.select_world(manifest.as_ref(), &minecraft_dir)
                        })
                    {
                        self.spawn_active_discovery_dependencies();
                    }
                    return Ok(());
                }
                let confirming = self
                    .active_discovery_state_mut()
                    .and_then(|state| state.version_popup.as_ref())
                    .is_some_and(|popup| popup.confirming);
                if confirming {
                    self.spawn_active_discovery_install();
                } else {
                    let kind = self.active_discovery_state_mut().map(|state| state.kind);
                    if kind == Some(crate::instance::ContentKind::DataPack) {
                        let worlds = self.worlds_state.entries.clone();
                        if let Some(state) = self.active_discovery_state_mut() {
                            state.begin_world_selection(worlds);
                        }
                    } else {
                        self.spawn_active_discovery_dependencies();
                    }
                }
                return Ok(());
            }
            if popup_open && key_event.code == KeyCode::Tab {
                self.spawn_active_discovery_version_source();
                return Ok(());
            }
            let installed = self.content_mode == widgets::content::ContentMode::Installed;
            let handled = self.active_discovery_state_mut().is_some_and(|state| {
                state.set_local_mode(installed);
                widgets::content::discovery::handle_key(&key_event, state)
            });
            if handled {
                let overlay_open = self.active_discovery_state().is_some_and(|state| {
                    state.version_popup.is_some() || state.project_page_open()
                });
                if !overlay_open
                    && (matches!(
                        key_event.code,
                        KeyCode::Char('j') | KeyCode::Char('k') | KeyCode::Down | KeyCode::Up
                    ) || widgets::content::discovery::page_key_direction(&key_event).is_some())
                {
                    self.spawn_active_discovery_page();
                }
                return Ok(());
            }
        } else if self.focused == FocusedArea::Content {
            if self.content_tab == widgets::content::ContentTab::Logs {
                if key_event.code == KeyCode::Char('d')
                    && !self.logs_state.filter_open
                    && !self.logs_state.search.active
                    && !self.logs_state.viewer_search.active
                {
                    if let Some(pending) = self.logs_state.pending_delete() {
                        self.confirm_content_delete(pending);
                    }
                    return Ok(());
                }
                if widgets::logs_viewer::handle_key(&key_event, &mut self.logs_state) {
                    return Ok(());
                }
            } else if self.content_tab == widgets::content::ContentTab::Screenshots {
                if key_event.code == KeyCode::Char('d') && !self.screenshots_state.search.active {
                    if let Some(pending) = self.screenshots_state.pending_delete() {
                        self.confirm_content_delete(pending);
                    }
                    return Ok(());
                }
                if widgets::screenshots_grid::handle_key(&key_event, &mut self.screenshots_state) {
                    return Ok(());
                }
            } else if self.content_tab == widgets::content::ContentTab::Worlds {
                if self.open_world_datapacks.is_some() {
                    if key_event.code == KeyCode::Char('u')
                        && !self.world_datapacks_state.search.active
                    {
                        self.spawn_bulk_content_updates();
                        return Ok(());
                    }
                    if key_event.code == KeyCode::Char('v')
                        && !self.world_datapacks_state.search.active
                        && self.world_datapacks_state.selected_has_provider_project()
                    {
                        self.spawn_installed_versions();
                        return Ok(());
                    }
                    if matches!(key_event.code, KeyCode::Esc | KeyCode::Char('h'))
                        && !self.world_datapacks_state.search.active
                    {
                        self.open_world_datapacks = None;
                        return Ok(());
                    }
                    if key_event.code == KeyCode::Char('d')
                        && !self.world_datapacks_state.search.active
                    {
                        if let Some(pending) = self.world_datapacks_state.pending_delete() {
                            self.confirm_content_delete(pending);
                        }
                        return Ok(());
                    }
                    if widgets::content::list::handle_key_no_toggle(
                        &key_event,
                        &mut self.world_datapacks_state,
                    ) {
                        return Ok(());
                    }
                    return Ok(());
                }
                if key_event.code == KeyCode::Enter
                    && !key_event.modifiers.contains(KeyModifiers::SHIFT)
                    && !self.worlds_state.search.active
                {
                    self.open_world_datapacks = self
                        .worlds_state
                        .selected_entry()
                        .map(|entry| (entry.name.clone(), entry.path.clone()));
                    return Ok(());
                }
                if key_event.code == KeyCode::Char('q') && !self.worlds_state.search.active {
                    if self.selected_instance_supports_quick_play() {
                        let instance = self.instances_state.selected_instance().cloned();
                        let world = self
                            .worlds_state
                            .selected_entry()
                            .and_then(|entry| entry.path.file_name())
                            .and_then(|name| name.to_str())
                            .map(str::to_owned);
                        if let (Some(instance), Some(world)) = (instance, world) {
                            self.spawn_launch(instance, Some(world));
                        }
                    }
                    return Ok(());
                }
                if key_event.code == KeyCode::Char('d') && !self.worlds_state.search.active {
                    if let Some(pending) = self.worlds_state.pending_delete() {
                        self.confirm_content_delete(pending);
                    }
                    return Ok(());
                }
                if widgets::content::list::handle_key_no_toggle(&key_event, &mut self.worlds_state)
                {
                    return Ok(());
                }
            } else {
                let state = match self.content_tab {
                    widgets::content::ContentTab::Mods => Some(&mut self.mods_state),
                    widgets::content::ContentTab::ResourcePacks => {
                        Some(&mut self.resource_packs_state)
                    }
                    widgets::content::ContentTab::Shaders => Some(&mut self.shaders_state),
                    _ => None,
                };
                if let Some(state) = state {
                    if key_event.code == KeyCode::Char('u') && !state.search.active {
                        self.spawn_bulk_content_updates();
                        return Ok(());
                    }
                    if key_event.code == KeyCode::Char('v')
                        && !state.search.active
                        && state.selected_has_provider_project()
                    {
                        self.spawn_installed_versions();
                        return Ok(());
                    }
                    if key_event.code == KeyCode::Char('d') && !state.search.active {
                        if let Some(pending) = state.pending_delete() {
                            self.confirm_content_delete(pending);
                        }
                        return Ok(());
                    }
                    if widgets::content::list::handle_key(&key_event, state) {
                        return Ok(());
                    }
                }
            }
        }

        if self.focused == FocusedArea::Account
            && widgets::account::handle_key(&key_event, &mut self.account_state)
        {
            return Ok(());
        }

        if self.focused == FocusedArea::Account
            && let KeyCode::Char('d') = key_event.code
            && let Some(index) = self.account_state.list_state.selected
            && let Some(account) = self.account_state.store.accounts.get(index)
        {
            confirm_popup::set_pending(confirm_popup::ConfirmTarget::Account {
                username: account.username.clone(),
                index,
            });
            self.focused = FocusedArea::ConfirmDelete;
            return Ok(());
        }

        if self.focused == FocusedArea::Settings {
            let editing_profile = matches!(
                &self.settings_state.add_mode,
                widgets::settings::AddMode::ProfileName(_)
            );
            match widgets::settings::handle_key(
                &key_event,
                &mut self.settings_state,
                self.instances_state.selected_instance(),
            ) {
                widgets::settings::SettingsAction::OpenInstance => {
                    self.open_instance_settings(FocusedArea::Settings);
                    return Ok(());
                }
                widgets::settings::SettingsAction::OpenGlobal => {
                    self.pre_overlay_focused = FocusedArea::Settings;
                    self.global_settings = Some(
                        widgets::popups::global_settings::State::with_detected_image_protocol(
                            self.detected_image_protocol,
                        ),
                    );
                    self.focused = FocusedArea::GlobalSettings;
                    return Ok(());
                }
                widgets::settings::SettingsAction::SelectProfile(profile) => {
                    if let Some(instance) = self.instances_state.selected_instance().cloned() {
                        if self
                            .pending_instance_settings_updates
                            .contains(&instance.name)
                        {
                            error_buffer::push_message(
                                tracing::Level::WARN,
                                super::app::RUNTIME_UPDATE_PENDING_MESSAGE,
                            );
                            return Ok(());
                        }
                        let mut updated = instance.clone();
                        match crate::instance::config_sync::switch_profile_and_save(
                            &self.instance_manager,
                            &mut updated,
                            profile.as_deref(),
                        ) {
                            Ok(()) => {
                                self.instances_state
                                    .replace_instance(&instance.name, updated);
                            }
                            Err(error) => {
                                error_buffer::push_error(error_buffer::ErrorEvent {
                                    id: 0,
                                    level: tracing::Level::ERROR,
                                    message: error.to_string(),
                                    pushed_at: std::time::Instant::now(),
                                });
                            }
                        }
                    }
                    return Ok(());
                }
                widgets::settings::SettingsAction::ConfirmDeleteProfile(profile) => {
                    confirm_popup::set_pending(confirm_popup::ConfirmTarget::ConfigProfile {
                        profile,
                    });
                    self.focused = FocusedArea::ConfirmDelete;
                    return Ok(());
                }
                widgets::settings::SettingsAction::Error(message) => {
                    error_buffer::push_error(error_buffer::ErrorEvent {
                        id: 0,
                        level: tracing::Level::ERROR,
                        message,
                        pushed_at: std::time::Instant::now(),
                    });
                    return Ok(());
                }
                widgets::settings::SettingsAction::None => {}
            }
            if editing_profile {
                return Ok(());
            }
        }

        if self.focused == FocusedArea::GlobalSettings {
            let action = self
                .global_settings
                .as_mut()
                .map(|state| state.handle_key(&key_event))
                .unwrap_or(widgets::popups::global_settings::Action::Close);
            match action {
                widgets::popups::global_settings::Action::None => {}
                widgets::popups::global_settings::Action::Error(message) => {
                    error_buffer::push_error(error_buffer::ErrorEvent {
                        id: 0,
                        level: tracing::Level::ERROR,
                        message,
                        pushed_at: std::time::Instant::now(),
                    });
                }
                widgets::popups::global_settings::Action::ConfirmJavaAuto => {
                    confirm_popup::set_pending(confirm_popup::ConfirmTarget::AutomaticSelection {
                        setting: confirm_popup::AutomaticSetting::Java,
                        instance: None,
                    });
                    self.focused = FocusedArea::ConfirmDelete;
                }
                widgets::popups::global_settings::Action::ClearCache => {
                    confirm_popup::set_pending(confirm_popup::ConfirmTarget::LauncherCache);
                    self.focused = FocusedArea::ConfirmDelete;
                }
                widgets::popups::global_settings::Action::Close => {
                    self.global_settings = None;
                    self.focused = self.pre_overlay_focused;
                }
                widgets::popups::global_settings::Action::OpenRaw(path) => {
                    self.pending_editor = Some(path);
                    self.global_settings = None;
                    self.focused = self.pre_overlay_focused;
                }
                widgets::popups::global_settings::Action::Save(config) => {
                    self.apply_global_settings(*config);
                }
            }
            return Ok(());
        }

        if self.focused == FocusedArea::InstanceSettings {
            let action = self
                .instance_settings
                .as_mut()
                .map(|state| state.handle_key(&key_event))
                .unwrap_or(widgets::popups::instance_settings::Action::Close);
            match action {
                widgets::popups::instance_settings::Action::None => {}
                widgets::popups::instance_settings::Action::Error(message) => {
                    error_buffer::push_error(error_buffer::ErrorEvent {
                        id: 0,
                        level: tracing::Level::ERROR,
                        message,
                        pushed_at: std::time::Instant::now(),
                    });
                }
                widgets::popups::instance_settings::Action::Warning(message) => {
                    error_buffer::push_message(tracing::Level::WARN, message);
                }
                widgets::popups::instance_settings::Action::Close => {
                    self.instance_settings = None;
                    self.focused = self.pre_overlay_focused;
                }
                widgets::popups::instance_settings::Action::OpenRaw => {
                    if let Some(instance) = self.instances_state.selected_instance() {
                        self.pending_editor = Some(
                            self.instance_manager
                                .instances_dir
                                .join(&instance.name)
                                .join("instance.json"),
                        );
                    }
                    self.instance_settings = None;
                    self.focused = self.pre_overlay_focused;
                }
                widgets::popups::instance_settings::Action::Save(updated, desktop) => {
                    self.apply_instance_settings(*updated, desktop);
                }
                widgets::popups::instance_settings::Action::ConfirmRuntime { name } => {
                    confirm_popup::set_pending(confirm_popup::ConfirmTarget::InstanceRuntime {
                        name,
                    });
                    self.focused = FocusedArea::ConfirmDelete;
                }
                widgets::popups::instance_settings::Action::ConfirmJavaAuto { name } => {
                    confirm_popup::set_pending(confirm_popup::ConfirmTarget::AutomaticSelection {
                        setting: confirm_popup::AutomaticSetting::Java,
                        instance: Some(name),
                    });
                    self.focused = FocusedArea::ConfirmDelete;
                }
                widgets::popups::instance_settings::Action::ConfirmAccountAuto { name } => {
                    confirm_popup::set_pending(confirm_popup::ConfirmTarget::AutomaticSelection {
                        setting: confirm_popup::AutomaticSetting::Account,
                        instance: Some(name),
                    });
                    self.focused = FocusedArea::ConfirmDelete;
                }
            }
            return Ok(());
        }

        match self.focused {
            FocusedArea::Popup => {
                new_instance::handle_key(&key_event, &mut self.instances_state);
            }
            FocusedArea::ImportPopup => {
                import_modpack::handle_key(&key_event, &mut self.instances_state);
            }
            _ => {
                if self.focused == FocusedArea::Instances && self.instances_state.renaming.is_some()
                {
                    match key_event.code {
                        KeyCode::Enter => {
                            let new_name = self.instances_state.renaming.take().unwrap_or_default();
                            if let Some(inst) = self.instances_state.selected_instance() {
                                let old_name = inst.name.clone();
                                match self.instance_manager.rename(&old_name, &new_name) {
                                    Ok(()) => {
                                        if let Some(inst) = self
                                            .instances_state
                                            .instances
                                            .iter_mut()
                                            .find(|i| i.name == old_name)
                                        {
                                            inst.name = new_name.trim().to_owned();
                                        }
                                    }
                                    Err(e) => {
                                        tracing::error!(
                                            "Failed to rename instance '{}': {}",
                                            old_name,
                                            e
                                        );
                                        error_buffer::push_error(error_buffer::ErrorEvent {
                                            id: 0,
                                            level: tracing::Level::ERROR,
                                            message: format!("Rename failed: {e}"),
                                            pushed_at: std::time::Instant::now(),
                                        });
                                    }
                                }
                            }
                        }
                        KeyCode::Esc => {
                            self.instances_state.renaming = None;
                        }
                        KeyCode::Backspace => {
                            if let Some(ref mut name) = self.instances_state.renaming {
                                widgets::search::backspace(name, key_event.modifiers);
                            }
                        }
                        KeyCode::Char(c) => {
                            if let Some(ref mut name) = self.instances_state.renaming {
                                name.push(c);
                            }
                        }
                        _ => {}
                    }
                    return Ok(());
                }

                if self.focused == FocusedArea::Instances && self.instances_state.search.active {
                    self.instances_state.handle_key(&key_event);
                    return Ok(());
                }

                match key_event.code {
                    KeyCode::Char('q') => self.exit = true,
                    KeyCode::Esc
                        if matches!(
                            self.focused,
                            FocusedArea::Content
                                | FocusedArea::Account
                                | FocusedArea::Settings
                                | FocusedArea::Overview
                        ) =>
                    {
                        self.focused = FocusedArea::Instances;
                    }
                    KeyCode::Char('I') => self.focused = FocusedArea::Instances,
                    KeyCode::Char('C') => self.focused = FocusedArea::Content,
                    KeyCode::Char('A') => self.focused = FocusedArea::Account,
                    KeyCode::Char('S') => self.focused = FocusedArea::Settings,
                    KeyCode::Char('E') => {
                        self.open_instance_settings(self.focused);
                    }
                    KeyCode::Char('G') => {
                        self.pre_overlay_focused = self.focused;
                        self.global_settings = Some(
                            widgets::popups::global_settings::State::with_detected_image_protocol(
                                self.detected_image_protocol,
                            ),
                        );
                        self.focused = FocusedArea::GlobalSettings;
                    }
                    KeyCode::Char('O') => {
                        self.pre_overlay_focused = self.focused;
                        self.focused = FocusedArea::OverviewExpanded;
                    }
                    KeyCode::Tab if self.focused == FocusedArea::Content => {
                        self.content_mode = self.content_mode.toggle();
                        self.content_tab = match (self.content_mode, self.content_tab) {
                            (
                                widgets::content::ContentMode::Installed,
                                widgets::content::ContentTab::DataPacks,
                            ) => widgets::content::ContentTab::Worlds,
                            (
                                widgets::content::ContentMode::Discover,
                                widgets::content::ContentTab::Worlds,
                            ) => widgets::content::ContentTab::DataPacks,
                            (widgets::content::ContentMode::Discover, tab)
                                if !matches!(
                                    tab,
                                    widgets::content::ContentTab::Mods
                                        | widgets::content::ContentTab::ResourcePacks
                                        | widgets::content::ContentTab::Shaders
                                        | widgets::content::ContentTab::DataPacks
                                ) =>
                            {
                                widgets::content::ContentTab::Mods
                            }
                            (_, tab) => tab,
                        };
                        if self.content_mode == widgets::content::ContentMode::Discover {
                            self.open_world_datapacks = None;
                        }
                        let installed =
                            self.content_mode == widgets::content::ContentMode::Installed;
                        if let Some(state) = self.active_discovery_state_mut() {
                            state.set_local_mode(installed);
                        }
                        self.ensure_active_discovery_loaded();
                    }
                    KeyCode::Char('l') | KeyCode::Right if self.focused == FocusedArea::Content => {
                        self.content_tab = self.content_tab.next_for_mode(self.content_mode);
                        self.ensure_active_discovery_loaded();
                    }
                    KeyCode::Char('h') | KeyCode::Left if self.focused == FocusedArea::Content => {
                        self.content_tab = self.content_tab.previous_for_mode(self.content_mode);
                        self.ensure_active_discovery_loaded();
                    }
                    KeyCode::Char('d')
                        if self.focused == FocusedArea::Instances
                            && !self.instances_state.search.active =>
                    {
                        if let Some(instance) = self.instances_state.selected_instance() {
                            let name = instance.name.clone();
                            if self.pending_instance_settings_updates.contains(&name) {
                                error_buffer::push_message(
                                    tracing::Level::WARN,
                                    super::app::RUNTIME_UPDATE_PENDING_MESSAGE,
                                );
                            } else {
                                confirm_popup::set_pending_instance_delete(&name);
                                self.focused = FocusedArea::ConfirmDelete;
                            }
                        }
                    }
                    KeyCode::Enter
                        if self.focused == FocusedArea::Instances
                            && !self.instances_state.search.active
                            && key_event.modifiers.contains(KeyModifiers::SHIFT) =>
                    {
                        if let Some(instance) = self.instances_state.selected_instance() {
                            let dir = self
                                .instance_manager
                                .instances_dir
                                .join(&instance.name)
                                .join(crate::storage::MINECRAFT_DIR_NAME);
                            if let Err(error) = open::that_detached(&dir) {
                                tracing::error!("Failed to open instance directory: {}", error);
                                error_buffer::push_message(
                                    tracing::Level::ERROR,
                                    format!("Could not open {}: {error}", dir.display()),
                                );
                            }
                        }
                    }
                    KeyCode::Enter
                        if self.focused == FocusedArea::Instances
                            && !self.instances_state.search.active =>
                    {
                        self.focused = FocusedArea::Content;
                    }
                    // only allow launching if instance isn't already running.
                    // crashed instances can be relaunched (clears old state first)
                    KeyCode::Char('l')
                        if self.focused == FocusedArea::Instances
                            && !self.instances_state.search.active =>
                    {
                        if let Some(instance) = self.instances_state.selected_instance().cloned() {
                            self.spawn_launch(instance, None);
                        }
                    }
                    KeyCode::Char('u')
                        if self.focused == FocusedArea::Instances
                            && !self.instances_state.search.active =>
                    {
                        self.spawn_modpack_update();
                    }
                    KeyCode::Char('v')
                        if self.focused == FocusedArea::Instances
                            && !self.instances_state.search.active =>
                    {
                        self.open_modpack_versions();
                    }
                    KeyCode::Char('r')
                        if self.focused == FocusedArea::Instances
                            && !self.instances_state.search.active =>
                    {
                        if let Some(inst) = self.instances_state.selected_instance() {
                            if self.pending_instance_settings_updates.contains(&inst.name) {
                                error_buffer::push_message(
                                    tracing::Level::WARN,
                                    super::app::RUNTIME_UPDATE_PENDING_MESSAGE,
                                );
                            } else {
                                self.instances_state.renaming = Some(inst.name.clone());
                            }
                        }
                    }
                    KeyCode::Esc
                        if self.focused == FocusedArea::Instances
                            && !self.instances_state.search.active =>
                    {
                        if let Some(instance) = self.instances_state.selected_instance() {
                            crate::instance::runtime::send_kill(&instance.name);
                        }
                    }
                    _ => {}
                }

                if self.focused == FocusedArea::Instances {
                    self.instances_state.handle_key(&key_event)
                }
            }
        }

        if self.instances_state.wants_popup() {
            self.focused = FocusedArea::Popup;
        } else if self.focused == FocusedArea::Popup {
            self.focused = FocusedArea::Instances;
        }

        if self.instances_state.wants_import_popup() {
            if self.focused != FocusedArea::ImportPopup {
                import_modpack::open();
            }
            self.focused = FocusedArea::ImportPopup;
        } else if self.focused == FocusedArea::ImportPopup {
            self.focused = FocusedArea::Instances;
        }

        Ok(())
    }

    fn spawn_modpack_update(&mut self) {
        if self
            .instances_state
            .selected_instance()
            .is_some_and(|instance| {
                self.pending_instance_settings_updates
                    .contains(&instance.name)
            })
        {
            error_buffer::push_message(
                tracing::Level::WARN,
                super::app::RUNTIME_UPDATE_PENDING_MESSAGE,
            );
            return;
        }
        let Some(target) = self.instances_state.selected_modpack_update() else {
            return;
        };
        self.spawn_modpack_refresh(target, widgets::popups::modpack_update::Action::Update);
    }

    fn open_modpack_versions(&mut self) {
        let Some(instance) = self.instances_state.selected_instance() else {
            return;
        };
        if self
            .pending_instance_settings_updates
            .contains(&instance.name)
        {
            error_buffer::push_message(
                tracing::Level::WARN,
                super::app::RUNTIME_UPDATE_PENDING_MESSAGE,
            );
            return;
        }
        let Some(source) = instance.modpack_source.clone() else {
            return;
        };
        let mut state = widgets::content::DiscoveryState::new_modpacks();
        let Some(request) = state.begin_managed_modpack_versions(&instance.name, source) else {
            return;
        };
        self.modpack_versions_state = Some(state);
        self.spawn_modpack_versions_request(request);
    }

    fn spawn_modpack_versions_request(
        &self,
        request: widgets::content::discovery::VersionsRequest,
    ) {
        tokio::spawn(async move {
            let source = crate::instance::ProviderProject {
                provider: request.provider.clone(),
                project_id: request.project_id.clone(),
                version_id: request.current_version_id.clone().unwrap_or_default(),
            };
            let result = crate::instance::import::provider_versions(&source)
                .await
                .map_err(|error| error.to_string());
            let result = widgets::content::discovery::prepare_project_versions(result, false).await;
            widgets::content::DiscoveryState::push_action_result(
                &request.pending,
                widgets::content::discovery::DiscoveryActionResult::Versions {
                    request_id: request.request_id,
                    project_id: request.project_id,
                    result,
                },
            );
        });
    }

    fn handle_modpack_versions_key(&mut self, key_event: KeyEvent) {
        if key_event.code == KeyCode::Tab {
            let request = self
                .modpack_versions_state
                .as_mut()
                .and_then(widgets::content::DiscoveryState::switch_version_source);
            if let Some(request) = request {
                self.spawn_modpack_versions_request(request);
            }
            return;
        }
        if key_event.code == KeyCode::Enter {
            let target = self
                .modpack_versions_state
                .as_ref()
                .and_then(|state| state.version_popup.as_ref())
                .filter(|popup| !popup.loading)
                .and_then(|popup| popup.selected_version().cloned());
            if let Some(target) = target {
                let action = if self
                    .instances_state
                    .selected_instance()
                    .and_then(|instance| instance.modpack_source.as_ref())
                    .is_some_and(|source| source.version_id == target.id)
                {
                    widgets::popups::modpack_update::Action::Reinstall
                } else {
                    widgets::popups::modpack_update::Action::Change
                };
                self.modpack_versions_state = None;
                self.spawn_modpack_refresh(target, action);
            }
            return;
        }
        let Some(state) = self.modpack_versions_state.as_mut() else {
            return;
        };
        widgets::content::discovery::handle_key(&key_event, state);
        if state.version_popup.is_none() {
            self.modpack_versions_state = None;
        }
    }

    fn spawn_modpack_refresh(
        &mut self,
        target: crate::net::modrinth::VersionInfo,
        action: widgets::popups::modpack_update::Action,
    ) {
        let Some(instance) = self.instances_state.selected_instance().cloned() else {
            return;
        };
        let state = widgets::popups::modpack_update::State::preparing(action);
        let pending = state.pending.clone();
        self.modpack_update_popup = Some(state);
        let instances_dir = self.instance_manager.instances_dir.clone();
        let meta_dir = self.instance_manager.meta_dir.clone();
        tokio::spawn(async move {
            let action = match action {
                widgets::popups::modpack_update::Action::Update => "update",
                widgets::popups::modpack_update::Action::Change => "change",
                widgets::popups::modpack_update::Action::Reinstall => "reinstall",
            };
            let progress = crate::feedback::progress::ProgressTask::start(format!(
                "Preparing modpack {action}"
            ));
            let manager = crate::instance::InstanceManager::new(instances_dir, meta_dir);
            let result =
                crate::instance::import::refresh::prepare(&manager, &instance, target).await;
            if let Err(error) = &result {
                progress.fail(error);
            } else {
                progress.finish();
            }
            if let Ok(mut pending) = pending.lock() {
                pending.push(widgets::popups::modpack_update::PendingResult::Prepared(
                    Box::new(result),
                ));
                crate::feedback::request_redraw();
            }
        });
    }

    fn handle_modpack_update_key(&mut self, key_event: KeyEvent) {
        let Some(state) = self.modpack_update_popup.as_mut() else {
            return;
        };
        match (state.phase, key_event.code) {
            (widgets::popups::modpack_update::Phase::Preparing, KeyCode::Esc)
            | (widgets::popups::modpack_update::Phase::Conflicts, KeyCode::Esc)
            | (widgets::popups::modpack_update::Phase::Review, KeyCode::Esc) => {
                self.modpack_update_popup = None;
            }
            (
                widgets::popups::modpack_update::Phase::Conflicts,
                KeyCode::Char('j') | KeyCode::Down,
            ) => {
                let count = state.plan.as_ref().map_or(0, |plan| plan.conflicts.len());
                state.selected = (state.selected + 1).min(count.saturating_sub(1));
            }
            (
                widgets::popups::modpack_update::Phase::Conflicts,
                KeyCode::Char('k') | KeyCode::Up,
            ) => state.selected = state.selected.saturating_sub(1),
            (widgets::popups::modpack_update::Phase::Conflicts, KeyCode::Char(' ')) => {
                if let Some(replace) = state.replace.get_mut(state.selected) {
                    *replace = !*replace;
                }
            }
            (widgets::popups::modpack_update::Phase::Conflicts, KeyCode::Enter) => {
                state.phase = widgets::popups::modpack_update::Phase::Review;
            }
            (widgets::popups::modpack_update::Phase::Review, KeyCode::Enter)
                if state.plan.is_some() =>
            {
                self.spawn_modpack_update_apply();
            }
            _ => {}
        }
    }

    fn spawn_modpack_update_apply(&mut self) {
        let Some(state) = self.modpack_update_popup.as_mut() else {
            return;
        };
        let action = state.action;
        let replacements = state.replacements();
        let Some(plan) = state.plan.take() else {
            return;
        };
        state.phase = widgets::popups::modpack_update::Phase::Applying;
        let pending = state.pending.clone();
        tokio::spawn(async move {
            let progress = crate::feedback::progress::ProgressTask::start(match action {
                widgets::popups::modpack_update::Action::Update => "Updating modpack",
                widgets::popups::modpack_update::Action::Change => "Changing modpack",
                widgets::popups::modpack_update::Action::Reinstall => "Reinstalling modpack",
            });
            let result = tokio::task::spawn_blocking(move || {
                crate::instance::import::refresh::apply(plan, &replacements)
            })
            .await
            .map_err(|error| error.to_string())
            .and_then(|result| result);
            if let Err(error) = &result {
                progress.fail(error);
            } else {
                progress.finish();
            }
            if let Ok(mut pending) = pending.lock() {
                pending.push(widgets::popups::modpack_update::PendingResult::Applied(
                    Box::new(result),
                ));
                crate::feedback::request_redraw();
            }
        });
    }

    pub(super) fn ensure_active_discovery_loaded(&mut self) {
        if self.content_mode != widgets::content::ContentMode::Discover {
            return;
        }
        let Some(instance) = self.instances_state.selected_instance().cloned() else {
            return;
        };
        let Some(state) = self.active_discovery_state_mut() else {
            return;
        };
        if state.unavailable_message(&instance).is_some() {
            state.set_unavailable(&instance);
            return;
        }
        let needs_search = state.needs_search(&instance);
        let search_due = state.search_due();
        if needs_search || search_due {
            self.spawn_active_discovery_search();
        } else {
            self.spawn_active_discovery_page();
        }
    }

    fn active_discovery_state_mut(
        &mut self,
    ) -> Option<&mut widgets::content::discovery::DiscoveryState> {
        match self.content_tab {
            widgets::content::ContentTab::Mods => Some(&mut self.mods_discovery_state),
            widgets::content::ContentTab::ResourcePacks => {
                Some(&mut self.resource_packs_discovery_state)
            }
            widgets::content::ContentTab::Shaders => Some(&mut self.shaders_discovery_state),
            widgets::content::ContentTab::DataPacks => Some(&mut self.datapacks_discovery_state),
            widgets::content::ContentTab::Worlds if self.open_world_datapacks.is_some() => {
                Some(&mut self.datapacks_discovery_state)
            }
            _ => None,
        }
    }

    pub(super) fn active_discovery_state(
        &self,
    ) -> Option<&widgets::content::discovery::DiscoveryState> {
        match self.content_tab {
            widgets::content::ContentTab::Mods => Some(&self.mods_discovery_state),
            widgets::content::ContentTab::ResourcePacks => {
                Some(&self.resource_packs_discovery_state)
            }
            widgets::content::ContentTab::Shaders => Some(&self.shaders_discovery_state),
            widgets::content::ContentTab::DataPacks => Some(&self.datapacks_discovery_state),
            widgets::content::ContentTab::Worlds if self.open_world_datapacks.is_some() => {
                Some(&self.datapacks_discovery_state)
            }
            _ => None,
        }
    }

    pub(super) fn discovery_activity(&self) -> Option<&'static str> {
        if self.instances_state.show_import_popup {
            return widgets::popups::import_modpack::discovery_activity();
        }
        if self.content_mode != widgets::content::ContentMode::Discover
            || self.instances_state.selected_instance().is_none()
        {
            return None;
        }
        self.active_discovery_state()
            .and_then(widgets::content::discovery::DiscoveryState::activity_label)
    }

    fn spawn_bulk_content_updates(&mut self) {
        if self.content_update_popup.is_some() {
            return;
        }
        let Some(instance) = self.instances_state.selected_instance().cloned() else {
            return;
        };
        let (kind, target_world) = match self.content_tab {
            widgets::content::ContentTab::Mods => (crate::instance::ContentKind::Mod, None),
            widgets::content::ContentTab::ResourcePacks => {
                (crate::instance::ContentKind::ResourcePack, None)
            }
            widgets::content::ContentTab::Shaders => (crate::instance::ContentKind::Shader, None),
            widgets::content::ContentTab::Worlds => (
                crate::instance::ContentKind::DataPack,
                self.open_world_datapacks.clone(),
            ),
            _ => return,
        };
        if kind == crate::instance::ContentKind::DataPack && target_world.is_none() {
            return;
        }
        let Some((_, manifest)) = self
            .content_manifest
            .as_ref()
            .filter(|(name, _)| name == &instance.name)
            .cloned()
        else {
            return;
        };
        let paths = crate::storage::InstancePaths::new(
            self.instance_manager.instances_dir.join(&instance.name),
        );
        let source_entries = match self.content_tab {
            widgets::content::ContentTab::Mods => self.mods_state.entries.clone(),
            widgets::content::ContentTab::ResourcePacks => {
                self.resource_packs_state.entries.clone()
            }
            widgets::content::ContentTab::Shaders => self.shaders_state.entries.clone(),
            widgets::content::ContentTab::Worlds => self.world_datapacks_state.entries.clone(),
            _ => return,
        };
        let cached_snapshot = self
            .content_update_snapshot
            .as_ref()
            .filter(|(name, snapshot)| name == &instance.name && snapshot.applies_to(&instance))
            .map(|(_, snapshot)| snapshot.clone());
        if cached_snapshot
            .as_ref()
            .is_none_or(|snapshot| !snapshot.updates.iter().any(|update| update.kind == kind))
        {
            return;
        }
        let mut state = widgets::content::update::State::checking(
            instance.name.clone(),
            kind,
            target_world.clone(),
            source_entries,
        );
        let pending = state.pending.clone();
        let task = tokio::spawn(async move {
            let progress = crate::feedback::progress::ProgressTask::start("Preparing updates");
            let snapshot = cached_snapshot.expect("checked above");
            let target_directory = target_world
                .as_ref()
                .map(|(_, world)| world.join("datapacks"));
            let mut requests = Vec::new();
            let mut conflicts = Vec::new();
            for record in manifest.files.iter().filter(|record| {
                record.kind == kind
                    && target_directory.as_ref().is_none_or(|directory| {
                        paths
                            .minecraft()
                            .join(&record.relative_path)
                            .starts_with(directory)
                    })
            }) {
                let Some(installed) = record.resolved_project() else {
                    continue;
                };
                let installed_path = paths.minecraft().join(&record.relative_path);
                let title = record
                    .relative_path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .unwrap_or("content")
                    .to_owned();
                if let Some(update) = snapshot.update_for(installed) {
                    requests.push(crate::instance::content::updates::UpdateRequest {
                        title,
                        installed_path,
                        target_world: target_world.as_ref().map(|(_, path)| path.clone()),
                        update: update.clone(),
                    });
                } else if let Some(failure) = snapshot
                    .failures
                    .iter()
                    .find(|failure| failure.installed == *installed && failure.kind == kind)
                {
                    conflicts.push(crate::instance::content::updates::UpdateConflict {
                        title,
                        installed_path,
                        reason: failure.reason.clone(),
                    });
                }
            }
            let plan = crate::instance::content::updates::plan_bulk(
                &instance,
                &manifest,
                &paths.minecraft(),
                requests,
                conflicts,
                Some(&progress.handle()),
            )
            .await;
            progress.finish();
            if let Ok(mut pending) = pending.lock() {
                pending.push(widgets::content::update::PendingResult::Prepared(
                    snapshot, plan,
                ));
                crate::feedback::request_redraw();
            }
        });
        state.abort_handle = Some(task.abort_handle());
        self.content_update_popup = Some(state);
    }

    fn handle_content_update_key(&mut self, key_event: KeyEvent) {
        let Some(state) = self.content_update_popup.as_mut() else {
            return;
        };
        match (state.phase, key_event.code) {
            (widgets::content::update::Phase::Checking, KeyCode::Esc) => {
                state.cancel();
                self.content_update_popup = None;
            }
            (widgets::content::update::Phase::Review, KeyCode::Esc)
            | (widgets::content::update::Phase::Conflicts, KeyCode::Esc) => {
                self.content_update_popup = None;
            }
            (widgets::content::update::Phase::Conflicts, KeyCode::Char('h')) => {
                self.content_update_popup = None;
            }
            (widgets::content::update::Phase::Conflicts, KeyCode::Enter) => {
                if state.has_updates() {
                    state.show_review();
                }
            }
            (widgets::content::update::Phase::Review, KeyCode::Char('h')) => {
                state.show_conflicts();
            }
            (widgets::content::update::Phase::Review, KeyCode::Enter) => {
                self.spawn_bulk_content_install();
            }
            (_, KeyCode::Char('j') | KeyCode::Down | KeyCode::Char('k') | KeyCode::Up) => {
                widgets::content::list::handle_key_no_toggle(&key_event, &mut state.list);
            }
            _ => {}
        }
    }

    fn spawn_bulk_content_install(&mut self) {
        let Some(instance) = self.instances_state.selected_instance().cloned() else {
            return;
        };
        let Some(state) = self.content_update_popup.as_mut() else {
            return;
        };
        if state.instance_name != instance.name {
            self.content_update_popup = None;
            return;
        }
        let Some(plan) = state.plan.as_ref().map(|plan| plan.dependency_plan.clone()) else {
            return;
        };
        if plan.items.is_empty() {
            self.content_update_popup = None;
            return;
        }
        state.phase = widgets::content::update::Phase::Applying;
        let pending = state.pending.clone();
        let instances_dir = self.instance_manager.instances_dir.clone();
        let paths = crate::storage::InstancePaths::new(instances_dir.join(&instance.name));
        tokio::spawn(async move {
            let progress = crate::feedback::progress::ProgressTask::start("Updating content");
            let registry = crate::instance::content::provider::ProviderRegistry::configured(
                crate::net::HttpClient::new(),
            );
            let result = crate::instance::content::dependencies::install(
                &registry,
                &paths.content_manifest(),
                &paths.minecraft(),
                &plan,
            )
            .await
            .map(|_| ())
            .map_err(|error| error.to_string());
            if let Err(error) = &result {
                progress.fail(error);
            } else {
                progress.finish();
                crate::instance::content::reconcile::spawn_after_change(
                    instance,
                    instances_dir,
                    crate::net::HttpClient::new(),
                );
            }
            if let Ok(mut pending) = pending.lock() {
                pending.push(widgets::content::update::PendingResult::Applied(result));
                crate::feedback::request_redraw();
            }
        });
    }

    fn spawn_installed_versions(&mut self) {
        let Some(instance) = self.instances_state.selected_instance().cloned() else {
            return;
        };
        let (entry, kind, target_world) = match self.content_tab {
            widgets::content::ContentTab::Mods => (
                self.mods_state.selected_entry().cloned(),
                crate::instance::ContentKind::Mod,
                None,
            ),
            widgets::content::ContentTab::ResourcePacks => (
                self.resource_packs_state.selected_entry().cloned(),
                crate::instance::ContentKind::ResourcePack,
                None,
            ),
            widgets::content::ContentTab::Shaders => (
                self.shaders_state.selected_entry().cloned(),
                crate::instance::ContentKind::Shader,
                None,
            ),
            widgets::content::ContentTab::Worlds => (
                self.world_datapacks_state.selected_entry().cloned(),
                crate::instance::ContentKind::DataPack,
                self.open_world_datapacks.clone(),
            ),
            _ => return,
        };
        let Some(entry) = entry else {
            return;
        };
        let paths = crate::storage::InstancePaths::new(
            self.instance_manager.instances_dir.join(&instance.name),
        );
        let Ok(relative_path) = entry.path.strip_prefix(paths.minecraft()) else {
            return;
        };
        let Some(record) = self
            .content_manifest
            .as_ref()
            .filter(|(name, _)| name == &instance.name)
            .and_then(|(_, manifest)| manifest.record(relative_path))
            .cloned()
        else {
            return;
        };
        let Some(state) = self.active_discovery_state_mut() else {
            return;
        };
        let Some(request) = state.begin_installed_versions(&entry, &record, target_world) else {
            return;
        };
        self.spawn_discovery_versions_request(instance, kind, request);
    }

    fn spawn_active_discovery_versions(&mut self) {
        let Some(instance) = self.instances_state.selected_instance().cloned() else {
            return;
        };
        let Some(state) = self.active_discovery_state_mut() else {
            return;
        };
        let kind = state.kind;
        let Some(request) = state.begin_versions() else {
            return;
        };
        self.spawn_discovery_versions_request(instance, kind, request);
    }

    fn spawn_active_discovery_version_source(&mut self) {
        let Some(instance) = self.instances_state.selected_instance().cloned() else {
            return;
        };
        let Some(state) = self.active_discovery_state_mut() else {
            return;
        };
        let kind = state.kind;
        let Some(request) = state.switch_version_source() else {
            return;
        };
        self.spawn_discovery_versions_request(instance, kind, request);
    }

    fn spawn_active_discovery_dependencies(&mut self) {
        let Some(mut instance) = self.instances_state.selected_instance().cloned() else {
            return;
        };
        let Some(request) = self
            .active_discovery_state_mut()
            .and_then(|state| state.begin_dependency_resolution())
        else {
            return;
        };
        let paths = crate::storage::InstancePaths::new(
            self.instance_manager.instances_dir.join(&instance.name),
        );
        tokio::spawn(async move {
            if let Some(game_version) = request.game_version {
                instance.game_version = game_version;
            }
            let result = async {
                let manifest = crate::instance::ContentManifest::load(&paths.content_manifest())
                    .map_err(|error| crate::net::NetError::Parse(error.to_string()))?;
                let registry = crate::instance::content::provider::ProviderRegistry::configured(
                    crate::net::HttpClient::new(),
                );
                crate::instance::content::dependencies::resolve(
                    &registry,
                    &manifest,
                    &paths.minecraft(),
                    &instance,
                    request.root,
                )
                .await
            }
            .await
            .map_err(|error| error.to_string());
            widgets::content::DiscoveryState::push_action_result(
                &request.pending,
                widgets::content::discovery::DiscoveryActionResult::Dependencies {
                    request_id: request.request_id,
                    project_id: request.project_id,
                    result,
                },
            );
        });
    }

    fn spawn_discovery_versions_request(
        &self,
        instance: crate::instance::InstanceConfig,
        kind: crate::instance::ContentKind,
        request: widgets::content::discovery::VersionsRequest,
    ) {
        let game_version = if !request.game_version_overrides.is_empty() {
            request.game_version_overrides.join("+")
        } else if request.all_game_versions {
            "any".to_owned()
        } else {
            instance.game_version.clone()
        };
        let version_cache = crate::storage::MetadataPaths::new(&self.instance_manager.meta_dir)
            .provider_versions(&request.provider)
            .join(&request.project_id)
            .join(format!(
                "{}-{}.json",
                game_version,
                instance.loader.to_string().to_lowercase()
            ));
        tokio::spawn(async move {
            let registry = crate::instance::content::provider::ProviderRegistry::configured(
                crate::net::HttpClient::new(),
            );
            let primary_version = if request.game_version_overrides.len() == 1 {
                request.game_version_overrides[0].clone()
            } else if request.all_game_versions {
                String::new()
            } else {
                instance.game_version.clone()
            };
            let current_version_id = request.current_version_id.clone();
            let mut fallback = false;
            let result = match registry.get(&request.provider) {
                Some(provider) => {
                    let mut primary = widgets::content::discovery::fetch_popup_versions(
                        provider,
                        &request.project_id,
                        kind,
                        &primary_version,
                        instance.loader,
                        current_version_id.as_deref(),
                    )
                    .await;
                    if let Ok(versions) = &mut primary
                        && request.game_version_overrides.len() > 1
                    {
                        versions.retain(|version| {
                            version.game_versions.iter().any(|game_version| {
                                request.game_version_overrides.contains(game_version)
                            })
                        });
                    }
                    if let Ok(versions) = &primary
                        && let Ok(bytes) = serde_json::to_vec_pretty(versions)
                    {
                        let _ = crate::storage::write_atomic(&version_cache, &bytes);
                    }
                    match primary {
                        // The list facet matched but no file fits this
                        // version/loader: retry unfiltered so the popup offers
                        // the version picker instead of a dead end.
                        Ok(versions) if versions.is_empty() && !request.all_game_versions => {
                            fallback = true;
                            widgets::content::discovery::fetch_popup_versions(
                                provider,
                                &request.project_id,
                                kind,
                                "",
                                instance.loader,
                                current_version_id.as_deref(),
                            )
                            .await
                        }
                        Ok(versions) => Ok(versions),
                        Err(error) => match std::fs::read(&version_cache)
                            .ok()
                            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                        {
                            Some(versions) => Ok(versions),
                            None => Err(error),
                        },
                    }
                }
                None => Err(format!(
                    "{} content provider is unavailable",
                    request.provider
                )),
            };
            let result = widgets::content::discovery::prepare_project_versions(
                result,
                request.all_game_versions || fallback,
            )
            .await;
            widgets::content::DiscoveryState::push_action_result(
                &request.pending,
                if fallback {
                    widgets::content::discovery::DiscoveryActionResult::VersionsUnfiltered {
                        request_id: request.request_id,
                        project_id: request.project_id,
                        result,
                    }
                } else {
                    widgets::content::discovery::DiscoveryActionResult::Versions {
                        request_id: request.request_id,
                        project_id: request.project_id,
                        result,
                    }
                },
            );
        });
    }

    fn spawn_active_discovery_project_page(&mut self) {
        let Some(request) = self
            .active_discovery_state_mut()
            .and_then(widgets::content::DiscoveryState::begin_project_page)
        else {
            return;
        };
        widgets::content::discovery::spawn_project_page(request);
    }

    fn spawn_active_discovery_install(&mut self) {
        let Some(instance) = self.instances_state.selected_instance().cloned() else {
            return;
        };
        let Some(state) = self.active_discovery_state_mut() else {
            return;
        };
        let kind = state.kind;
        let Some(request) = state.begin_install() else {
            return;
        };
        let instances_dir = self.instance_manager.instances_dir.clone();
        let instance_paths = crate::storage::InstancePaths::new(instances_dir.join(&instance.name));
        let manifest_path = instance_paths.content_manifest();
        let minecraft_dir = instance_paths.minecraft();
        let destination = match (&request.target_world, kind) {
            (Some((_, world)), crate::instance::ContentKind::DataPack) => world.join("datapacks"),
            _ => minecraft_dir.join(kind.directory()),
        };
        tokio::spawn(async move {
            let action = if request.installed_path.is_some() {
                format!("Changing {} version", request.project_title)
            } else {
                format!("Installing {}", request.project_title)
            };
            let progress = crate::feedback::progress::ProgressTask::start(action);
            progress.set_sub_action(&request.version.version_number);
            let client = crate::net::HttpClient::new();
            let result = async {
                if kind == crate::instance::ContentKind::DataPack
                    && (request.target_world.is_none()
                        || !destination.starts_with(minecraft_dir.join("saves")))
                {
                    return Err(crate::net::NetError::Parse(
                        "Invalid datapack target world".to_owned(),
                    ));
                }
                let registry =
                    crate::instance::content::provider::ProviderRegistry::configured(client);
                let plan = request.dependency_plan.as_ref().ok_or_else(|| {
                    crate::net::NetError::Parse(
                        "Installation has no resolved dependency plan".to_owned(),
                    )
                })?;
                let installed = crate::instance::content::dependencies::install(
                    &registry,
                    &manifest_path,
                    &minecraft_dir,
                    plan,
                )
                .await?;
                Ok::<_, crate::net::NetError>(widgets::content::discovery::InstallCompletion {
                    path: installed.root_path,
                    replaced: installed.replaced,
                    skipped: installed.skipped,
                    orphaned_dependencies: installed.orphaned_dependencies,
                })
            }
            .await
            .map_err(|error| error.to_string());
            if let Err(error) = &result {
                progress.fail(error);
            } else {
                progress.finish();
                crate::instance::content::reconcile::spawn_after_change(
                    instance,
                    instances_dir,
                    crate::net::HttpClient::new(),
                );
            }
            widgets::content::DiscoveryState::push_action_result(
                &request.pending,
                widgets::content::discovery::DiscoveryActionResult::Install {
                    request_id: request.request_id,
                    generation: request.generation,
                    project_id: request.project_id,
                    project_title: request.project_title,
                    result,
                },
            );
        });
    }

    fn spawn_active_discovery_search(&mut self) {
        let Some(instance) = self.instances_state.selected_instance().cloned() else {
            return;
        };
        let manifest = self
            .content_manifest
            .as_ref()
            .filter(|(name, _)| name == &instance.name)
            .map(|(_, manifest)| manifest.clone());
        let minecraft_dir = crate::storage::InstancePaths::new(
            self.instance_manager.instances_dir.join(&instance.name),
        )
        .minecraft();
        let meta_dir = self.instance_manager.meta_dir.clone();
        let Some(state) = self.active_discovery_state_mut() else {
            return;
        };
        if state.unavailable_message(&instance).is_some() {
            state.set_unavailable(&instance);
            return;
        }
        let kind = state.kind;
        let query = state.search.query.clone();
        let request = state.begin_search(&instance);
        if request.cached
            && let Some(manifest) = &manifest
        {
            state.refresh_installed_manifest(manifest, &minecraft_dir);
        }
        Self::spawn_discovery_request(
            instance,
            kind,
            query,
            manifest,
            minecraft_dir,
            meta_dir,
            request,
        );
    }

    fn spawn_active_discovery_page(&mut self) {
        let Some(instance) = self.instances_state.selected_instance().cloned() else {
            return;
        };
        let manifest = self
            .content_manifest
            .as_ref()
            .filter(|(name, _)| name == &instance.name)
            .map(|(_, manifest)| manifest.clone());
        let minecraft_dir = crate::storage::InstancePaths::new(
            self.instance_manager.instances_dir.join(&instance.name),
        )
        .minecraft();
        let meta_dir = self.instance_manager.meta_dir.clone();
        let Some(state) = self.active_discovery_state_mut() else {
            return;
        };
        if state.unavailable_message(&instance).is_some() {
            state.set_unavailable(&instance);
            return;
        }
        let kind = state.kind;
        let query = state.search.query.clone();
        let Some(request) = state.begin_next_page() else {
            return;
        };
        Self::spawn_discovery_request(
            instance,
            kind,
            query,
            manifest,
            minecraft_dir,
            meta_dir,
            request,
        );
    }

    fn spawn_discovery_request(
        instance: crate::instance::InstanceConfig,
        kind: crate::instance::ContentKind,
        query: String,
        manifest: Option<crate::instance::ContentManifest>,
        minecraft_dir: std::path::PathBuf,
        meta_dir: std::path::PathBuf,
        request: widgets::content::discovery::DiscoveryRequest,
    ) {
        widgets::content::discovery::spawn_provider_search(
            query,
            widgets::content::discovery::DiscoveryTarget::Content(Box::new(
                widgets::content::discovery::ContentDiscoveryTarget {
                    instance,
                    kind,
                    manifest,
                    minecraft_dir,
                },
            )),
            meta_dir,
            request,
        );
    }

    fn delete_config_profile(&mut self, profile: &str) -> color_eyre::Result<()> {
        let instances = self.instance_manager.load_all();
        if let Some(instance) = instances.iter().find(|instance| {
            instance.config_sync_profile.as_deref() == Some(profile)
                && crate::instance::runtime::is_active(&instance.name)
        }) {
            return Err(color_eyre::eyre::eyre!(
                "Stop '{}' before deleting its active config profile",
                instance.name
            ));
        }
        for instance in instances
            .into_iter()
            .filter(|instance| instance.config_sync_profile.as_deref() == Some(profile))
        {
            let mut updated = instance.clone();
            crate::instance::config_sync::switch_profile_and_save(
                &self.instance_manager,
                &mut updated,
                None,
            )?;
            self.instances_state
                .replace_instance(&instance.name, updated);
        }

        crate::instance::config_sync::delete_profile(&self.instance_manager.meta_dir, profile)?;
        Ok(())
    }

    fn confirm_content_delete(&mut self, pending: widgets::content::list::PendingContentDelete) {
        let dependents = self
            .content_manifest_for_path(&pending.path)
            .map(|(manifest, relative_path, _)| {
                manifest
                    .dependent_paths(&relative_path)
                    .into_iter()
                    .map(|path| {
                        path.file_stem()
                            .and_then(|name| name.to_str())
                            .unwrap_or("mod")
                            .to_owned()
                    })
                    .collect()
            })
            .unwrap_or_default();
        confirm_popup::set_pending_managed_content_delete(pending.name, pending.path, dependents);
        self.focused = FocusedArea::ConfirmDelete;
    }

    fn delete_content_path(
        &mut self,
        path: &std::path::Path,
        orphan_only: bool,
    ) -> std::io::Result<Vec<std::path::PathBuf>> {
        let instance = self
            .instances_state
            .selected_instance()
            .ok_or_else(|| std::io::Error::other("No instance is selected"))?;
        let name = instance.name.clone();
        let paths =
            crate::storage::InstancePaths::new(self.instance_manager.instances_dir.join(&name));
        let minecraft_dir = paths.minecraft();
        let relative = crate::instance::content::local::relative_path(&minecraft_dir, path)?;
        let manifest = match self
            .content_manifest
            .as_ref()
            .filter(|(cached_name, _)| cached_name == &name)
        {
            Some((_, manifest)) => manifest.clone(),
            None => crate::instance::ContentManifest::load(&paths.content_manifest())
                .map_err(std::io::Error::other)?,
        };
        let expected = manifest
            .files
            .iter()
            .filter(|record| record.relative_path.starts_with(&relative))
            .cloned()
            .collect::<Vec<_>>();
        let (manifest, orphaned) = crate::instance::content::local::remove(
            &paths.content_manifest(),
            &minecraft_dir,
            path,
            &expected,
            orphan_only,
        )?;
        self.content_manifest = Some((name, manifest));
        self.apply_cached_content_manifest();
        Ok(orphaned)
    }

    fn content_manifest_for_path(
        &self,
        path: &std::path::Path,
    ) -> Option<(
        crate::instance::ContentManifest,
        std::path::PathBuf,
        std::path::PathBuf,
    )> {
        let instance = self.instances_state.selected_instance()?;
        let paths = crate::storage::InstancePaths::new(
            self.instance_manager.instances_dir.join(&instance.name),
        );
        let minecraft_dir = paths.minecraft();
        let relative_path = path.strip_prefix(&minecraft_dir).ok()?.to_owned();
        let manifest = crate::instance::ContentManifest::load(&paths.content_manifest()).ok()?;
        Some((manifest, relative_path, minecraft_dir))
    }

    fn remove_content_path_from_states(&mut self, path: &std::path::Path) {
        self.mods_state.remove_path(path);
        self.resource_packs_state.remove_path(path);
        self.shaders_state.remove_path(path);
        self.world_datapacks_state.remove_path(path);
        self.worlds_state.remove_path(path);
        self.screenshots_state.remove_path(path);
        self.logs_state.remove_path(path);
        if path
            .parent()
            .and_then(std::path::Path::file_name)
            .is_some_and(|name| name == "datapacks")
            && let Some(world_path) = path.parent().and_then(std::path::Path::parent)
            && let Some(world) = self
                .worlds_state
                .entries
                .iter_mut()
                .find(|world| world.path == world_path)
            && let Some(details) = world.world_details.as_mut()
        {
            details.datapacks = crate::instance::content::worlds::datapack_names(world_path);
        }
    }

    fn open_instance_settings(&mut self, return_focus: FocusedArea) {
        let Some(instance) = self.instances_state.selected_instance().cloned() else {
            return;
        };
        let mut state = widgets::popups::instance_settings::State::with_accounts(
            &instance,
            &self.instance_manager.meta_dir,
            &crate::storage::InstancePaths::new(
                self.instance_manager.instances_dir.join(&instance.name),
            )
            .minecraft(),
            self.account_state.store.accounts.clone(),
        );
        if self
            .pending_instance_settings_updates
            .contains(&instance.name)
        {
            state.mark_runtime_update_pending();
        }
        self.pre_overlay_focused = return_focus;
        self.instance_settings = Some(state);
        self.focused = FocusedArea::InstanceSettings;
    }

    fn apply_global_settings(&mut self, config: crate::config::Config) {
        let result = crate::config::SETTINGS.save_launcher_settings(config);
        match result {
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
                if outcome.restart_required && outcome.restart_changed {
                    error_buffer::push_message(
                        tracing::Level::INFO,
                        "Restart rmcl to apply storage and image changes",
                    );
                }
                crate::feedback::request_redraw();
            }
            Err(error) => {
                if let Some(state) = self.global_settings.as_mut() {
                    state.mark_save_failed();
                }
                error_buffer::push_message(tracing::Level::ERROR, error.to_string());
            }
        }
    }

    pub(super) fn apply_instance_settings(
        &mut self,
        mut updated: crate::instance::models::InstanceConfig,
        desktop: bool,
    ) {
        let Some(previous) = self.instances_state.selected_instance().cloned() else {
            return;
        };
        let structural_change = previous.game_version != updated.game_version
            || previous.loader != updated.loader
            || previous.loader_version != updated.loader_version;
        if structural_change {
            if crate::instance::runtime::is_active(&previous.name) {
                error_buffer::push_error(error_buffer::ErrorEvent {
                    id: 0,
                    level: tracing::Level::ERROR,
                    message: "Stop the instance before changing its runtime".to_owned(),
                    pushed_at: std::time::Instant::now(),
                });
                if let Some(state) = self.instance_settings.as_mut() {
                    let remaining = state.cancel_runtime_change();
                    if let Some((updated, desktop)) = remaining {
                        self.apply_instance_settings(*updated, desktop);
                    }
                }
                return;
            }
            self.pending_instance_settings_updates
                .insert(previous.name.clone());
            self.spawn_instance_settings_update(previous, updated, desktop);
            if let Some(state) = self.instance_settings.as_mut() {
                state.mark_runtime_update_pending();
            }
            return;
        }

        let current = match self.instance_manager.load_one(&previous.name) {
            Ok(current) => current,
            Err(error) => {
                if let Some(state) = self.instance_settings.as_mut() {
                    state.mark_save_failed();
                }
                error_buffer::push_message(tracing::Level::ERROR, error.to_string());
                return;
            }
        };
        updated = super::event::merge_instance_settings(&previous, &updated, current);
        match self.instance_manager.save(&updated) {
            Ok(()) => {
                let shortcut_result = crate::instance::desktop::set_enabled(&updated, desktop);
                let saved_desktop = match shortcut_result {
                    Ok(()) => desktop,
                    Err(error) => {
                        error_buffer::push_error(error_buffer::ErrorEvent {
                            id: 0,
                            level: tracing::Level::ERROR,
                            message: format!("Instance saved, but shortcut update failed: {error}"),
                            pushed_at: std::time::Instant::now(),
                        });
                        crate::instance::desktop::exists(&updated.name)
                    }
                };
                self.instances_state
                    .replace_instance(&previous.name, updated.clone());
                if let Some(state) = self.instance_settings.as_mut() {
                    state.mark_saved(&updated, saved_desktop);
                }
            }
            Err(error) => {
                if let Some(state) = self.instance_settings.as_mut() {
                    state.mark_save_failed();
                }
                error_buffer::push_error(error_buffer::ErrorEvent {
                    id: 0,
                    level: tracing::Level::ERROR,
                    message: error.to_string(),
                    pushed_at: std::time::Instant::now(),
                });
            }
        }
    }

    pub(super) fn reset_discovery_states(&mut self) {
        crate::instance::content::updates::cancel(None);
        self.reconciliation_for = None;
        for cached in self.cached_instance_content.values_mut() {
            cached.reconciliation_for = None;
        }
        self.mods_discovery_state =
            widgets::content::DiscoveryState::new(crate::instance::ContentKind::Mod);
        self.resource_packs_discovery_state =
            widgets::content::DiscoveryState::new(crate::instance::ContentKind::ResourcePack);
        self.shaders_discovery_state =
            widgets::content::DiscoveryState::new(crate::instance::ContentKind::Shader);
        self.datapacks_discovery_state =
            widgets::content::DiscoveryState::new(crate::instance::ContentKind::DataPack);
    }

    pub(super) fn queue_content_update_checks(&mut self) {
        self.content_update_check_pending = true;
        for cached in self.cached_instance_content.values_mut() {
            cached.update_check_pending = true;
        }
    }

    pub(super) fn spawn_selected_content_update_check(&self) {
        let Some(instance) = self.instances_state.selected_instance().cloned() else {
            return;
        };
        let Some((name, manifest)) = self.content_manifest.as_ref() else {
            return;
        };
        if name != &instance.name {
            return;
        }
        let path = crate::storage::InstancePaths::new(
            self.instance_manager.instances_dir.join(&instance.name),
        )
        .content_updates();
        crate::instance::content::updates::spawn(
            instance,
            manifest.clone(),
            path,
            self.content_update_priority(),
            self.content_update_snapshot
                .as_ref()
                .map(|(_, snapshot)| snapshot.clone()),
        );
    }

    pub(super) fn active_installed_content_state(
        &self,
    ) -> &widgets::content::list::ContentListState {
        match self.content_tab {
            widgets::content::ContentTab::ResourcePacks => &self.resource_packs_state,
            widgets::content::ContentTab::Shaders => &self.shaders_state,
            widgets::content::ContentTab::Worlds => &self.world_datapacks_state,
            _ => &self.mods_state,
        }
    }

    pub(super) fn content_update_priority(&self) -> Vec<crate::instance::ProviderProject> {
        [
            self.active_installed_content_state(),
            &self.mods_state,
            &self.resource_packs_state,
            &self.shaders_state,
            &self.world_datapacks_state,
        ]
        .into_iter()
        .flat_map(|state| {
            state
                .filtered_indices()
                .into_iter()
                .filter_map(|index| state.entries[index].provider_project.clone())
                .chain(
                    state
                        .entries
                        .iter()
                        .filter_map(|entry| entry.provider_project.clone()),
                )
        })
        .collect()
    }
}

pub(super) fn project_link_url(link: &str) -> std::io::Result<reqwest::Url> {
    let url = reqwest::Url::parse(link)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Project links must use HTTP or HTTPS",
        ));
    }
    Ok(url)
}
