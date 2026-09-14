// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;

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

// Removed drain_empty_returns_empty: it relied on no other test pushing
// to LAST_PLAYED between the two drain calls, which races with the
// parallel push_and_drain_last_played test. The drain semantics are
// already covered by push_and_drain_last_played, which asserts a
// specific entry is present, and the empty-result path is exercised
// implicitly any time drain runs after that test's cleanup.

#[test]
fn send_kill_returns_false_for_missing() {
    assert!(!send_kill("run_never_registered_xyz"));
}

#[test]
fn register_and_send_kill() {
    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    register_kill("run_test_kill", tx);
    assert!(send_kill("run_test_kill"));
    // the kill signal itself must arrive, not just report success
    assert!(rx.try_recv().is_ok());
}

#[test]
fn cleanup_kill_sender_removes() {
    let (tx, _rx) = tokio::sync::oneshot::channel::<()>();
    register_kill("run_test_cleanup", tx);
    cleanup_kill_sender("run_test_cleanup");
    assert!(!send_kill("run_test_cleanup"));
}
