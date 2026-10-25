// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;
use crate::tui::tests::harness::UiHarness;

fn add_instance(ui: &mut UiHarness, name: &str) {
    ui.add_instance(name);
    std::fs::create_dir_all(ui.instance_path(name).join("minecraft")).unwrap();
}

#[test]
fn export_format_rotates_inline_and_only_multimc_has_a_bundled_badge() {
    let mut ui = UiHarness::new();
    add_instance(&mut ui, "Example");
    ui.key(KeyCode::Char('e'));
    assert!(ui.app.export_popup.as_ref().unwrap().pending.is_some());
    assert!(!ui.app.export_popup.as_ref().unwrap().text_input_active());
    ui.key(KeyCode::Char('l'));
    assert_eq!(
        ui.app.export_popup.as_ref().unwrap().format,
        PackFormat::Mrpack
    );
    ui.draw();
    assert!(ui.screen().contains(".mrpack"));
    assert!(!ui.screen().contains("Bundled"));
    ui.key(KeyCode::Right);
    assert_eq!(
        ui.app.export_popup.as_ref().unwrap().format,
        PackFormat::Mmc
    );
    ui.draw();
    assert!(ui.screen().contains("Bundled"));
    ui.key(KeyCode::Char('h'));
    assert_eq!(
        ui.app.export_popup.as_ref().unwrap().format,
        PackFormat::Mrpack
    );
    ui.key(KeyCode::Left);
    assert_eq!(
        ui.app.export_popup.as_ref().unwrap().format,
        PackFormat::CurseForge
    );
    ui.key(KeyCode::Char('h'));
    assert_eq!(
        ui.app.export_popup.as_ref().unwrap().format,
        PackFormat::Mmc
    );
    ui.key(KeyCode::Enter);
    assert_eq!(
        ui.app.export_popup.as_ref().unwrap().format,
        PackFormat::CurseForge
    );
    assert!(Field::Author.supported(ui.app.export_popup.as_ref().unwrap().format));
    ui.key(KeyCode::Char('q'));
    ui.key(KeyCode::Char('d'));
    assert!(!ui.app.exit);
    assert!(super::super::confirm::pending_target().is_none());
    ui.key(KeyCode::Esc);
    assert!(ui.app.export_popup.is_none());
}

#[test]
fn export_form_uses_settings_layout_without_a_review_or_format_picker() {
    let mut ui = UiHarness::new();
    add_instance(&mut ui, "Example");
    let mut state = State::new(ui.app.instances_state.selected_instance().unwrap());
    state.fields[2] = "Example Author".to_owned();
    state.fields[3] = "~/Downloads/example.zip".to_owned();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 26)).unwrap();
    terminal.draw(|frame| render(frame, &mut state)).unwrap();
    insta::assert_snapshot!("export_form", terminal.backend());
    state.rotate_format(false);
    assert_eq!(state.fields[3], "~/Downloads/example.zip");
    terminal.draw(|frame| render(frame, &mut state)).unwrap();
    insta::assert_snapshot!("export_format_bundled", terminal.backend());
    assert!(terminal.backend().to_string().contains(".zip   Bundled"));
    assert!(!terminal.backend().to_string().contains("Review export"));
}

#[test]
fn export_form_keeps_unsupported_rows_and_geometry_without_helper_text() {
    let mut ui = UiHarness::new();
    add_instance(&mut ui, "Example");
    let mut state = State::new(ui.app.instances_state.selected_instance().unwrap());
    assert_eq!(
        state.fields[2],
        std::env::var(if cfg!(windows) { "USERNAME" } else { "USER" }).unwrap_or_default()
    );
    state.fields[1] = "2.3.4".to_owned();
    state.fields[2] = "Example Author".to_owned();
    state.fields[3] = "~/Downloads/example.zip".to_owned();
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 28)).unwrap();
    let mut positions = None;
    for format in PackFormat::ALL {
        state.format = format;
        terminal.draw(|frame| render(frame, &mut state)).unwrap();
        let screen = terminal.backend().to_string();
        let current = [
            "Export Modpack ·",
            "Pack version",
            "Author",
            "Output file",
            "Included files",
            "╰",
        ]
        .map(|label| {
            screen
                .lines()
                .position(|line| line.contains(label))
                .unwrap()
        });
        if let Some(expected) = positions {
            assert_eq!(current, expected);
        } else {
            positions = Some(current);
        }
        assert!(!screen.contains("Cycle the format"));
        assert!(!screen.contains("Choose files and folders"));
        let cells = terminal.backend().buffer().content();
        let extension = cells
            .windows(2)
            .position(|pair| {
                pair[0].symbol() == "."
                    && pair[1].symbol()
                        == if format == PackFormat::Mrpack {
                            "m"
                        } else {
                            "z"
                        }
            })
            .unwrap();
        assert_eq!(cells[extension].bg, THEME.as_ref().text_dim());
        assert_eq!(cells[extension].fg, THEME.as_ref().background());
        if format == PackFormat::Mmc {
            assert!(!screen.contains("2.3.4"));
            state.selected = Field::Version;
            state.begin_edit(&ui.app.instance_manager);
            assert!(state.editing.is_none());
            state.selected = Field::Name;
            state.move_selection(true);
            assert_eq!(state.selected, Field::Output);
            state.selected = Field::Format;
        } else {
            assert!(screen.contains("2.3.4"));
        }
        assert_eq!(state.fields[1], "2.3.4");
    }
}

