// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn format_relative_time_none_returns_never_played() {
    assert_eq!(format_relative_time(None), "Never played");
}

// each #[case] picks a "seconds ago" value that lands in exactly one
// bucket of the match. mutating any bucket boundary (e.g. 3600 to 3601,
// or "minutes" to "seconds") makes one of these cases fail.
#[rstest::rstest]
#[case::just_now(0, "Just now")]
#[case::just_now_upper(59, "Just now")]
#[case::minutes(60, "1 minute ago")]
#[case::minutes_upper(3599, "59 minutes ago")]
#[case::hours(3600, "1 hour ago")]
#[case::hours_upper(86_399, "23 hours ago")]
#[case::days(86_400, "1 day ago")]
#[case::days_upper(2_591_999, "29 days ago")]
#[case::months(2_592_000, "1 month ago")]
#[case::months_upper(31_535_999, "12 months ago")]
#[case::over_a_year(31_536_000, "Over a year ago")]
fn format_relative_time_buckets(#[case] seconds_ago: i64, #[case] expected: &str) {
    let dt = chrono::Utc::now() - chrono::Duration::seconds(seconds_ago);
    assert_eq!(format_relative_time(Some(dt)), expected);
}

use crate::instance::models::{InstanceConfig, ModLoader};
use ratatui::Terminal;
use ratatui::backend::TestBackend;

fn synthetic_instance(name: &str) -> InstanceConfig {
    // last_played intentionally None so the rendered text is the
    // deterministic "Never played" string. anything else would make the
    // snapshot drift relative to chrono::Utc::now().
    InstanceConfig {
        name: name.to_string(),
        game_version: "1.20.1".to_string(),
        loader: ModLoader::Vanilla,
        loader_version: None,
        created: chrono::Utc::now(),
        last_played: None,
        java_path: None,
        memory_max: None,
        memory_min: None,
        jvm_args: vec![],
        environment: Default::default(),
        window_mode: Default::default(),
        inherit_window_mode: false,
        resolution: None,
        inherit_resolution: false,
        preferred_account: None,
        pre_launch_command: Default::default(),
        post_exit_command: Default::default(),
        glfw_path: None,
        config_sync_profile: None,
        modpack_source: None,
    }
}

#[test]
fn instances_list_renders_three_instances() {
    let mut state = State::with_instances(vec![
        synthetic_instance("Vanilla 1.20.1"),
        synthetic_instance("Forge Pack"),
        synthetic_instance("Fabric Test"),
    ]);

    let backend = TestBackend::new(40, 12);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render(f, f.area(), FocusedArea::Instances, &mut state))
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}

#[test]
fn instances_list_renders_empty() {
    let mut state = State::with_instances(vec![]);

    let backend = TestBackend::new(40, 8);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| render(f, f.area(), FocusedArea::Instances, &mut state))
        .unwrap();

    insta::assert_snapshot!(terminal.backend());
}
