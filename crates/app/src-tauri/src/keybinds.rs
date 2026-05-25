//! Per-game global hotkey bindings for switchable features.
//!
//! Each toggle / single-CTA feature can have ONE chord (e.g. `F5`,
//! `CommandOrControl+Shift+G`) bound to it. The store is persisted to
//! `keybinds.json` under the config dir and indexed two ways so both
//! lookups (chord → feature, feature → chord) are O(1).
//!
//! Registration lifetime: chords are registered with the OS once at
//! app startup (`register_at_startup`) and mutated piecewise by the
//! `set_keybind` / `clear_keybind` IPC commands. Attach/detach do
//! NOT touch the OS-level shortcut set; `dispatch()` is the gate that
//! enforces attach-awareness — pressing a hotkey while idle / on the
//! wrong game logs and no-ops. Earlier we cycled register/unregister
//! per attach, which wedged `tauri-plugin-global-shortcut`'s
//! internal HashMap and caused presses to silently no-op.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use openforge_runtime::{Value, WriteStrategyKind};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tracing::{info, warn};

use crate::state::AppState;
use crate::types::{
    FeatureChangedEvent, FeatureFreezeToggledEvent, HotkeyAction, HotkeyFiredEvent,
};

/// In-memory + on-disk store of per-game keybindings.
///
/// On-disk shape (`keybinds.json`):
/// ```json
/// {
///   "bindings": {
///     "batman-lod": { "lock_health": "F5", "god_mode": "F6" }
///   }
/// }
/// ```
///
/// The reverse `by_chord` index is derived on load and after every
/// mutation; it is *not* serialized.
#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct KeybindStore {
    /// `game_id` → `feature_id` → canonical chord string.
    bindings: HashMap<String, HashMap<String, String>>,
    #[serde(skip)]
    by_chord: HashMap<String, (String, String)>,
    #[serde(skip)]
    path: PathBuf,
}

