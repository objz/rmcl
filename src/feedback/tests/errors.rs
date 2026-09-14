// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use super::*;
use crate::tests::TEST_LOCK;

fn clear_errors_for_test() {
    ERROR_EVENTS.lock().unwrap().clear();
}

fn make_event(msg: &str) -> ErrorEvent {
    ErrorEvent {
        id: 0,
        level: Level::ERROR,
        message: msg.to_string(),
        pushed_at: Instant::now(),
    }
}

#[test]
fn peek_does_not_remove() {
    let _guard = TEST_LOCK.lock().unwrap();
    clear_errors_for_test();

    push_error(make_event("peek-test"));

    let count_before = peek_all_errors().len();
    let peeked = peek_error();
    let count_after = peek_all_errors().len();

    assert_eq!(
        count_before, count_after,
        "peek should not change queue length"
    );

    assert!(peeked.is_some());
    assert_eq!(peeked.unwrap().message, "peek-test");
}

#[test]
fn peek_all_returns_newest_first() {
    let _guard = TEST_LOCK.lock().unwrap();
    clear_errors_for_test();

    push_error(make_event("newest_a"));
    push_error(make_event("newest_b"));

    let all = peek_all_errors();

    assert_eq!(all.len(), 2);
    assert!(all[0].message.ends_with("_b"));
    assert!(all[1].message.ends_with("_a"));
    assert!(all[0].id > all[1].id);
}

#[test]
fn auto_assigned_ids_are_unique() {
    let _guard = TEST_LOCK.lock().unwrap();
    clear_errors_for_test();

    push_error(make_event("unique_1"));
    push_error(make_event("unique_2"));

    let all = peek_all_errors();

    assert_eq!(all.len(), 2);
    assert_ne!(all[0].id, all[1].id);
}

#[test]
fn overflow_drops_oldest() {
    let _guard = TEST_LOCK.lock().unwrap();
    clear_errors_for_test();

    let max_error_events = 50;
    set_max_error_events(max_error_events);
    for i in 0..(max_error_events + 10) {
        push_error(make_event(&format!("overflow_{i}")));
    }

    let all = peek_all_errors();

    assert_eq!(all.len(), max_error_events);

    assert!(!all.iter().any(|e| e.message == "overflow_0"));
    assert!(!all.iter().any(|e| e.message == "overflow_9"));

    assert!(all.iter().any(|e| e.message == "overflow_10"));
    assert!(all.iter().any(|e| e.message == "overflow_59"));
}

#[test]
fn lowering_limit_trims_existing_events_immediately() {
    let _guard = TEST_LOCK.lock().unwrap();
    clear_errors_for_test();
    set_max_error_events(5);
    for i in 0..5 {
        push_error(make_event(&format!("trim_{i}")));
    }

    set_max_error_events(2);
    let all = peek_all_errors();

    assert_eq!(all.len(), 2);
    assert_eq!(all[0].message, "trim_4");
    assert_eq!(all[1].message, "trim_3");
    set_max_error_events(50);
}