#[test]
fn file_browser_folder_navigation_stops_at_root_and_only_escape_goes_back() {
    let mut ui = UiHarness::new();
    add_instance(&mut ui, "Example");
    ui.key(KeyCode::Char('e'));
    let state = ui.app.export_popup.as_mut().unwrap();
    state.phase = Phase::Files;
    state.browser.directory = PathBuf::from("config/nested");
    ui.key(KeyCode::Char('h'));
    assert_eq!(
        ui.app.export_popup.as_ref().unwrap().browser.directory,
        PathBuf::from("config")
    );
    ui.key(KeyCode::Left);
    ui.app.export_popup.as_mut().unwrap().browser.selected = 0;
    for key in [
        KeyCode::Char('h'),
        KeyCode::Left,
        KeyCode::Char('h'),
        KeyCode::Left,
    ] {
        ui.key(key);
        let state = ui.app.export_popup.as_ref().unwrap();
        assert_eq!(state.phase, Phase::Files);
        assert!(state.browser.directory.as_os_str().is_empty());
        assert_eq!(state.browser.selected, 0);
    }
    ui.key(KeyCode::Esc);
    assert_eq!(ui.app.export_popup.as_ref().unwrap().phase, Phase::Form);
}

#[test]
fn export_edits_only_after_enter_and_escape_discards_the_edit() {
    let mut ui = UiHarness::new();
    add_instance(&mut ui, "Example");
    ui.key(KeyCode::Char('e'));
    ui.key(KeyCode::Down);
    ui.key(KeyCode::Enter);
    ui.key(KeyCode::Char('q'));
    assert!(
        ui.app
            .export_popup
            .as_ref()
            .unwrap()
            .editing
            .as_ref()
            .unwrap()
            .lines()[0]
            .ends_with('q')
    );
    assert!(!ui.app.exit);
    ui.key(KeyCode::Esc);
    assert_eq!(ui.app.export_popup.as_ref().unwrap().fields[0], "Example");
    ui.key(KeyCode::Enter);
    ui.key(KeyCode::Char('q'));
    ui.key(KeyCode::Enter);
    assert_eq!(ui.app.export_popup.as_ref().unwrap().fields[0], "Exampleq");
    ui.app.export_popup.as_mut().unwrap().selected = Field::Include;
    for key in [KeyCode::Enter, KeyCode::Right] {
        ui.key(key);
        let state = ui.app.export_popup.as_ref().unwrap();
        assert_eq!(state.phase, Phase::Form);
        assert!(state.pending.is_some());
        assert!(!keybinds(state).iter().any(|(key, _)| *key == "Enter"));
    }
    ui.key(KeyCode::Esc);
    assert!(ui.app.export_popup.is_none());
}

#[test]
fn export_navigation_repeat_and_mouse_wheel_stay_in_the_popup() {
    use crossterm::event::{KeyEventKind, KeyModifiers, MouseEventKind};
    let mut ui = UiHarness::new();
    add_instance(&mut ui, "Example");
    ui.key(KeyCode::Char('e'));
    let selected_instance = ui.app.instances_state.list_state.selected;
    ui.mouse(MouseEventKind::ScrollDown);
    assert_eq!(ui.app.export_popup.as_ref().unwrap().selected, Field::Name);
    assert_eq!(
        ui.app.instances_state.list_state.selected,
        selected_instance
    );
    let mut key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
    key.kind = KeyEventKind::Repeat;
    assert!(ui.key_event(key));
    assert_eq!(
        ui.app.export_popup.as_ref().unwrap().selected,
        Field::Version
    );
    key.code = KeyCode::Enter;
    assert!(!ui.key_event(key));
    assert!(!ui.app.export_popup.as_ref().unwrap().text_input_active());
}

