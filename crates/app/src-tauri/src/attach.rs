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
use openforge_engine::EngineSession;
use openforge_runtime::{EngineKind, Feature};
use openforge_ue5_host::Ue5Session;
use parking_lot::Mutex;
use tauri::async_runtime::JoinHandle;

use crate::profile::CachedAddress;
use crate::types::AttachStatePayload;

/// The injected-DLL session backing an attach, held as an engine-agnostic trait
/// object.
///
/// UE5 and Glacier games share the `Ctx` memory surface (read / write / scan /
/// the declarative resolve), so feature resolution and the freeze/probe write
/// paths are backend-agnostic — they call through [`session_as_ctx`]. They
/// diverge only on game-specific extras — UE5 reflection + Lua reach the
/// concrete session via the [`as_ue5`] downcast, while DLL-side freeze is an
/// [`EngineSession`] capability (`supports_dll_freeze`/`dll_freeze`) so no
/// engine-identity downcast is needed. The concrete backend is selected at
/// attach time from the game's manifest-declared [`EngineKind`] (see
/// `commands::attach`).
pub type Session = Arc<dyn EngineSession>;

/// The shared memory surface every feature resolve / read / write uses.
pub fn session_as_ctx(session: &Session) -> &dyn Ctx {
    session.as_ctx()
}

/// Main-module load base — the resolved-address cache key. Falls back to `0`
/// if the session can't report a base (should not happen post-handshake; the
/// only effect would be a cache miss).
pub fn session_main_module_base(session: &Session) -> u64 {
    session.main_module_base().unwrap_or(0)
}

/// Human-readable engine label for logs / UI ("UE5" / "Glacier 2").
pub fn engine_label(session: &Session) -> &'static str {
    match session.engine_kind() {
        EngineKind::Ue5 => "UE5",
        EngineKind::Glacier2 => "Glacier 2",
    }
}

/// Downcast to the concrete UE5 session, when this attach is UE5-backed
/// (Lua + reflection-explorer ops). Replaces the old `Session::ue5` accessor.
pub fn as_ue5(session: &Session) -> Option<&Ue5Session> {
    session.as_any().downcast_ref::<Ue5Session>()
}

/// Owned-`Arc` variant of [`as_ue5`]: recover the concrete `Arc<Ue5Session>`
/// so it can be moved into a long-lived background task (read-probe + Lua
/// polling). Replaces the old `session.ue5().map(Arc::clone)` idiom.
pub fn as_ue5_arc(session: &Session) -> Option<Arc<Ue5Session>> {
    Arc::clone(session)
        .into_any_arc()
        .downcast::<Ue5Session>()
        .ok()
}

/// Downcast to the concrete Glacier session, when this attach is Glacier-backed.
/// Used by the engine-action commands (`give weapon` / `list firearms`), whose
/// dispatch — `game_thread_call` + the firearm heap-walk — is inherent to
/// `GlacierSession` rather than part of the engine-agnostic `Ctx`/`EngineSession`
/// surface.
#[cfg(windows)]
pub fn as_glacier(session: &Session) -> Option<&openforge_glacier_host::GlacierSession> {
    session
        .as_any()
        .downcast_ref::<openforge_glacier_host::GlacierSession>()
}

pub struct Attached {
    pub session: Session,
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

/// Shared cancellation handle for one Lua polling task. Carries the
/// `(source, slug)` of the script the task is polling so the `stop_lua_script`
/// command can emit a properly-keyed `lua_script_status` event (otherwise
/// the FE has no way to know WHICH script just stopped — it would see the
/// synthetic stop event with empty slug and ignore it).
pub struct LuaPollingHandle {
    pub cancel: Arc<AtomicBool>,
    /// `"user"` or `"community"` — matches the LuaSource the script was
    /// loaded from.
    pub source: String,
    pub slug: String,
}

impl LuaPollingHandle {
    pub fn new(source: String, slug: String) -> (Self, Arc<AtomicBool>) {
        let cancel = Arc::new(AtomicBool::new(false));
        (
            Self {
                cancel: cancel.clone(),
                source,
                slug,
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
        session: Session,
        game_id: &str,
        detected_version: &str,
        features: &[&'static dyn Feature],
        cache: &HashMap<String, CachedAddress>,
        progress: impl Fn(usize, usize, &str, ResolutionStatus, Option<&str>),
    ) -> Self {
        let mut resolutions: HashMap<String, ResolvedFeature> = HashMap::new();
        let mut feature_snapshots: HashMap<String, Vec<u8>> = HashMap::new();
        let total = features.len();
        let module_base = session_main_module_base(&session);
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
                    if feature.quick_check(session.as_ctx(), addr) {
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
                None => feature.resolve(session.as_ctx()),
            };

            match result {
                Ok(addr) => {
                    progress(i + 1, total, &id, ResolutionStatus::Resolved, None);
                    // Best-effort snapshot of pre-modification bytes so a
                    // later freeze toggle-off can restore "default". Failure
                    // here is non-fatal — the feature still resolves, the
                    // restore path will silently no-op.
                    match feature.snapshot(session.as_ctx(), addr) {
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

#[cfg(test)]
mod engine_registry_tests {
    use openforge_runtime::EngineKind;

    // Force-link both host crates' `register_engine!` inventory items in THIS
    // test binary by calling each backend's `ensure_linked()` (a bare
    // `use ... as _` is a no-op that does NOT force linkage — that gap is what
    // let Glacier2 dead-strip from the real app binary). The production binary
    // force-links the same way from `main.rs` startup (and asserts both kinds
    // resolved). This test guards the registry resolution itself.
    fn force_link() {
        openforge_glacier_host::backend::ensure_linked();
        openforge_ue5_host::backend::ensure_linked();
    }

    /// R-4 mitigation: prove the inventory registry resolves a backend for both
    /// shipped engine kinds. If a host crate's `register_engine!` ever fails to
    /// link, this fails instead of silently returning `None` at attach.
    #[test]
    fn both_engine_kinds_have_a_registered_backend() {
        force_link();
        assert!(
            openforge_engine::backend_for(EngineKind::Ue5).is_some(),
            "no backend registered for EngineKind::Ue5"
        );
        assert!(
            openforge_engine::backend_for(EngineKind::Glacier2).is_some(),
            "no backend registered for EngineKind::Glacier2"
        );
    }
}
