//! Attach lifecycle: state machine + the `Attached` runtime struct.
//!
//! Single-channel architecture: `Attached` holds an
//! `Arc<Ue5Session>` instead of an `Arc<Target>`. Every read / write / scan /
//! patch routes through the injected DLL via named pipe. The host process
//! itself never calls `WriteProcessMemory` / `ReadProcessMemory` against the
//! game.
//!
//! Code-patch lifecycle: applied patches are tracked **inside the DLL** (per
//! pipe-client `ConnState`). When the host detaches (clean disconnect or
//! game-exit auto-detach), dropping the `Ue5Session` closes the pipe; the
//! DLL auto-restores every patch as part of `ConnState::drop`. The trainer
//! no longer maintains its own applied-patches set.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use openforge_core::Ctx;
use openforge_runtime::Feature;
use openforge_ue5_host::Ue5Session;
use parking_lot::Mutex;
use tauri::async_runtime::JoinHandle;

use crate::profile::CachedAddress;
use crate::types::AttachStatePayload;

pub struct Attached {
    pub session: Arc<Ue5Session>,
    pub game_id: String,
    pub detected_version: String,
    pub resolutions: HashMap<String, ResolvedFeature>,
    /// Per-feature snapshot captured immediately after a successful resolve
    /// (before any user writes). Used to restore "default" values when a
    /// freeze toggle is switched off — the user expects `OFF = back to stock`.
    /// The byte layout is whatever `feature.snapshot()` returned: primary
    /// field first, then any reflection `also_write` companions concatenated.
    pub feature_snapshots: Mutex<HashMap<String, Vec<u8>>>,
    /// Background task that blocks on the attached process's exit handle. When
    /// the game dies we auto-detach. User-initiated detach aborts this task
    /// first so the auto-detach handler doesn't fire on an already-clean state.
    pub exit_watch: Mutex<Option<JoinHandle<()>>>,
    /// Cancellation flag + handle for the currently-running Lua script's
    /// polling task (drains `print()` output, emits `lua_output` /
    /// `lua_script_status` events). `None` when no script is running.
    ///
    /// Stop semantics:
    ///   * User clicks Stop in the UI → command sets `cancel = true`, sends
    ///     `StopLua` to the DLL, polling task observes flag + exits.
    ///   * New Run replaces prior → command first cancels the old token,
    ///     awaits the prior status event, then installs a fresh one.
    ///   * Process exit auto-detach → `do_detach` cancels before dropping
    ///     `Attached`, so the polling task doesn't outlive the pipe.
    pub lua_polling: Mutex<Option<LuaPollingHandle>>,
}

/// Shared cancellation handle for one Lua polling task.
pub struct LuaPollingHandle {
    pub cancel: Arc<AtomicBool>,
}

impl LuaPollingHandle {
    pub fn new() -> (Self, Arc<AtomicBool>) {
        let cancel = Arc::new(AtomicBool::new(false));
        (
            Self {
                cancel: cancel.clone(),
            },
            cancel,
        )
    }

    pub fn cancel(&self) {
        self.cancel
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedFeature {
    pub feature_id: String,
    pub address: Option<usize>,
    pub error: Option<String>,
    pub status: ResolutionStatus,
}

/// Tri-state outcome of a feature resolution attempt.
///
/// `Resolved` and `Failed` are terminal. `Pending` is transient: the
/// feature's prerequisites (e.g. a live UE5 UObject) aren't loaded yet,
/// typically because the player is on the main menu. The background retry
/// loop ([`commands::spawn_pending_retry_task`]) re-runs `resolve()` on
/// Pending features every few seconds and promotes them to Resolved or
/// Failed as state changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionStatus {
    /// Address found; feature is fully usable.
    Resolved,
    /// Transient miss — live game object not loaded. Auto-retry will
    /// promote this to Resolved (or eventually Failed) on its own.
    Pending,
    /// Permanent failure (bad TOML, missing class on the build, etc.).
    /// Retry won't help; user intervention is needed.
    Failed,
}

impl ResolutionStatus {
    /// String tag for the wire DTO + UI logic.
    pub fn as_str(self) -> &'static str {
        match self {
            ResolutionStatus::Resolved => "resolved",
            ResolutionStatus::Pending => "pending",
            ResolutionStatus::Failed => "failed",
        }
    }
}

/// Heuristic: is this resolve error a transient "world isn't loaded yet"
/// state rather than a permanent configuration / build mismatch?
///
/// We pattern-match on the error message because Rust's error variants
/// don't carry enough metadata yet (and adding a `Transient` variant would
/// thread through `openforge_core::Error`, `RuntimeError`, and every layer
/// that surfaces a resolve failure). The strings below are produced by:
///
/// - `feature.rs::resolve_via_reflection` — "reflection: no live UObject"
/// - `feature.rs::resolve` (SetProgressTags branch) — "set_progress_tags: no live"
/// - `feature.rs::write_set_progress_tags` — "set_progress_tags: no live world-context"
/// - `engine` returning `NotFound` translated to "FindUObject(...) returned NotFound"
pub fn is_transient_resolve_error(msg: &str) -> bool {
    let lower = msg.to_ascii_lowercase();
    lower.contains("no live uobject")
        || lower.contains("set_progress_tags: no live")
        || lower.contains("isn't loaded yet")
        || lower.contains("not loaded yet")
        || lower.contains("is the player in a level")
        || lower.contains("notfound")
        // Deref-chain failures along a "wrong concrete subclass" path
        // (e.g. PawnPrivate currently points at a vehicle pawn, not a
        // character — vehicle classes have no `CharacterMovement` field).
        // These resolve themselves as the player switches possessions, so
        // they're transient by nature.
        || lower.contains("reflection.deref:")
        || (lower.contains("predicate any") && lower.contains("not found"))
}