#[test]
fn export_drain_reports_task_failure_without_discarding_edits() {
    let mut ui = UiHarness::new();
    add_instance(&mut ui, "Example");
    let mut state = State::new(ui.app.instances_state.selected_instance().unwrap());
    for _ in 0..2 {
        let (sender, receiver) = oneshot::channel();
        state.pending = Some(receiver);
        drop(sender);
        state.drain();
        assert!(
            crate::feedback::errors::peek_all_errors()[0]
                .message
                .contains("stopped unexpectedly")
        );
        assert_eq!(state.phase, Phase::Form);
    }
    let (sender, receiver) = oneshot::channel();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    state.resolving.insert(
        state.format,
        Resolving {
            receiver,
            task: runtime.spawn(async {}),
        },
    );
    assert!(sender.send(Err("Provider unavailable".to_owned())).is_ok());
    state.drain();
    assert!(
        crate::feedback::errors::peek_all_errors()[0]
            .message
            .contains("Provider unavailable")
    );
    let count = crate::feedback::errors::peek_all_errors().len();
    state.drain();
    assert_eq!(crate::feedback::errors::peek_all_errors().len(), count);
    assert_eq!(state.fields[0], "Example");
}

#[test]
fn empty_export_field_uses_a_toast_and_keeps_the_editor_open() {
    let mut ui = UiHarness::new();
    add_instance(&mut ui, "Example");
    let mut state = State::new(ui.app.instances_state.selected_instance().unwrap());
    state.selected = Field::Name;
    state.editing = Some(settings_text_area(vec![String::new()]));
    state.commit_edit();
    assert!(state.text_input_active());
    assert_eq!(state.fields[0], "Example");
    assert_eq!(
        crate::feedback::errors::peek_all_errors()[0].message,
        "Pack name must not be empty"
    );
    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 28)).unwrap();
    terminal.draw(|frame| render(frame, &mut state)).unwrap();
    assert!(!terminal.backend().to_string().contains("must not be empty"));
}

#[test]
fn export_popup_does_not_cover_stacked_toasts() {
    let mut ui = UiHarness::new();
    add_instance(&mut ui, "Example");
    let mut state = State::new(ui.app.instances_state.selected_instance().unwrap());
    state.phase = Phase::Files;
    ui.app.export_popup = Some(state);
    for message in [
        "Export toast stays above the file browser",
        "Another export error",
    ] {
        crate::feedback::errors::push_message(tracing::Level::ERROR, message);
    }
    for event in crate::feedback::errors::ERROR_EVENTS
        .lock()
        .unwrap()
        .iter_mut()
    {
        event.pushed_at -= std::time::Duration::from_millis(500);
        ui.app
            .error_effects
            .insert(event.id, crate::tui::app::ErrorEffectState::Idle);
    }
    ui.draw();
    let screen = ui.screen();
    assert!(
        screen.contains("Export toast stays above the file browser"),
        "{screen}"
    );
    assert!(screen.contains("Another export error"));
}

