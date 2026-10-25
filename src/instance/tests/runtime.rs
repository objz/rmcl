// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

#[test]
fn dropping_an_instance_lock_releases_it_even_with_a_duplicated_descriptor() {
    let temp = tempfile::tempdir().unwrap();
    let lock = lock_instance(temp.path(), "duplicated").unwrap();
    let duplicate = lock.file.try_clone().unwrap();
    drop(lock);
    let next = lock_instance(temp.path(), "duplicated");
    drop(duplicate);
    assert!(
        next.is_ok(),
        "A duplicate descriptor retained the released instance lock"
    );
}

#[test]
fn running_content_changes_remain_serialized() {
    let temp = tempfile::tempdir().unwrap();
    let mut game = lock_instance(temp.path(), "game").unwrap();
    assert!(lock_content(temp.path(), "game").is_err());
    game.allow_content_changes().unwrap();
    let content = lock_content(temp.path(), "game").unwrap();
    assert!(lock_content(temp.path(), "game").is_err());
    assert!(lock_instance(temp.path(), "game").is_err());
    drop(content);
    assert!(lock_content(temp.path(), "game").is_ok());
    assert!(lock_instance(temp.path(), "game").is_err());
    drop(game);
    assert!(lock_instance(temp.path(), "game").is_ok());
}

#[test]
fn content_locks_do_not_collide_with_other_instance_names() {
    let temp = tempfile::tempdir().unwrap();
    let _game = lock_instance(temp.path(), "game").unwrap();
    let _other = lock_instance(temp.path(), "game.content").unwrap();
}

#[test]
fn set_and_get_state() {
    set_state("run_test_1", RunState::Starting);
    assert_eq!(get("run_test_1"), Some(RunState::Starting));
}

#[test]
fn get_missing_returns_none() {
    assert_eq!(get("run_never_set_xyz"), None);
}

#[test]
fn remove_clears_state() {
    set_state("run_test_2", RunState::Running);
    remove("run_test_2");
    assert_eq!(get("run_test_2"), None);
}

#[test]
fn set_state_overwrites() {
    set_state("run_test_3", RunState::Starting);
    set_state("run_test_3", RunState::Running);
    assert_eq!(get("run_test_3"), Some(RunState::Running));
}

#[test]
fn all_returns_entries() {
    set_state("run_test_all_a", RunState::Running);
    let entries = all();
    assert!(entries.iter().any(|(k, _)| k == "run_test_all_a"));
}

#[test]
fn crashed_state_stores_exit_code() {
    set_state("run_test_crash", RunState::Crashed(Some(1)));
    assert_eq!(get("run_test_crash"), Some(RunState::Crashed(Some(1))));
}

#[test]
fn crashed_instances_are_not_active() {
    set_state("run_test_inactive_crash", RunState::Crashed(Some(1)));
    assert!(!is_active("run_test_inactive_crash"));
    set_state("run_test_active_start", RunState::Starting);
    assert!(is_active("run_test_active_start"));
}

#[test]
fn push_and_drain_last_played() {
    let time = Utc::now();
    push_last_played("run_test_lp", time);
    let drained = drain_last_played();
    assert!(drained.iter().any(|(k, _)| k == "run_test_lp"));
}

#[test]
fn send_kill_returns_false_for_missing() {
    assert!(!send_kill("run_never_registered_xyz"));
}

#[test]
fn register_and_send_kill() {
    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    register_kill("run_test_kill", tx);
    assert!(send_kill("run_test_kill"));
    assert!(rx.try_recv().is_ok());
}

#[test]
fn cleanup_kill_sender_removes() {
    let (tx, _rx) = tokio::sync::oneshot::channel::<()>();
    register_kill("run_test_cleanup", tx);
    cleanup_kill_sender("run_test_cleanup");
    assert!(!send_kill("run_test_cleanup"));
}
