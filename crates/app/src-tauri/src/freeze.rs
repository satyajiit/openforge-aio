//! Freeze loops. Runs against an `Arc<Ue5Session>`
//! (the IPC-backed `Ctx`) instead of a native `Target`. The write itself is
//! identical from the trait's perspective.

use std::sync::Arc;
use std::time::Duration;

use openforge_runtime::{Feature, Value};
use openforge_ue5_host::Ue5Session;
use tauri::async_runtime::{self, JoinHandle};
use tauri::{AppHandle, Emitter};
use tracing::{info, warn};

use crate::attach;
use crate::types::{FeatureChangedEvent, FeatureFreezeStateEvent};

pub fn spawn_freeze_task(
    app: AppHandle,
    session: Arc<Ue5Session>,
    feature: &'static dyn Feature,
    addr: usize,
    desired: Value,
    game_id: String,
    interval_ms: u32,
) -> JoinHandle<()> {
    let feature_id = feature.id().to_string();
    async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(interval_ms.max(50) as u64));
        // Track current freeze health so we only emit on transitions. Start
        // as `None` so the first tick — success or failure — always emits.
        // The UI keeps the switch ON throughout; the badge just toggles.
        let mut last_healthy: Option<bool> = None;
        loop {
            tick.tick().await;
            let result = feature.write(session.as_ref(), addr, desired);
            let healthy_now = result.is_ok();
            match &result {
                Ok(()) => {
                    let _ = app.emit(
                        "feature_changed",
                        FeatureChangedEvent {
                            game_id: game_id.clone(),
                            feature_id: feature_id.clone(),
                            value: desired,
                        },
                    );
                }
                Err(e) => {
                    warn!(
                        game = %game_id,
                        feature = %feature_id,
                        addr = format!("0x{addr:X}"),
                        error = %e,
                        "freeze write failed; continuing"
                    );
                }
            }
            // Edge-trigger the state event. Saves 4 events/sec/feature on
            // the steady-state happy path.
            if last_healthy != Some(healthy_now) {
                last_healthy = Some(healthy_now);
                let (state, hint) = if healthy_now {
                    ("active", None)
                } else {
                    (
                        "waiting",
                        Some("Waiting for the level to load…".to_string()),
                    )
                };
                let _ = app.emit(
                    "feature_freeze_state",
                    FeatureFreezeStateEvent {
                        game_id: game_id.clone(),
                        feature_id: feature_id.clone(),
                        state: state.to_string(),
                        hint,
                    },
                );
            }
        }
    })
}

/// Lightweight read-probe loop for non-freeze reflection-backed features
/// (e.g. movement mods). The point isn't to write anything — it's to keep
/// the UI informed of whether the feature is currently *reachable*, so
/// transient states (player in a vehicle, in a cutscene, on the main menu)
/// surface as a "waiting" badge instead of an empty input field.
///
/// On every tick:
/// * Attempt `feature.read(...)`. If it succeeds, emit `feature_changed`
///   with the fresh value (so the input field stays current). If it fails
///   with a transient reflection error (vehicle, level transition, etc.),
///   the loop quietly keeps trying.
/// * Edge-trigger `feature_freeze_state` (reused for "runtime health"
///   semantics — both kinds of loops emit identical events, so the FE
///   doesn't need to distinguish).
///
/// Hard reads (genuine bugs — SignatureInvalid that isn't transient) are
/// still emitted as "waiting" but logged at WARN so they don't get lost.
///
/// 2-second cadence: slow enough to be free on the wire (~6 ops/min/feature),
/// fast enough that re-entering the level updates the badge within one
/// blink.
/// Run one read-probe tick: read the feature value, emit `feature_changed`
/// on success, and edge-trigger `feature_freeze_state` against the caller's
/// `last_healthy` baseline. Returns the new healthy state so the caller can
/// thread it forward (e.g. into the recurring task's pre-seeded state, so
/// the same transition isn't logged twice).
///
/// Synchronous — IPC-blocking. Callers in async contexts must wrap in
/// `spawn_blocking`.
pub fn probe_once(
    app: &AppHandle,
    session: &Ue5Session,
    feature: &'static dyn Feature,
    addr: usize,
    game_id: &str,
    last_healthy: Option<bool>,
) -> bool {
    let feature_id = feature.id();
    let result = feature.read(session, addr);
    let healthy_now = result.is_ok();
    match &result {
        Ok(v) => {
            let _ = app.emit(
                "feature_changed",
                FeatureChangedEvent {
                    game_id: game_id.to_string(),
                    feature_id: feature_id.to_string(),
                    value: *v,
                },
            );
        }
        Err(e) => {
            let msg = e.to_string();
            if !attach::is_transient_resolve_error(&msg) {
                warn!(
                    game = %game_id,
                    feature = %feature_id,
                    addr = format!("0x{addr:X}"),
                    error = %msg,
                    "read probe: non-transient failure"
                );
            }
        }
    }
    if last_healthy != Some(healthy_now) {
        let (state, hint) = if healthy_now {
            ("active", None)
        } else {
            (
                "waiting",
                Some(
                    "Currently unreachable — try entering a level or switching back to a character."
                        .to_string(),
                ),
            )
        };
        info!(
            game = %game_id,
            feature = %feature_id,
            state,
            "read probe: state transition"
        );
        let _ = app.emit(
            "feature_freeze_state",
            FeatureFreezeStateEvent {
                game_id: game_id.to_string(),
                feature_id: feature_id.to_string(),
                state: state.to_string(),
                hint,
            },
        );
    }
    healthy_now
}

pub fn spawn_read_probe_task(
    app: AppHandle,
    session: Arc<Ue5Session>,
    feature: &'static dyn Feature,
    addr: usize,
    game_id: String,
    initial_last_healthy: Option<bool>,
) -> JoinHandle<()> {
    // 4-second cadence: slow enough that the per-tick chain walk + read is
    // a rounding error on the game thread; fast enough that the "got out of
    // the car" transition shows the input value within ~one blink. Bumping
    // higher would feel laggy; lower started visibly stalling the game when
    // the cache invalidated and we re-walked properties on a fresh pawn
    // class.
    const PROBE_INTERVAL_MS: u64 = 4_000;
    let feature_id = feature.id().to_string();
    info!(
        game = %game_id,
        feature = %feature_id,
        addr = format!("0x{addr:X}"),
        interval_ms = PROBE_INTERVAL_MS,
        "spawning read probe"
    );
    async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(PROBE_INTERVAL_MS));
        // Skip the immediate first tick — `tokio::time::interval` fires
        // instantly otherwise, and the inline `probe_once` during the
        // attach finalize phase already covered initial state (and
        // pre-seeded our `last_healthy` so we don't re-log the transition
        // 4s later).
        tick.tick().await;
        let mut last_healthy: Option<bool> = initial_last_healthy;
        loop {
            tick.tick().await;
            last_healthy = Some(probe_once(
                &app,
                session.as_ref(),
                feature,
                addr,
                &game_id,
                last_healthy,
            ));
        }
    })
}