#[test]
fn export_closes_after_confirmation_and_only_prompts_again_for_overwrites() {
    let mut ui = UiHarness::new();
    add_instance(&mut ui, "Example");
    let instance = ui.app.instances_state.selected_instance().unwrap().clone();
    let root = ui.instance_path("Example").join("minecraft");
    std::fs::create_dir_all(root.join("config")).unwrap();
    std::fs::write(root.join("config/settings.txt"), b"settings").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        for scenario in ["new", "overwrite", "race"] {
            let output = ui.instance_path("Example").join(format!("{scenario}.zip"));
            if scenario == "overwrite" {
                std::fs::write(&output, b"keep until approved").unwrap();
            }
            let mut state = State::new(&instance);
            state.format = PackFormat::Mmc;
            state.fields[3] = output.display().to_string();
            state.plan = Some(
                export::prepare_instance_export(
                    &ui.app.instance_manager,
                    "Example",
                    &state.options(),
                )
                .await
                .unwrap(),
            );
            state.phase = Phase::Confirm;
            ui.app.export_popup = Some(state);
            let enter = KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE);
            ui.app.handle_key_event(enter).unwrap();
            if scenario == "overwrite" {
                assert_eq!(
                    ui.app.export_popup.as_ref().unwrap().phase,
                    Phase::Overwrite
                );
                assert_eq!(std::fs::read(&output).unwrap(), b"keep until approved");
                let mut terminal =
                    ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).unwrap();
                terminal
                    .draw(|frame| render(frame, ui.app.export_popup.as_mut().unwrap()))
                    .unwrap();
                assert!(terminal.backend().to_string().contains("Overwrite file?"));
                assert!(
                    terminal
                        .backend()
                        .to_string()
                        .contains("This will overwrite the existing file")
                );
                assert!(!terminal.backend().to_string().contains("existing file."));
                let warning = "This will overwrite the existing file";
                let cells = terminal.backend().buffer().content();
                let start = cells
                    .windows(warning.len())
                    .position(|cells| {
                        cells.iter().map(|cell| cell.symbol()).collect::<String>() == warning
                    })
                    .unwrap();
                assert!(
                    cells[start..start + warning.len()]
                        .iter()
                        .all(|cell| cell.fg == THEME.as_ref().warning())
                );
                assert!(!terminal.backend().to_string().contains("Runtime"));
                ui.app
                    .handle_key_event(KeyEvent::new(
                        KeyCode::Esc,
                        crossterm::event::KeyModifiers::NONE,
                    ))
                    .unwrap();
                assert_eq!(ui.app.export_popup.as_ref().unwrap().phase, Phase::Confirm);
                assert_eq!(std::fs::read(&output).unwrap(), b"keep until approved");
                ui.app.handle_key_event(enter).unwrap();
                ui.app.handle_key_event(enter).unwrap();
            }
            assert!(ui.app.export_popup.is_none());
            assert!(crate::feedback::progress::is_active());
            ui.draw();
            assert!(!ui.screen().contains("Export Complete"));
            if scenario == "race" {
                std::fs::write(&output, b"created after confirmation").unwrap();
            }
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                loop {
                    let finished = if scenario == "race" {
                        crate::feedback::errors::peek_all_errors()
                            .iter()
                            .any(|event| event.message.contains("Output already exists"))
                    } else {
                        output.exists()
                    };
                    if finished
                        && crate::instance::runtime::lock_instance(
                            &ui.app.instance_manager.instances_dir,
                            "Example",
                        )
                        .is_ok()
                    {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert!(ui.app.export_popup.is_none());
            if scenario == "race" {
                assert_eq!(
                    std::fs::read(&output).unwrap(),
                    b"created after confirmation"
                );
                assert!(
                    crate::feedback::errors::peek_all_errors()[0]
                        .message
                        .contains("Output already exists")
                );
            } else {
                assert_eq!(
                    crate::instance::import::build_summary(&output)
                        .unwrap()
                        .format,
                    PackFormat::Mmc
                );
                assert!(
                    crate::feedback::errors::peek_all_errors().is_empty(),
                    "No completion toast should be shown"
                );
            }
        }
    });
}