impl KeybindStore {
    /// Load from `path`. Missing or corrupt files yield an empty store —
    /// keybindings are user preference, never load-bearing.
    pub fn load(path: PathBuf) -> Self {
        let mut store: Self = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                warn!(error = %e, "keybinds: parse failed, starting empty");
                Self::default()
            }),
            Err(_) => Self::default(),
        };
        store.path = path;
        store.rebuild_by_chord();
        store
    }

    fn rebuild_by_chord(&mut self) {
        self.by_chord.clear();
        for (game_id, fmap) in &self.bindings {
            for (feature_id, chord) in fmap {
                self.by_chord
                    .insert(chord.clone(), (game_id.clone(), feature_id.clone()));
            }
        }
    }

    /// Atomic-rename write so a half-completed save can't corrupt the
    /// store. Best-effort: a save failure is logged but never raises.
    pub fn save(&self) -> Result<()> {
        if self.path.as_os_str().is_empty() {
            return Ok(());
        }
        let tmp = self.path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(self)?;
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn list_for_game(&self, game_id: &str) -> Vec<(String, String)> {
        self.bindings
            .get(game_id)
            .map(|m| {
                m.iter()
                    .map(|(f, c)| (f.clone(), c.clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }

    /// Every chord across every game. Used at app startup to install
    /// all OS-level shortcuts in one shot — they live for the process
    /// lifetime and `dispatch()` filters by currently-attached game.
    pub fn all_chords(&self) -> Vec<String> {
        self.by_chord.keys().cloned().collect()
    }

    pub fn lookup_chord(&self, chord: &str) -> Option<&(String, String)> {
        self.by_chord.get(chord)
    }

    pub fn lookup_feature(&self, game_id: &str, feature_id: &str) -> Option<&String> {
        self.bindings.get(game_id).and_then(|m| m.get(feature_id))
    }

    /// Install `chord` for `(game_id, feature_id)`, displacing any
    /// previous binding for the same feature (returned for unregister).
    /// Does NOT enforce conflict detection — call `lookup_chord` first
    /// if the caller wants override semantics.
    pub fn set(&mut self, game_id: &str, feature_id: &str, chord: String) -> Option<String> {
        // Remove any other feature currently holding `chord` (cross-game
        // displacement). This shouldn't normally happen — set() is called
        // after the caller has resolved the conflict — but keep the
        // invariant tight so the two indices can't disagree.
        if let Some((old_game, old_feat)) = self.by_chord.remove(&chord)
            && let Some(fmap) = self.bindings.get_mut(&old_game)
        {
            fmap.remove(&old_feat);
        }
        let prev_chord = self
            .bindings
            .entry(game_id.to_string())
            .or_default()
            .insert(feature_id.to_string(), chord.clone());
        if let Some(p) = &prev_chord {
            self.by_chord.remove(p);
        }
        self.by_chord
            .insert(chord, (game_id.to_string(), feature_id.to_string()));
        prev_chord
    }

    pub fn clear(&mut self, game_id: &str, feature_id: &str) -> Option<String> {
        let prev = self
            .bindings
            .get_mut(game_id)
            .and_then(|m| m.remove(feature_id));
        if let Some(p) = &prev {
            self.by_chord.remove(p);
        }
        prev
    }
}

/// Canonicalize a chord by parsing then re-serializing through the
/// plugin's `Shortcut::Display` — guarantees the same key the FE typed
/// matches what the OS later hands back in the handler.
pub fn canonicalize(chord: &str) -> Result<String, String> {
    let s: Shortcut = chord
        .parse()
        .map_err(|e| format!("invalid chord '{chord}': {e}"))?;
    Ok(s.to_string())
}

/// Install every persisted chord with the OS as a global shortcut.
/// Called ONCE at app startup. Lifetime decision: chord bindings are
/// user-config that should fire whenever the trainer is running —
/// scoping them to "while attached" via attach/detach register cycles
/// turned out to wedge `tauri-plugin-global-shortcut`'s internal
/// HashMap (the second `register(F12)` returns "already registered"
/// without re-arming the OS callback, so the press silently no-ops).
/// Process-lifetime registration sidesteps that; dispatch is still
/// attach-aware — `dispatch()` bails when no game is attached or the
/// wrong game is.
pub fn register_at_startup(app: &AppHandle) {
    let chords = {
        let st = app.state::<AppState>();
        st.keybinds.read().all_chords()
    };
    if chords.is_empty() {
        info!("hotkeys: no bindings to register at startup");
        return;
    }
    info!(count = chords.len(), "hotkeys: registering at startup");
    for chord in chords {
        register_one(app, &chord);
    }
}

/// Best-effort per-chord register. Logs success/failure so failures
/// (e.g. another app or OS subsystem already owns the chord) are
/// visible in the trainer's logs without taking the whole app down.
pub fn register_one(app: &AppHandle, chord: &str) {
    match app.global_shortcut().register(chord) {
        Ok(()) => info!(chord = %chord, "hotkey registered"),
        Err(e) => warn!(chord = %chord, error = %e, "hotkey register failed"),
    }
}

/// Per-chord register that surfaces the failure reason to the caller.
/// Use this on user-initiated bind paths where the FE needs to learn
/// "the OS refused; ask for a different chord" — startup/bulk paths
/// should keep using `register_one` so one bad chord doesn't take the
/// rest down.
pub fn try_register(app: &AppHandle, chord: &str) -> Result<(), String> {
    match app.global_shortcut().register(chord) {
        Ok(()) => {
            info!(chord = %chord, "hotkey registered");
            Ok(())
        }
        Err(e) => {
            let reason = e.to_string();
            warn!(chord = %chord, error = %reason, "hotkey register failed");
            Err(reason)
        }
    }
}

/// Best-effort per-chord unregister. "Not registered" is treated as
/// success — the caller's invariant ("this chord should not be
/// active") still holds.
pub fn unregister_one(app: &AppHandle, chord: &str) {
    match app.global_shortcut().unregister(chord) {
        Ok(()) => info!(chord = %chord, "hotkey unregistered"),
        Err(e) => {
            warn!(chord = %chord, error = %e, "hotkey unregister failed (ok if it was never registered)")
        }
    }
}

/// Entry point wired into `Builder::with_handler(...)` in `main.rs`.
/// Fires for every registered chord; we resolve to a feature and
/// dispatch on a worker thread so a slow IPC round-trip can't block
/// the global-shortcut listener.
pub fn on_shortcut(app: &AppHandle, shortcut: &Shortcut, state: ShortcutState) {
    // Always log: gives us a single-line audit trail of "did the OS
    // even hand the press to us?" — separate from later
    // dispatch-bailed-out cases.
    info!(chord = %shortcut, state = ?state, "hotkey: received");
    if state != ShortcutState::Pressed {
        return;
    }
    let chord = shortcut.to_string();
    let app_clone = app.clone();
    tauri::async_runtime::spawn(async move {
        dispatch(app_clone, chord).await;
    });
}

async fn dispatch(app: AppHandle, chord: String) {
    let state = app.state::<AppState>();

    // 1. Lookup binding under a short read-lock.
    let Some((game_id, feature_id)) = state.keybinds.read().lookup_chord(&chord).cloned() else {
        info!(chord = %chord, "hotkey: no binding (stale registration?)");
        return;
    };

    // 2. Verify attached + same game + feature resolved.
    let (display_name, strategy) = {
        let attached_guard = state.attached.read();
        let Some(att) = attached_guard.as_ref() else {
            info!(chord = %chord, "hotkey: not attached, ignoring");
            return;
        };
        if att.game_id != game_id {
            info!(chord = %chord, want = %game_id, have = %att.game_id, "hotkey: other game attached, ignoring");
            return;
        }
        if att.feature_addr(&feature_id).is_none() {
            warn!(chord = %chord, feature = %feature_id, "hotkey: feature not resolved");
            return;
        }
        let Some(feature) = state.registry.feature(&game_id, &feature_id) else {
            return;
        };
        (feature.display_name().to_string(), feature.write_strategy())
    };

    // 3. Branch on strategy. We deliberately reuse the existing
    //    `#[tauri::command]` functions (they are plain `fn`s under the
    //    macro and still callable from Rust) so the click path and the
    //    hotkey path stay bit-identical.
    let action = match strategy {
        WriteStrategyKind::Freeze => match toggle_freeze(&app, &game_id, &feature_id).await {
            Some(now_on) => {
                if now_on {
                    HotkeyAction::ToggleOn
                } else {
                    HotkeyAction::ToggleOff
                }
            }
            None => return,
        },
        WriteStrategyKind::CodePatch => {
            match toggle_code_patch(&app, &game_id, &feature_id).await {
                Some(now_on) => {
                    if now_on {
                        HotkeyAction::ToggleOn
                    } else {
                        HotkeyAction::ToggleOff
                    }
                }
                None => return,
            }
        }
        WriteStrategyKind::OneShot => {
            // Bool-synthesized one-shots (SetProgressTags,
            // TeleportToWaypoint, CallInstanceUFunction): the standard
            // "fire" is `Value::Bool(true)`. Numeric one-shots aren't
            // bindable in the first place (no value to enter) — they
            // shouldn't reach this branch because the FE never offers
            // an Assign button for them.
            if !fire_one_shot(&app, &game_id, &feature_id).await {
                return;
            }
            HotkeyAction::Fire
        }
    };

    let _ = app.emit(
        "hotkey_fired",
        HotkeyFiredEvent {
            game_id,
            feature_id,
            display_name,
            action,
        },
    );
}

/// Toggle freeze ON ↔ OFF using `freeze_handles` as the source of
/// truth. Returns Some(new_state) on success.
async fn toggle_freeze(app: &AppHandle, game_id: &str, feature_id: &str) -> Option<bool> {
    let key = (game_id.to_string(), feature_id.to_string());
    let was_on = {
        let st = app.state::<AppState>();
        st.freeze_handles.lock().contains_key(&key)
    };
    let new_state = !was_on;

    // Run the synchronous command on a blocking worker so the dispatch
    // task doesn't hold the freeze loop's spawn for IPC time.
    let app_for_blocking = app.clone();
    let game_id_owned = game_id.to_string();
    let feature_id_owned = feature_id.to_string();
    let res = tauri::async_runtime::spawn_blocking(move || {
        let st = app_for_blocking.state::<AppState>();
        crate::commands::set_freeze(
            st,
            app_for_blocking.clone(),
            game_id_owned,
            feature_id_owned,
            new_state,
        )
    })
    .await;
    match res {
        Ok(Ok(())) => {
            // Notify FE so the Switch's `checked` state stays in sync
            // — the freeze loop's own `feature_freeze_state` event only
            // covers ON→active and ON→waiting, not OFF, and is keyed
            // on tick, not on user intent.
            let _ = app.emit(
                "feature_freeze_toggled",
                FeatureFreezeToggledEvent {
                    game_id: game_id.to_string(),
                    feature_id: feature_id.to_string(),
                    frozen: new_state,
                },
            );
            Some(new_state)
        }
        Ok(Err(e)) => {
            warn!(error = %e, "hotkey: set_freeze failed");
            None
        }
        Err(e) => {
            warn!(error = %e, "hotkey: set_freeze worker panicked");
            None
        }
    }
}

/// Toggle a code-patch / bool feature. Source-of-truth is the live
/// value read from the game (matches the SwitchControl in `feature-card.tsx`).
async fn toggle_code_patch(app: &AppHandle, game_id: &str, feature_id: &str) -> Option<bool> {
    // Read current to determine the toggle direction.
    let current_applied = {
        let st = app.state::<AppState>();
        let attached_guard = st.attached.read();
        let att = attached_guard.as_ref()?;
        if att.game_id != game_id {
            return None;
        }
        let addr = att.feature_addr(feature_id)?;
        let feature = st.registry.feature(game_id, feature_id)?;
        match feature.read(att.session.as_ref(), addr) {
            Ok(Value::Bool(b)) => b,
            // code_patch features always read as Bool; any other kind
            // would be a contract violation. Bail loudly rather than
            // guess at "applied".
            Ok(other) => {
                warn!(feature = %feature_id, kind = ?other.kind(), "hotkey: code_patch read returned non-bool");
                return None;
            }
            Err(e) => {
                warn!(error = %e, "hotkey: read current failed");
                return None;
            }
        }
    };
    let new_state = !current_applied;

    let app_for_blocking = app.clone();
    let game_id_owned = game_id.to_string();
    let feature_id_owned = feature_id.to_string();
    let res = tauri::async_runtime::spawn_blocking(move || {
        let st = app_for_blocking.state::<AppState>();
        crate::commands::set_code_patch(st, game_id_owned, feature_id_owned, new_state)
    })
    .await;
    match res {
        Ok(Ok(())) => {
            // Push the new value through the same channel a click would
            // — FE's `onFeatureChanged` reads this and updates the store,
            // which is what drives `isApplied(current)` on the Switch.
            let _ = app.emit(
                "feature_changed",
                FeatureChangedEvent {
                    game_id: game_id.to_string(),
                    feature_id: feature_id.to_string(),
                    value: Value::Bool(new_state),
                },
            );
            Some(new_state)
        }
        Ok(Err(e)) => {
            warn!(error = %e, "hotkey: set_code_patch failed");
            None
        }
        Err(e) => {
            warn!(error = %e, "hotkey: set_code_patch worker panicked");
            None
        }
    }
}

/// Single-shot CTA: write `Value::Bool(true)`. Returns true on success.
async fn fire_one_shot(app: &AppHandle, game_id: &str, feature_id: &str) -> bool {
    let app_for_blocking = app.clone();
    let game_id_owned = game_id.to_string();
    let feature_id_owned = feature_id.to_string();
    let res = tauri::async_runtime::spawn_blocking(move || {
        let st = app_for_blocking.state::<AppState>();
        crate::commands::write_feature(st, game_id_owned, feature_id_owned, Value::Bool(true))
    })
    .await;
    match res {
        Ok(Ok(_v)) => true,
        Ok(Err(e)) => {
            warn!(error = %e, "hotkey: write_feature (one-shot) failed");
            false
        }
        Err(e) => {
            warn!(error = %e, "hotkey: one-shot worker panicked");
            false
        }
    }
}
