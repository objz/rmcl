// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

// thread-safe FIFO queue for error/warning toasts displayed in the UI.
// also (ab)used for INFO toasts like "desktop shortcut created" because
// why build a separate notification system when this one works fine.
//
// callers pass id: 0 and push_error assigns a real unique id. the id is
// used by the render layer to track per-toast animation state.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use std::sync::LazyLock;
use tracing::Level;

static NEXT_ERROR_ID: AtomicU64 = AtomicU64::new(1);
static MAX_ERROR_EVENTS: AtomicUsize = AtomicUsize::new(50);

pub(crate) fn set_max_error_events(max_events: usize) {
    let max_events = max_events.max(1);
    MAX_ERROR_EVENTS.store(max_events, Ordering::Relaxed);
    if let Ok(mut events) = ERROR_EVENTS.lock() {
        while events.len() > max_events {
            events.pop_front();
        }
    }
}

#[derive(Debug, Clone)]
pub struct ErrorEvent {
    pub id: u64,
    pub level: Level,
    pub message: String,
    pub pushed_at: Instant,
}

pub static ERROR_EVENTS: LazyLock<Arc<Mutex<VecDeque<ErrorEvent>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(VecDeque::new())));

pub fn push_error(event: ErrorEvent) {
    match ERROR_EVENTS.lock() {
        Ok(mut events) => {
            let mut event = event;
            event.id = NEXT_ERROR_ID.fetch_add(1, Ordering::Relaxed);

            events.push_back(event);

            let max_events = MAX_ERROR_EVENTS.load(Ordering::Relaxed);
            while events.len() > max_events {
                events.pop_front();
            }
            super::request_redraw();
        }
        Err(e) => {
            tracing::error!("Error buffer lock poisoned: {}", e);
        }
    }
}

pub fn push_message(level: Level, message: impl Into<String>) {
    push_error(ErrorEvent {
        id: 0,
        level,
        message: message.into(),
        pushed_at: Instant::now(),
    });
}

#[must_use]
pub fn has_errors() -> bool {
    match ERROR_EVENTS.lock() {
        Ok(events) => !events.is_empty(),
        Err(_) => false,
    }
}

#[must_use]
pub fn pop_error() -> Option<ErrorEvent> {
    match ERROR_EVENTS.lock() {
        Ok(mut events) => {
            let event = events.pop_front();
            if event.is_some() {
                super::request_redraw();
            }
            event
        }
        Err(_) => None,
    }
}

#[must_use]
pub fn peek_error() -> Option<ErrorEvent> {
    match ERROR_EVENTS.lock() {
        Ok(events) => events.front().cloned(),
        Err(_) => None,
    }
}

#[must_use]
// returned in reverse order (newest first) so they stack top-down in the UI
pub fn peek_all_errors() -> Vec<ErrorEvent> {
    match ERROR_EVENTS.lock() {
        Ok(events) => events.iter().rev().cloned().collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
#[path = "tests/errors.rs"]
mod tests;