#[test]
fn file_browser_selects_defaults_and_exports_only_chosen_files_in_each_format() {
    use crossterm::event::KeyModifiers;
    let _guard = crate::tests::TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        for format in PackFormat::ALL {
            let temp = tempfile::tempdir().unwrap();
            let manager = InstanceManager::new(temp.path().join("instances"), temp.path().join("meta"));
            let instance: InstanceConfig = serde_json::from_value(serde_json::json!({"name":"Example","game_version":"1.21.1","loader":"vanilla","created":"2026-01-01T00:00:00Z"})).unwrap();
            let root = crate::storage::InstancePaths::new(manager.instances_dir.join(&instance.name)).minecraft();
            for (path, data) in [("config/keep.txt", "keep"), ("config/omit.txt", "omit"), ("mods/custom.txt", "custom"), ("options.txt", "options"), ("saves/World/level.dat", "world")] {
                std::fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
                std::fs::write(root.join(path), data).unwrap();
            }
            manager.save(&instance).unwrap();
            let mut state = State::new(&instance);
            state.fields[2] = "Example Author".to_owned();
            state.format = format;
            let output = temp.path().join(format!("pack.{}", format.extension()));
            state.fields[3] = output.display().to_string();
            state.start_scan(&manager);
            state.selected = Field::Include;
            state.begin_edit(&manager);
            assert_eq!(state.phase, Phase::Form);
            state.task.take().unwrap().await.unwrap();
            state.drain();
            if let Some(pending) = state.resolving.remove(&format) {
                pending.task.await.unwrap();
                let references = pending.receiver.await.unwrap().unwrap();
                let plan = state.scan.as_ref().unwrap().plan(&state.options(), &state.scan.as_ref().unwrap().paths().cloned().collect(), &references).unwrap();
                state.resolved.insert(format, Ok(Resolved { references, paths: plan.referenced_paths().clone() }));
            }
            assert!(state.selected_files.contains(std::path::Path::new("config/keep.txt")));
            assert!(!state.selected_files.contains(std::path::Path::new("options.txt")));
            assert!(!state.selected_files.contains(std::path::Path::new("saves/World/level.dat")));
            state.begin_edit(&manager);
            assert_eq!(state.phase, Phase::Files);
            state.fields[3] = "exports/example.zip".to_owned();
            if format == PackFormat::CurseForge {
                let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30)).unwrap();
                terminal.draw(|frame| render(frame, &mut state)).unwrap();
                insta::assert_snapshot!("export_file_browser", terminal.backend());
                let mut compact = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
                compact.draw(|frame| render(frame, &mut state)).unwrap();
                insta::assert_snapshot!("export_file_browser_compact", compact.backend());
            }
            state.fields[3] = output.display().to_string();
            state.browser.selected = state.file_entries().iter().position(|entry| entry.path == std::path::Path::new("config")).unwrap();
            state.enter_directory();
            assert_eq!(state.browser.directory, PathBuf::from("config"));
            state.browser.selected = state.file_entries().iter().position(|entry| entry.path == std::path::Path::new("config/omit.txt")).unwrap();
            state.toggle_file();
            assert!(!state.selected_files.contains(std::path::Path::new("config/omit.txt")));
            state.handle_key(&KeyEvent::new(KeyCode::Left, KeyModifiers::NONE), &manager);
            assert!(state.browser.directory.as_os_str().is_empty());
            state.browser.selected = state.file_entries().iter().position(|entry| entry.path == std::path::Path::new("config")).unwrap();
            assert!(matches!(&state.file_entries()[state.browser.selected], files::Entry { total: 2, included: 1, .. }));
            state.toggle_file();
            assert!(state.selected_files.contains(std::path::Path::new("config/omit.txt")));
            state.toggle_file();
            assert!(!state.selected_files.contains(std::path::Path::new("config/keep.txt")));
            state.toggle_file();
            state.enter_directory();
            state.browser.selected = state.file_entries().iter().position(|entry| entry.path == std::path::Path::new("config/omit.txt")).unwrap();
            state.toggle_file();
            state.handle_key(&KeyEvent::new(KeyCode::Left, KeyModifiers::NONE), &manager);
            state.browser.selected = state.file_entries().iter().position(|entry| entry.path == std::path::Path::new("options.txt")).unwrap();
            state.toggle_file();
            assert!(state.selected_files.contains(std::path::Path::new("options.txt")));
            if format != PackFormat::Mmc {
                state.confirm(&manager);
                assert_eq!(state.phase, Phase::Files);
                assert!(crate::feedback::errors::peek_all_errors()[0].message.contains("Approve copying"));
                assert!(!file_summary(&state, false).iter().any(|line| line.to_string().contains("Approve copying")));
                state.handle_key(&KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE), &manager);
                assert!(!state.bundle_unmatched);
                for modifiers in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
                    state.handle_key(&KeyEvent::new(KeyCode::Char('c'), modifiers), &manager);
                    assert!(!state.bundle_unmatched);
                }
                state.handle_key(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE), &manager);
                assert!(state.bundle_unmatched);
                state.browser.selected = state.file_entries().iter().position(|entry| entry.path == std::path::Path::new("mods")).unwrap();
                state.toggle_file();
                assert!(state.bundle_unmatched, "Removing unmatched files must preserve approval");
                assert_eq!(state.file_entries()[state.browser.selected].path, std::path::Path::new("mods"));
                state.toggle_file();
                assert!(!state.bundle_unmatched, "Adding unmatched files must require approval again");
                assert_eq!(state.file_entries()[state.browser.selected].path, std::path::Path::new("mods"));
                state.handle_key(&KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE), &manager);
            }
            for (width, height) in [(40, 12), (20, 8)] {
                let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
                terminal.draw(|frame| render(frame, &mut state)).unwrap();
            }
            state.confirm(&manager);
            assert_eq!(state.phase, Phase::Confirm, "{:?}", crate::feedback::errors::peek_all_errors());
            assert!(!output.exists());
            if format == PackFormat::CurseForge {
                state.fields[3] = "exports/example.zip".to_owned();
                let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(90, 26)).unwrap();
                terminal.draw(|frame| render(frame, &mut state)).unwrap();
                insta::assert_snapshot!("export_confirmation", terminal.backend());
                let cells = terminal.backend().buffer().content();
                let name = cells.windows(9).position(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>() == "Pack name").unwrap();
                assert_eq!(cells[name].fg, THEME.as_ref().text_dim());
                assert_eq!(cells[name + 18].fg, THEME.as_ref().text());
                let extension = cells.windows(4).position(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>() == ".zip").unwrap();
                assert_eq!(cells[extension].bg, THEME.as_ref().text_dim());
                assert_eq!(cells[extension].fg, THEME.as_ref().background());
                for title in ["Pack", "Runtime", "Files"] { assert!(state.confirmation_lines().iter().any(|line| line.to_string() == format!("  {title}"))); }
                assert!(!terminal.backend().to_string().contains("Links download on import"));
                let long_output = format!("exports/{}TAIL_END.zip", "long-directory/".repeat(12));
                state.fields[3] = long_output.clone();
                let mut wrapped = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 32)).unwrap();
                wrapped.draw(|frame| render(frame, &mut state)).unwrap();
                let visible = wrapped.backend().buffer().content().iter().map(|cell| cell.symbol()).collect::<String>()
                    .chars().filter(|ch| !ch.is_whitespace() && !matches!(ch, '│' | '─' | '╭' | '╮' | '╰' | '╯')).collect::<String>();
                assert!(visible.contains(&long_output), "Long output paths must wrap without losing text");
                state.fields[3] = output.display().to_string();
            }
            state.handle_key(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE), &manager);
            assert_eq!(state.phase, Phase::Files);
            state.confirm(&manager);
            state.write(false).unwrap().await.unwrap();
            let mut archive = zip::ZipArchive::new(std::fs::File::open(output).unwrap()).unwrap();
            let prefix = match format { PackFormat::CurseForge => "overrides", PackFormat::Mrpack => "client-overrides", PackFormat::Mmc => ".minecraft" };
            assert!(archive.by_name(&format!("{prefix}/config/keep.txt")).is_ok());
            assert!(archive.by_name(&format!("{prefix}/options.txt")).is_ok());
            assert!(archive.by_name(&format!("{prefix}/config/omit.txt")).is_err());
            assert!(archive.by_name(&format!("{prefix}/saves/World/level.dat")).is_err());
            drop(state);
            assert!(crate::instance::runtime::lock_instance(&manager.instances_dir, &instance.name).is_ok());
        }
    });
}

