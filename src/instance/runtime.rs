// SPDX-FileCopyrightText: 2026 Constantin Bauer
// SPDX-License-Identifier: GPL-3.0-only

use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::LazyLock;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq)]
pub enum RunState {
    Authenticating,
    Starting,
    Running,
    Crashed(Option<i32>),
}

pub static RUNNING: LazyLock<Arc<Mutex<HashMap<String, RunState>>>> =
    LazyLock::new(|| Arc::new(Mutex::new(HashMap::new())));

// The monitor persists timestamps; the TUI drains these to update its cached instances.
type PendingLastPlayed = Arc<Mutex<Vec<(String, DateTime<Utc>)>>>;
pub static PENDING_LAST_PLAYED: LazyLock<PendingLastPlayed> =
    LazyLock::new(|| Arc::new(Mutex::new(Vec::new())));

type KillSenders = Arc<Mutex<HashMap<String, tokio::sync::oneshot::Sender<()>>>>;
pub static KILL_SENDERS: LazyLock<KillSenders> =
    LazyLock::new(|| Arc::new(Mutex::new(HashMap::new())));

pub fn set_state(name: &str, state: RunState) {
    if let Ok(mut map) = RUNNING.lock() {
        map.insert(name.to_string(), state);
        crate::feedback::request_redraw();
    }
}

pub fn remove(name: &str) {
    if let Ok(mut map) = RUNNING.lock() {
        map.remove(name);
        crate::feedback::request_redraw();
    }
}

#[must_use]
pub fn get(name: &str) -> Option<RunState> {
    RUNNING.lock().ok().and_then(|map| map.get(name).cloned())
}

#[must_use]
pub fn is_active(name: &str) -> bool {
    matches!(
        get(name),
        Some(RunState::Authenticating | RunState::Starting | RunState::Running)
    )
}

#[must_use]
pub fn all() -> Vec<(String, RunState)> {
    RUNNING
        .lock()
        .ok()
        .map(|map| map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

#[must_use]
pub fn has_active() -> bool {
    RUNNING.lock().is_ok_and(|map| {
        map.values().any(|state| {
            matches!(
                state,
                RunState::Authenticating | RunState::Starting | RunState::Running
            )
        })
    })
}

pub fn push_last_played(name: &str, time: DateTime<Utc>) {
    if let Ok(mut q) = PENDING_LAST_PLAYED.lock() {
        q.push((name.to_string(), time));
        crate::feedback::request_redraw();
    }
}

pub fn drain_last_played() -> Vec<(String, DateTime<Utc>)> {
    PENDING_LAST_PLAYED
        .lock()
        .ok()
        .map(|mut q| q.drain(..).collect())
        .unwrap_or_default()
}

pub fn register_kill(name: &str, tx: tokio::sync::oneshot::Sender<()>) {
    if let Ok(mut map) = KILL_SENDERS.lock() {
        map.insert(name.to_string(), tx);
    }
}

pub fn send_kill(name: &str) -> bool {
    if let Ok(mut map) = KILL_SENDERS.lock()
        && let Some(tx) = map.remove(name)
    {
        let _ = tx.send(());
        return true;
    }
    false
}

pub fn cleanup_kill_sender(name: &str) {
    if let Ok(mut map) = KILL_SENDERS.lock() {
        map.remove(name);
    }
}

pub(crate) struct InstanceLock {
    file: std::fs::File,
    content: Option<std::fs::File>,
}

impl InstanceLock {
    pub(crate) fn allow_content_changes(&mut self) -> std::io::Result<()> {
        if let Some(file) = &self.content {
            file.unlock()?;
        }
        self.content = None;
        Ok(())
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // A concurrent fork can retain a descriptor until exec; closing our copy
        // alone would leave the instance locked during that interval.
        for file in self.content.iter().chain(std::iter::once(&self.file)) {
            if let Err(error) = file.unlock() {
                tracing::warn!("Could not release instance lock: {error}");
            }
        }
    }
}

pub(crate) fn lock_instance(
    root: &std::path::Path,
    name: &str,
) -> Result<InstanceLock, crate::instance::manager::InstanceError> {
    let mut guard = InstanceLock {
        file: lock_file(root, name, false)?,
        content: None,
    };
    guard.content = Some(lock_file(root, name, true)?);
    Ok(guard)
}

pub(crate) fn lock_content(
    root: &std::path::Path,
    name: &str,
) -> Result<InstanceLock, crate::instance::manager::InstanceError> {
    Ok(InstanceLock {
        file: lock_file(root, name, true)?,
        content: None,
    })
}

fn lock_file(
    root: &std::path::Path,
    name: &str,
    content: bool,
) -> Result<std::fs::File, crate::instance::manager::InstanceError> {
    crate::instance::manager::validate_name(name)?;
    let locks = root.join(if content {
        ".rmcl-locks/content"
    } else {
        ".rmcl-locks"
    });
    std::fs::create_dir_all(&locks)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(locks.join(format!("{name}.lock")))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(
            crate::instance::manager::InstanceError::InstanceRunning(name.to_owned()),
        ),
        Err(std::fs::TryLockError::Error(error)) => Err(error.into()),
    }
}

#[cfg(test)]
#[path = "tests/runtime.rs"]
mod tests;
