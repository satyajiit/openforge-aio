//! Background process watcher.
//!
//! Polls `ProcessSnapshot` periodically, computes the running set of registered
//! games, and emits a `process_state` event on change.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use openforge_core::ProcessSnapshot;
use openforge_runtime::REGISTRY;
use parking_lot::RwLock;
use tauri::async_runtime::{self, JoinHandle};
use tauri::{AppHandle, Emitter};

use crate::types::ProcessStateEvent;

pub struct Watcher {
    running: Arc<RwLock<HashSet<String>>>,
    cancel: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Default for Watcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Watcher {
    pub fn new() -> Self {
        Self {
            running: Arc::new(RwLock::new(HashSet::new())),
            cancel: Arc::new(AtomicBool::new(false)),
            paused: Arc::new(AtomicBool::new(false)),
            handle: None,
        }
    }

    pub fn snapshot(&self) -> Vec<String> {
        let mut v: Vec<String> = self.running.read().iter().cloned().collect();
        v.sort();
        v
    }

    pub fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
    }
    pub fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
    }

    pub fn start(&mut self, app: AppHandle, focused: Arc<AtomicBool>) {
        if self.handle.is_some() {
            return;
        }
        let running = Arc::clone(&self.running);
        let cancel = Arc::clone(&self.cancel);
        let paused = Arc::clone(&self.paused);

        let known_processes: Vec<(String, Vec<String>)> = REGISTRY
            .games()
            .map(|g| {
                (
                    g.id().to_string(),
                    g.process_names()
                        .iter()
                        .map(|n| n.to_ascii_lowercase())
                        .collect(),
                )
            })
            .collect();

        // Seed `running` synchronously before returning to the caller, so the
        // frontend's immediate `getProcessState()` (which the dev-mode pull
        // path relies on) always reports the true state — never an empty set
        // racing the polling task's first tick.
        if let Ok(snap) = ProcessSnapshot::capture() {
            *running.write() = scan_running(&known_processes, &snap);
        }

        let handle = async_runtime::spawn(async move {
            loop {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }

                if !paused.load(Ordering::Relaxed)
                    && let Ok(snap) = ProcessSnapshot::capture()
                {
                    let current: HashSet<String> = scan_running(&known_processes, &snap);
                    let prev = running.read().clone();
                    if current != prev {
                        *running.write() = current.clone();
                        let mut v: Vec<String> = current.into_iter().collect();
                        v.sort();
                        let _ = app.emit("process_state", ProcessStateEvent { running: v });
                    }
                }

                let delay_ms = if focused.load(Ordering::Relaxed) {
                    750
                } else {
                    3000
                };
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        });
        self.handle = Some(handle);
    }

    pub fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}

fn scan_running(games: &[(String, Vec<String>)], snap: &ProcessSnapshot) -> HashSet<String> {
    let process_set: HashSet<String> = snap
        .entries
        .iter()
        .map(|p| p.name.to_ascii_lowercase())
        .collect();
    games
        .iter()
        .filter_map(|(id, names)| {
            if names.iter().any(|n| process_set.contains(n)) {
                Some(id.clone())
            } else {
                None
            }
        })
        .collect()
}