#[test]
fn file_summary_is_minimal_and_unmatched_names_keep_their_colour() {
    let _guard = crate::tests::TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let temp = tempfile::tempdir().unwrap();
        let manager = InstanceManager::new(temp.path().join("instances"), temp.path().join("meta"));
        let instance: InstanceConfig = serde_json::from_value(serde_json::json!({"name":"Example","game_version":"1.21.1","loader":"vanilla","created":"2026-01-01T00:00:00Z"})).unwrap();
        let root = crate::storage::InstancePaths::new(manager.instances_dir.join(&instance.name)).minecraft();
        for path in ["config/settings.txt", "mods/matched.jar", "mods/custom.jar", "mods/nested/custom.txt", "shaderpacks/custom.zip"] {
            std::fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
            std::fs::write(root.join(path), path.as_bytes()).unwrap();
        }
        manager.save(&instance).unwrap();
        let scan = Arc::new(export::scan_instance_export(&manager, &instance.name).await.unwrap());
        let mut state = State::new(&instance);
        state.selected_files = scan.defaults.clone();
        state.scan = Some(scan);
        state.phase = Phase::Files;

        let pending = file_summary(&state, false).into_iter().map(|line| line.to_string()).collect::<Vec<_>>().join("\n");
        assert!(pending.contains("Identifying CurseForge"));
        assert!(pending.lines().any(|line| line.contains("Download links") && line.ends_with('…')));
        assert!(state.unmatched_files(&state.selected_files).is_empty());
        assert!(!pending.contains("Copy 5 unmatched"));
        state.selected_files = BTreeSet::from([PathBuf::from("mods/nested/custom.txt")]);
        state.bundle_unmatched = true;
        state.browser.directory = PathBuf::from("mods");
        state.browser.selected = state.file_entries().iter().position(|entry| entry.path == std::path::Path::new("mods/custom.jar")).unwrap();
        state.toggle_file();
        assert!(!state.bundle_unmatched, "New files awaiting identification must require fresh approval");
        state.selected_files = state.scan.as_ref().unwrap().defaults.clone();
        state.browser.directory = PathBuf::new();
        state.browser.selected = 0;
        state.resolved.insert(PackFormat::CurseForge, Err("Provider unavailable".to_owned()));
        assert!(state.unmatched_files(&state.selected_files).is_empty());
        assert!(!file_summary(&state, false).iter().any(|line| line.to_string().contains("Provider unavailable")));

        for (format, copies, unmatched) in [(PackFormat::CurseForge, 2, 2), (PackFormat::Mrpack, 1, 3), (PackFormat::Mmc, 5, 0)] {
            state.format = format;
            let downloads = usize::from(format != PackFormat::Mmc);
            state.resolved.insert(format, Ok(Resolved {
                references: ExportReferences::empty(format),
                paths: if downloads > 0 { BTreeSet::from([PathBuf::from("mods/matched.jar")]) } else { BTreeSet::new() },
            }));
            let lines = file_summary(&state, false).into_iter().map(|line| line.to_string()).collect::<Vec<_>>();
            let summary = lines.join("\n");
            assert_eq!(downloads + copies + unmatched, state.selected_files.len());
            assert!(!summary.contains("Bundled"));
            for (label, count) in if format == PackFormat::Mmc {
                vec![("Selected", 5), ("File copies", copies)]
            } else {
                vec![("Selected", 5), ("Download links", downloads), ("Other copies", copies), ("Unmatched", unmatched)]
            } {
                assert!(lines.iter().any(|line| line.trim_start().starts_with(label) && line.split_whitespace().last() == Some(count.to_string().as_str())), "{summary}");
            }
            if format == PackFormat::Mmc {
                assert!(!summary.contains("No MultiMC"));
                assert!(!summary.contains("Download links"));
            } else {
                assert!(summary.contains(&format!("Copy {unmatched} unmatched")));
                assert!(!summary.contains("download match:"));
                assert!(!summary.contains("custom.jar"));
                assert!(!summary.contains("settings.txt"));
                assert!(!summary.contains("config/"));
                assert!(!summary.contains("shaderpacks/"));
                assert!(!summary.contains("mods/"));
                assert!(!summary.contains("Links download"));
                assert!(!summary.contains("Copies go"));
                assert!(summary.contains("  Check licences"));
                assert!(!summary.contains("before sharing"));
                assert!(lines.len() <= 10, "{summary}");
                state.bundle_unmatched = true;
                let approved = file_summary(&state, false).into_iter().map(|line| line.to_string()).collect::<Vec<_>>().join("\n");
                assert!(approved.contains(&format!("[✓] Copy {unmatched} unmatched")));
                state.bundle_unmatched = false;
            }
            if format == PackFormat::CurseForge {
                for (directory, selected) in [("", "mods"), ("mods", "mods/nested"), ("mods", "mods/custom.jar"), ("mods/nested", "mods/nested/custom.txt")] {
                    state.browser.directory = PathBuf::from(directory);
                    let badge_row = state.file_entries().iter().position(|entry| entry.path == std::path::Path::new(selected)).unwrap();
                    for included in [true, false] {
                    if !included { state.browser.selected = badge_row; state.toggle_file(); }
                    for selection in [if badge_row == 0 { 1 } else { 0 }, badge_row] {
                        state.browser.selected = selection;
                        let area = Rect::new(0, 0, 70, 6);
                        let mut buffer = Buffer::empty(area);
                        buffer.set_style(area, Style::default().bg(THEME.as_ref().surface()));
                        files::render(&state, area, &mut buffer);
                        let text = buffer.content().iter().map(|cell| cell.symbol()).collect::<String>();
                        assert!(!text.contains("Download"));
                        assert!(!text.contains("Unmatched"));
                        let row = &buffer.content()[badge_row * 70..(badge_row + 1) * 70];
                        let entry = &state.file_entries()[badge_row];
                        let name = format!("{}{}", entry.path.file_name().unwrap().to_string_lossy(), if entry.directory { "/" } else { "" });
                        let start = row.windows(name.len()).position(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>() == name).unwrap();
                        for cell in &row[start..start + name.len()] {
                            assert_eq!(cell.fg, THEME.as_ref().warning());
                            assert_eq!(cell.bg, if selection == badge_row { THEME.as_ref().stripe() } else { THEME.as_ref().surface() });
                        }
                    }
                    if !included {
                        assert!(state.unmatched_files(&state.selected_files).len() < unmatched);
                        state.browser.selected = badge_row;
                        state.toggle_file();
                    }
                    }
                }
                state.browser.directory = PathBuf::new();
                state.browser.selected = 0;
                let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(130, 34)).unwrap();
                terminal.draw(|frame| render(frame, &mut state)).unwrap();
                insta::assert_snapshot!("export_file_breakdown", terminal.backend());
                state.browser.directory = PathBuf::from("mods");
                state.browser.selected = state.file_entries().iter().position(|entry| entry.path == std::path::Path::new("mods/custom.jar")).unwrap();
                terminal.draw(|frame| render(frame, &mut state)).unwrap();
                insta::assert_snapshot!("export_mods_unmatched", terminal.backend());
                state.browser.directory = PathBuf::new();
                state.browser.selected = 0;
            }
        }
        state.format = PackFormat::Mrpack;
        for count in [1648, 10000] {
            state.selected_files = (0..count).map(|index| PathBuf::from(format!("mods/unmatched-{index}.jar"))).collect();
            for (width, height) in [(100, 28), (130, 34), (80, 24)] {
                let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
                terminal.draw(|frame| render(frame, &mut state)).unwrap();
                let rows = terminal.backend().buffer().content().chunks(width as usize).collect::<Vec<_>>();
                let copy = format!("[ ] Copy {count} unmatched");
                let (index, row) = rows.iter().enumerate().find(|(_, row)| row.iter().map(|cell| cell.symbol()).collect::<String>().contains(&copy)).unwrap();
                let start = row.windows(copy.len()).position(|cells| cells.iter().map(|cell| cell.symbol()).collect::<String>() == copy).unwrap();
                assert!(row[start + copy.len()..start + copy.len() + 2].iter().all(|cell| cell.symbol() == " "), "Copy count needs right padding");
                assert!(rows[index + 1].iter().map(|cell| cell.symbol()).collect::<String>().contains("Check licences"), "Copy count must not wrap");
            }
        }
    });
}

