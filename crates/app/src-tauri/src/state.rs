use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use parking_lot::{Mutex, RwLock};
use tauri::AppHandle;
use tauri::async_runtime::JoinHandle;

use crate::attach::Attached;
use crate::keybinds::KeybindStore;
use crate::paths::AppPaths;
use crate::settings::Settings;
use crate::watch::Watcher;

pub struct AppState {
    pub registry: &'static openforge_runtime::Registry,
    pub watcher: Mutex<Watcher>,
    pub attached: RwLock<Option<Attached>>,
    pub active_game_id: RwLock<Option<String>>,
    pub settings: RwLock<Settings>,
    pub paths: AppPaths,
    pub freeze_handles: Mutex<HashMap<(String, String), JoinHandle<()>>>,
    /// Per-feature read-probe loops (one per reflection-backed non-freeze
    /// feature). Started at attach time, aborted at detach. Independent of
    /// `freeze_handles` because freeze and probe loops can't both run on
    /// the same feature — freeze loops already cover their own health
    /// reporting via `feature.write()` returning Ok/Err.
    pub read_probe_handles: Mutex<HashMap<(String, String), JoinHandle<()>>>,
    /// DLL-side freeze handles for Glacier god-mode-style freezes (a
    /// `freeze_copy_offset` feature on a Glacier session). Unlike
    /// `freeze_handles`, the freeze runs *inside the DLL's* per-frame thread
    /// (started via `GlacierSession::start_freeze`), so we only need the opaque
    /// `u32` handle here to stop it — there is no host-side `JoinHandle`. Keyed
    /// `(game_id, feature_id)`, mirroring `freeze_handles`.
    pub glacier_freeze_handles: Mutex<HashMap<(String, String), u32>>,
    pub window_focused: Arc<AtomicBool>,
    pub app_handle: RwLock<Option<AppHandle>>,
    /// Per-game global hotkey bindings. Loaded once from disk at
    /// startup; mutated by the `set_keybind` / `clear_keybind`
    /// commands, which save back to disk after each change.
    pub keybinds: RwLock<KeybindStore>,
}

impl AppState {
    pub fn new(paths: AppPaths, settings: Settings, keybinds: KeybindStore) -> Self {
        Self {
            registry: &openforge_runtime::REGISTRY,
            watcher: Mutex::new(Watcher::new()),
            attached: RwLock::new(None),
            active_game_id: RwLock::new(settings.last_active_game.clone()),
            settings: RwLock::new(settings),
            paths,
            freeze_handles: Mutex::new(HashMap::new()),
            read_probe_handles: Mutex::new(HashMap::new()),
            glacier_freeze_handles: Mutex::new(HashMap::new()),
            window_focused: Arc::new(AtomicBool::new(true)),
            app_handle: RwLock::new(None),
            keybinds: RwLock::new(keybinds),
        }
    }

    pub fn set_app_handle(&self, handle: AppHandle) {
        *self.app_handle.write() = Some(handle);
    }
}