impl Attached {
    pub fn feature_addr(&self, feature_id: &str) -> Option<usize> {
        self.resolutions.get(feature_id).and_then(|r| r.address)
    }

    pub fn set_exit_watch(&self, handle: JoinHandle<()>) {
        if let Some(prev) = self.exit_watch.lock().replace(handle) {
            prev.abort();
        }
    }

    pub fn take_exit_watch(&self) -> Option<JoinHandle<()>> {
        self.exit_watch.lock().take()
    }

    pub fn resolve_all(
        session: Arc<Ue5Session>,
        game_id: &str,
        detected_version: &str,
        features: &[&'static dyn Feature],
        cache: &HashMap<String, CachedAddress>,
        progress: impl Fn(usize, usize, &str, ResolutionStatus, Option<&str>),
    ) -> Self {
        let mut resolutions: HashMap<String, ResolvedFeature> = HashMap::new();
        let mut feature_snapshots: HashMap<String, Vec<u8>> = HashMap::new();
        let total = features.len();
        let module_base = session.main_module().base as u64;
        for (i, feature) in features.iter().enumerate() {
            let id = feature.id().to_string();
            // --- Cache fast path ---
            // Same process (module_base match) + feature's own validators pass
            // = we can skip the scan entirely. The biggest attach-time win on
            // re-attach within the same session.
            let cached_hit = cache.get(&id).and_then(|cached| {
                if cached.module_base != module_base {
                    None
                } else {
                    let addr = cached.address as usize;
                    if feature.quick_check(session.as_ref() as &dyn Ctx, addr) {
                        Some(addr)
                    } else {
                        None
                    }
                }
            });

            let result: Result<usize, openforge_core::Error> = match cached_hit {
                Some(addr) => {
                    tracing::info!(
                        game = %game_id,
                        feature = %id,
                        addr = format!("0x{addr:X}"),
                        "cache hit"
                    );
                    Ok(addr)
                }
                None => feature.resolve(session.as_ref()),
            };

            match result {
                Ok(addr) => {
                    progress(i + 1, total, &id, ResolutionStatus::Resolved, None);
                    // Best-effort snapshot of pre-modification bytes so a
                    // later freeze toggle-off can restore "default". Failure
                    // here is non-fatal — the feature still resolves, the
                    // restore path will silently no-op.
                    match feature.snapshot(session.as_ref() as &dyn Ctx, addr) {
                        Ok(bytes) => {
                            feature_snapshots.insert(id.clone(), bytes);
                        }
                        Err(e) => {
                            tracing::warn!(
                                game = %game_id,
                                feature = %id,
                                error = %e,
                                "snapshot failed — restore-on-toggle-off will be a no-op"
                            );
                        }
                    }
                    resolutions.insert(
                        id.clone(),
                        ResolvedFeature {
                            feature_id: id,
                            address: Some(addr),
                            error: None,
                            status: ResolutionStatus::Resolved,
                        },
                    );
                }
                Err(e) => {
                    let msg = e.to_string();
                    let status = if is_transient_resolve_error(&msg) {
                        ResolutionStatus::Pending
                    } else {
                        ResolutionStatus::Failed
                    };
                    if status == ResolutionStatus::Pending {
                        tracing::info!(
                            game = %game_id,
                            feature = %id,
                            "feature pending — live object not loaded yet (will retry)"
                        );
                    } else {
                        tracing::warn!(
                            game = %game_id,
                            feature = %id,
                            error = %msg,
                            "feature resolve failed"
                        );
                    }
                    progress(i + 1, total, &id, status, Some(&msg));
                    resolutions.insert(
                        id.clone(),
                        ResolvedFeature {
                            feature_id: id,
                            address: None,
                            error: Some(msg),
                            status,
                        },
                    );
                }
            }
        }
        Self {
            session,
            game_id: game_id.to_string(),
            detected_version: detected_version.to_string(),
            resolutions,
            feature_snapshots: Mutex::new(feature_snapshots),
            exit_watch: Mutex::new(None),
            lua_polling: Mutex::new(None),
        }
    }
}

pub fn idle() -> AttachStatePayload {
    AttachStatePayload::Idle
}

pub fn attaching(game_id: &str) -> AttachStatePayload {
    AttachStatePayload::Attaching {
        game_id: game_id.to_string(),
    }
}

pub fn resolving(game_id: &str, resolved: usize, total: usize) -> AttachStatePayload {
    AttachStatePayload::ResolvingAobs {
        game_id: game_id.to_string(),
        resolved,
        total,
    }
}

pub fn finalizing(game_id: &str) -> AttachStatePayload {
    AttachStatePayload::Finalizing {
        game_id: game_id.to_string(),
    }
}

pub fn attached(game_id: &str, pid: u32, detected_version: &str) -> AttachStatePayload {
    AttachStatePayload::Attached {
        game_id: game_id.to_string(),
        pid,
        detected_version: detected_version.to_string(),
    }
}