#[tokio::test]
async fn unmatched_highlighting_distinguishes_neighbouring_paths() {
    let temp = tempfile::tempdir().unwrap();
    let manager = InstanceManager::new(temp.path().join("instances"), temp.path().join("meta"));
    let instance: InstanceConfig = serde_json::from_value(serde_json::json!({"name":"Example","game_version":"1.21.1","loader":"vanilla","created":"2026-01-01T00:00:00Z"})).unwrap();
    let root =
        crate::storage::InstancePaths::new(manager.instances_dir.join(&instance.name)).minecraft();
    let matched = BTreeSet::from([
        PathBuf::from("mods/a/matched.jar"),
        PathBuf::from("mods/a.jar"),
        PathBuf::from("mods/z.jar"),
    ]);
    let unmatched = BTreeSet::from([
        PathBuf::from("mods/a-extra/custom.jar"),
        PathBuf::from("mods/a.jar-extra/custom.jar"),
        PathBuf::from("mods/custom.jar"),
    ]);
    for path in matched.iter().chain(&unmatched) {
        std::fs::create_dir_all(root.join(path).parent().unwrap()).unwrap();
        std::fs::write(root.join(path), b"mod").unwrap();
    }
    manager.save(&instance).unwrap();
    let mut state = State::new(&instance);
    state.scan = Some(Arc::new(
        export::scan_instance_export(&manager, &instance.name)
            .await
            .unwrap(),
    ));
    state.resolved.insert(
        PackFormat::CurseForge,
        Ok(Resolved {
            references: ExportReferences::empty(PackFormat::CurseForge),
            paths: matched,
        }),
    );
    for format in [PackFormat::CurseForge, PackFormat::Mmc] {
        state.format = format;
        for directory in ["", "mods", "mods/a", "mods/a-extra", "mods/a.jar-extra"] {
            state.browser.directory = PathBuf::from(directory);
            let area = Rect::new(0, 0, 80, 10);
            let mut buffer = Buffer::empty(area);
            files::render(&state, area, &mut buffer);
            for (index, entry) in state.file_entries().iter().enumerate() {
                let expected = if format != PackFormat::Mmc
                    && unmatched.iter().any(|file| file.starts_with(&entry.path))
                {
                    THEME.as_ref().warning()
                } else if entry.directory {
                    THEME.as_ref().text()
                } else {
                    THEME.as_ref().text_dim()
                };
                assert_eq!(
                    buffer[(6, index as u16)].fg,
                    expected,
                    "{format:?}: {}",
                    entry.path.display()
                );
            }
        }
    }
}
