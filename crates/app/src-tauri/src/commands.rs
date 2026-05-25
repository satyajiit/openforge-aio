use std::sync::Arc;

use openforge_core::ProcessSnapshot;
use openforge_runtime::{Feature, Game, Value};
use openforge_ue5_host::{Ue5Session, resolve_dll_path};
use tauri::{AppHandle, Emitter, Manager, State};
use tracing::{info, warn};

use crate::attach::{self, Attached};
use crate::elevation;
use crate::error::{AppError, AppResult};
use crate::freeze;
use crate::preflight;
use crate::profile::{self, CachedAddress, GameProfile};
use crate::settings::{self, Settings};
use crate::state::AppState;
use crate::types::{
    AttachInfo, AttachStatePayload, AttachStatusEvent, FeatureMeta, FeatureResolution,
    FeatureResolvedEvent, GameMeta, PreflightChangedEvent, PreflightReport, ProcessStateEvent,
    VersionWarningEvent, feature_meta_from,
};

#[tauri::command]
pub fn list_games(state: State<'_, AppState>) -> AppResult<Vec<GameMeta>> {
    let metas: Vec<GameMeta> = state
        .registry
        .games()
        .map(openforge_runtime::GameMeta::from_game)
        .map(GameMeta::from)
        .collect();
    Ok(metas)
}

#[tauri::command(rename_all = "camelCase")]
pub fn list_features(state: State<'_, AppState>, game_id: String) -> AppResult<Vec<FeatureMeta>> {
    if state.registry.game(&game_id).is_none() {
        return Err(AppError::UnknownGame(game_id));
    }
    let features = state.registry.features_for(&game_id);
    Ok(features.iter().map(|f| feature_meta_from(*f)).collect())
}

#[tauri::command]
pub fn start_watcher(state: State<'_, AppState>, app: AppHandle) -> AppResult<()> {
    state.set_app_handle(app.clone());
    let focused = Arc::clone(&state.window_focused);
    state.watcher.lock().start(app, focused);
    Ok(())
}

#[tauri::command]
pub fn stop_watcher(state: State<'_, AppState>) -> AppResult<()> {
    state.watcher.lock().stop();
    Ok(())
}

#[tauri::command]
pub fn get_process_state(state: State<'_, AppState>) -> AppResult<ProcessStateEvent> {
    Ok(ProcessStateEvent {
        running: state.watcher.lock().snapshot(),
    })
}

#[tauri::command(rename_all = "camelCase")]
pub fn preflight(
    state: State<'_, AppState>,
    app: AppHandle,
    game_id: String,
) -> AppResult<PreflightReport> {
    let game = state
        .registry
        .game(&game_id)
        .ok_or_else(|| AppError::UnknownGame(game_id.clone()))?;
    let report = preflight::evaluate(game);
    let _ = app.emit(
        "preflight_changed",
        PreflightChangedEvent {
            game_id,
            report: report.clone(),
        },
    );
    Ok(report)
}

#[tauri::command]
pub fn is_elevated() -> AppResult<bool> {
    Ok(elevation::is_elevated())
}

#[tauri::command]
pub fn relaunch_as_admin() -> AppResult<()> {
    elevation::relaunch_as_admin()
}

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> AppResult<Settings> {
    Ok(state.settings.read().clone())
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_settings(state: State<'_, AppState>, settings: Settings) -> AppResult<()> {
    *state.settings.write() = settings.clone();
    let path = state.paths.settings_file();
    settings::save(&path, &settings)?;
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub fn get_profile(state: State<'_, AppState>, game_id: String) -> AppResult<GameProfile> {
    if state.registry.game(&game_id).is_none() {
        return Err(AppError::UnknownGame(game_id));
    }
    profile::load(&state.paths.profile_file(&game_id), &game_id)
}

#[tauri::command(rename_all = "camelCase")]
pub fn save_profile(
    state: State<'_, AppState>,
    game_id: String,
    profile: GameProfile,
) -> AppResult<()> {
    if state.registry.game(&game_id).is_none() {
        return Err(AppError::UnknownGame(game_id));
    }
    profile::save(&state.paths.profile_file(&game_id), &profile)
}

#[tauri::command]
pub fn open_log_folder(state: State<'_, AppState>) -> AppResult<()> {
    let path = state.paths.logs().to_string_lossy().to_string();
    // Open via the OS shell. Use Windows `explorer.exe` directly to avoid the
    // pulse of the shell plugin's permission system.
    #[cfg(windows)]
    {
        use std::process::Command;
        Command::new("explorer")
            .arg(&path)
            .spawn()
            .map_err(|e| AppError::Other(format!("open explorer: {e}")))?;
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub async fn attach(
    state: State<'_, AppState>,
    app: AppHandle,
    game_id: String,
) -> AppResult<AttachInfo> {
    // --- Snapshot what we need from State (synchronous, fast) ---
    let game: &dyn Game = state
        .registry
        .game(&game_id)
        .ok_or_else(|| AppError::UnknownGame(game_id.clone()))?;

    state.watcher.lock().pause();
    emit_attach(&app, attach::attaching(&game_id), &game_id, None);

    // --- Find the game process (toolhelp32; sub-millisecond) ---
    let candidates: Vec<&str> = game.process_names().to_vec();
    let (pid, process_name) = match ProcessSnapshot::capture() {
        Ok(snap) => match snap.find_by_candidates(&candidates) {
            Some(info) => (info.pid, info.name.clone()),
            None => {
                let msg = format!("process not found: {}", candidates.join(" | "));
                let payload = AttachStatePayload::Error {
                    message: msg.clone(),
                };
                emit_attach(&app, payload, &game_id, Some("attach_failed"));
                state.watcher.lock().resume();
                return Err(AppError::Core(openforge_core::Error::ProcessNotFound(
                    candidates.join(" | "),
                )));
            }
        },
        Err(e) => {
            let payload = AttachStatePayload::Error {
                message: e.to_string(),
            };
            emit_attach(&app, payload, &game_id, Some("attach_failed"));
            state.watcher.lock().resume();
            return Err(AppError::Core(e));
        }
    };
    let _ = process_name;

    // --- Resolve the per-game DLL on disk ---
    let dll_file_name = game.dll_file_name();
    if dll_file_name.is_empty() {
        let msg = format!(
            "game `{}` declares no `dll_file_name` in its manifest; the trainer requires one",
            game.id()
        );
        let payload = AttachStatePayload::Error {
            message: msg.clone(),
        };
        emit_attach(&app, payload, &game_id, Some("attach_failed"));
        state.watcher.lock().resume();
        return Err(AppError::Other(msg));
    }
    let dll_path = match resolve_dll_path(dll_file_name) {
        Ok(p) => p,
        Err(e) => {
            let payload = AttachStatePayload::Error {
                message: format!("DLL not found: {e}"),
            };
            emit_attach(&app, payload, &game_id, Some("attach_failed"));
            state.watcher.lock().resume();
            return Err(AppError::Host(e));
        }
    };

    // --- Inject DLL + open the IPC session ---
    // `Ue5Session::attach_pid` injects (idempotently) and performs the Hello
    // handshake. The session's `Drop` is what signals the DLL to auto-restore
    // any in-flight code patches on this connection.
    let session = match tauri::async_runtime::spawn_blocking({
        let dll_path = dll_path.clone();
        move || Ue5Session::attach_pid(pid, &dll_path)
    })
    .await
    {
        Ok(Ok(s)) => Arc::new(s),
        Ok(Err(e)) => {
            let payload = AttachStatePayload::Error {
                message: format!("DLL session: {e}"),
            };
            emit_attach(&app, payload, &game_id, Some("attach_failed"));
            state.watcher.lock().resume();
            return Err(AppError::Host(e));
        }
        Err(e) => {
            let payload = AttachStatePayload::Error {
                message: format!("attach worker panicked: {e}"),
            };
            emit_attach(&app, payload, &game_id, Some("attach_failed"));
            state.watcher.lock().resume();
            return Err(AppError::Other(format!("attach worker panicked: {e}")));
        }
    };

    let detected_version = game
        .supported_versions()
        .first()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "unknown".into());

    if !game
        .supported_versions()
        .iter()
        .any(|v| *v == detected_version)
    {
        let _ = app.emit(
            "version_warning",
            VersionWarningEvent {
                game_id: game_id.clone(),
                detected_version: detected_version.clone(),
                supported: game
                    .supported_versions()
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            },
        );
    }

    // --- Resolve signatures on a blocking worker thread ---
    // Each resolve sends synchronous IPC frames to the DLL. In-DLL heap
    // scans are ~10× faster than the old host-side scans (no RPM overhead),
    // but resolves still take tens of milliseconds each, so we offload to
    // keep the Tauri IPC + WebView2 bridge responsive.
    let features: Vec<&'static dyn Feature> = state.registry.features_for(&game_id).to_vec();
    let total = features.len();

    // Load the profile so we can feed the resolved-address cache into the
    // resolver. A missing or corrupt profile must NOT block attach — we just
    // start with an empty cache and warn.
    let profile_path = state.paths.profile_file(&game_id);
    let mut profile = match profile::load(&profile_path, &game_id) {
        Ok(p) => p,
        Err(e) => {
            warn!(game = %game_id, error = %e, "profile load failed; continuing with empty cache");
            GameProfile::empty(&game_id)
        }
    };
    let cache_for_task = profile.feature_address_cache.clone();

    let session_for_task = Arc::clone(&session);
    let app_for_task = app.clone();
    let game_id_for_task = game_id.clone();
    let detected_version_for_task = detected_version.clone();

    let attached = tauri::async_runtime::spawn_blocking(move || {
        let game_id_for_cb = game_id_for_task.clone();
        let app_for_cb = app_for_task.clone();
        Attached::resolve_all(
            session_for_task,
            &game_id_for_task,
            &detected_version_for_task,
            &features,
            &cache_for_task,
            move |done, total, feat, status, err| {
                emit_attach(
                    &app_for_cb,
                    attach::resolving(&game_id_for_cb, done, total),
                    &game_id_for_cb,
                    None,
                );
                let _ = app_for_cb.emit(
                    "feature_resolved",
                    FeatureResolvedEvent {
                        game_id: game_id_for_cb.clone(),
                        feature_id: feat.to_string(),
                        ok: status == attach::ResolutionStatus::Resolved,
                        error: err.map(str::to_string),
                        status: Some(status.as_str().to_string()),
                    },
                );
            },
        )
    })
    .await
    .map_err(|e| AppError::Other(format!("attach worker panicked: {e}")))?;

    // --- Write the resolved-address cache back to the profile ---
    {
        let module_base = session.main_module_base();
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        for r in attached.resolutions.values() {
            if let Some(addr) = r.address {
                profile.feature_address_cache.insert(
                    r.feature_id.clone(),
                    CachedAddress {
                        address: addr as u64,
                        game_version: detected_version.clone(),
                        module_base,
                        captured_unix_secs: now_secs,
                    },
                );
            }
        }
        profile.last_seen_version = Some(detected_version.clone());
        if let Err(e) = profile::save(&profile_path, &profile) {
            warn!(game = %game_id, error = %e, "profile save failed");
        }
    }

    // --- Stash result + emit "attached" ---
    let per_feature: Vec<FeatureResolution> = attached
        .resolutions
        .values()
        .map(|r| FeatureResolution {
            feature_id: r.feature_id.clone(),
            ok: r.address.is_some(),
            error: r.error.clone(),
            status: r.status.as_str().to_string(),
        })
        .collect();

    *state.attached.write() = Some(attached);
    *state.active_game_id.write() = Some(game_id.clone());

    {
        let mut s = state.settings.write();
        s.last_active_game = Some(game_id.clone());
        let _ = settings::save(&state.paths.settings_file(), &s);
    }

    // --- Install the per-attach process-exit watcher ---
    {
        let pid_for_exit = pid;
        let app_for_exit = app.clone();
        let game_id_for_exit = game_id.clone();
        let exit_join = tauri::async_runtime::spawn(async move {
            let res = tauri::async_runtime::spawn_blocking(move || {
                openforge_core::wait_for_pid_exit(pid_for_exit)
            })
            .await;
            match res {
                Ok(Ok(())) => {
                    info!(game = %game_id_for_exit, pid = pid_for_exit, "attached process exited");
                }
                Ok(Err(e)) => {
                    warn!(
                        game = %game_id_for_exit,
                        pid = pid_for_exit,
                        error = %e,
                        "exit watcher: handle open failed, assuming process exited"
                    );
                }
                Err(e) => {
                    warn!(game = %game_id_for_exit, error = %e, "exit watcher task aborted");
                    return;
                }
            }
            let st = app_for_exit.state::<AppState>();
            let same_game = st
                .attached
                .read()
                .as_ref()
                .map(|a| a.game_id == game_id_for_exit)
                .unwrap_or(false);
            if same_game {
                do_detach(&st, &app_for_exit, Some("process_exit"));
            }
        });
        if let Some(att) = state.attached.read().as_ref() {
            att.set_exit_watch(exit_join);
        }
    }

    let _ = total;
    emit_attach(
        &app,
        attach::attached(&game_id, pid, &detected_version),
        &game_id,
        None,
    );

    info!(game = %game_id, pid, "attached");

    // --- Spawn background retry for Pending features --------------
    //
    // Features that need a live UE5 UObject (e.g. lock_health,
    // unlock_all_skills) fail their initial resolve when the game is on the
    // main menu. We don't surface those as Failed — they're Pending, and a
    // periodic retry promotes them to Resolved once the player enters a
    // level. The task self-terminates when no Pending entries remain or the
    // session is detached.
    spawn_pending_retry_task(app.clone(), game_id.clone(), state.registry);

    // --- Read-probe loops for non-freeze reflection features -----
    //
    // Mods (Super Jump, Super Speed, Low Gravity) read the live pawn's
    // CharacterMovement values through a deref chain. When the player is
    // in a vehicle (or any non-character pawn), the chain fails mid-walk.
    // Without a background probe the input fields would just sit empty —
    // looking broken. The probe re-reads every 2s, surfaces "waiting"
    // through the same UI badge the freeze loop uses, and auto-pushes the
    // value back into the input field the moment the chain works again.
    {
        let attached_guard = state.attached.read();
        if let Some(att) = attached_guard.as_ref() {
            let session = Arc::clone(&att.session);
            // Snapshot which features get probes so we can drop the lock
            // before spawning. Only reflection features that resolved go
            // here; SetProgressTags has no readable value (action trigger)
            // and freeze features are covered by their own loop.
            let to_probe: Vec<(&'static dyn openforge_runtime::Feature, usize)> = state
                .registry
                .features_for(&game_id)
                .iter()
                .filter_map(|f| {
                    if !matches!(
                        f.write_strategy(),
                        openforge_runtime::WriteStrategyKind::OneShot
                    ) {
                        return None;
                    }
                    // Skip expensive-read features (e.g. SetProgressTags
                    // calls UFunction per entry on the game thread —
                    // hammering it at 2s cadence visibly stalls the
                    // game frame).
                    if !f.supports_probe() {
                        return None;
                    }
                    let addr = att.feature_addr(f.id())?;
                    Some((*f, addr))
                })
                .collect();
            drop(attached_guard);
            for (feature, addr) in to_probe {
                let h = freeze::spawn_read_probe_task(
                    app.clone(),
                    Arc::clone(&session),
                    feature,
                    addr,
                    game_id.clone(),
                );
                state
                    .read_probe_handles
                    .lock()
                    .insert((game_id.clone(), feature.id().to_string()), h);
            }
        }
    }

    Ok(AttachInfo {
        pid,
        detected_version,
        per_feature_resolution: per_feature,
    })
}

/// Spawn a background task that retries `Pending` feature resolutions
/// every few seconds. Exits when there are no more `Pending` entries (all
/// promoted to `Resolved` or `Failed`) or the session changes / detaches.
fn spawn_pending_retry_task(
    app: AppHandle,
    game_id: String,
    registry: &'static openforge_runtime::Registry,
) {
    // 3-second cadence: a level load takes ~5-15s; checking every 3s makes
    // pending features go live within one cadence of entering the world.
    // Lower bound is 1s (avoid hammering the DLL with FindUObject walks
    // during the main menu); upper bound is ~5s before users would notice
    // the delay.
    const RETRY_INTERVAL_SECS: u64 = 3;

    tauri::async_runtime::spawn(async move {
        let mut consecutive_empty_cycles = 0u32;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(RETRY_INTERVAL_SECS)).await;

            // Snapshot the pending list under the read lock; release before
            // the (possibly slow) per-feature resolve calls.
            let pending_ids: Vec<String> = {
                let st = app.state::<AppState>();
                let attached_guard = st.attached.read();
                let Some(att) = attached_guard.as_ref() else {
                    info!(game = %game_id, "retry task: detached; exiting");
                    return;
                };
                if att.game_id != game_id {
                    info!(
                        game = %game_id,
                        current = %att.game_id,
                        "retry task: game changed; exiting"
                    );
                    return;
                }
                att.resolutions
                    .values()
                    .filter(|r| r.status == attach::ResolutionStatus::Pending)
                    .map(|r| r.feature_id.clone())
                    .collect()
            };

            if pending_ids.is_empty() {
                consecutive_empty_cycles += 1;
                // Two empty cycles in a row → no work to do, exit cleanly.
                // We don't exit on first empty because a feature might
                // briefly transition (cached hit + immediate quick_check)
                // and we want to give the system one more cycle.
                if consecutive_empty_cycles >= 2 {
                    info!(game = %game_id, "retry task: no pending features; exiting");
                    return;
                }
                continue;
            }
            consecutive_empty_cycles = 0;

            // Capture session + module_base under the read lock again
            // (state could have changed; bail if so).
            let session_opt: Option<Arc<openforge_ue5_host::Ue5Session>> = {
                let st = app.state::<AppState>();
                let attached_guard = st.attached.read();
                attached_guard
                    .as_ref()
                    .filter(|att| att.game_id == game_id)
                    .map(|att| att.session.clone())
            };
            let Some(session) = session_opt else {
                info!(game = %game_id, "retry task: detached mid-cycle; exiting");
                return;
            };

            for fid in pending_ids {
                let Some(feature) = registry.feature(&game_id, &fid) else {
                    continue;
                };
                let session_for_resolve = Arc::clone(&session);
                let resolve_res = tauri::async_runtime::spawn_blocking(move || {
                    feature.resolve(session_for_resolve.as_ref())
                })
                .await;
                let Ok(resolve_result) = resolve_res else {
                    continue;
                };
                match resolve_result {
                    Ok(addr) => {
                        info!(
                            game = %game_id,
                            feature = %fid,
                            addr = format!("0x{addr:X}"),
                            "pending → resolved"
                        );
                        {
                            let st = app.state::<AppState>();
                            let mut attached_guard = st.attached.write();
                            if let Some(att) = attached_guard.as_mut()
                                && att.game_id == game_id
                            {
                                att.resolutions.insert(
                                    fid.clone(),
                                    attach::ResolvedFeature {
                                        feature_id: fid.clone(),
                                        address: Some(addr),
                                        error: None,
                                        status: attach::ResolutionStatus::Resolved,
                                    },
                                );
                            }
                        }
                        let _ = app.emit(
                            "feature_resolved",
                            FeatureResolvedEvent {
                                game_id: game_id.clone(),
                                feature_id: fid.clone(),
                                ok: true,
                                error: None,
                                status: Some("resolved".to_string()),
                            },
                        );
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        // Still pending? Stay quiet (next cycle retries).
                        // Transitioned to permanent Failed? Promote + emit.
                        let now_status = if attach::is_transient_resolve_error(&msg) {
                            attach::ResolutionStatus::Pending
                        } else {
                            attach::ResolutionStatus::Failed
                        };
                        if now_status == attach::ResolutionStatus::Failed {
                            warn!(
                                game = %game_id,
                                feature = %fid,
                                error = %msg,
                                "pending → failed (no longer transient)"
                            );
                            {
                                let st = app.state::<AppState>();
                                let mut attached_guard = st.attached.write();
                                if let Some(att) = attached_guard.as_mut()
                                    && att.game_id == game_id
                                {
                                    att.resolutions.insert(
                                        fid.clone(),
                                        attach::ResolvedFeature {
                                            feature_id: fid.clone(),
                                            address: None,
                                            error: Some(msg.clone()),
                                            status: now_status,
                                        },
                                    );
                                }
                            }
                            let _ = app.emit(
                                "feature_resolved",
                                FeatureResolvedEvent {
                                    game_id: game_id.clone(),
                                    feature_id: fid.clone(),
                                    ok: false,
                                    error: Some(msg),
                                    status: Some("failed".to_string()),
                                },
                            );
                        }
                    }
                }
            }
        }
    });
}

#[tauri::command]
pub fn detach(state: State<'_, AppState>, app: AppHandle) -> AppResult<()> {
    do_detach(&state, &app, Some("detach"));
    Ok(())
}

/// Shared teardown path used by both user-initiated detach and the
/// process-exit watcher.
///
/// Single-channel architecture: code-patch auto-restore happens **inside the DLL**
/// when the pipe disconnects (see `ConnState::Drop` in
/// `crates/batman-lod-dll/src/ops/state.rs`). Dropping the `Ue5Session`
/// closes the pipe; the DLL then writes original bytes back. The host no
/// longer maintains an `applied_patches` set.
///
/// Steps, in order:
/// 1. Abort all freeze loops.
/// 2. Take `Attached` out of the slot.
/// 3. Abort the exit-watcher so it doesn't fire on the now-stale state.
/// 4. Drop `Attached` — this drops `Ue5Session`, which closes the pipe,
///    which triggers the DLL's auto-restore for any tracked patches.
/// 5. Emit the Idle transition and resume the process watcher.
pub(crate) fn do_detach(state: &AppState, app: &AppHandle, reason: Option<&str>) {
    {
        let mut handles = state.freeze_handles.lock();
        for (_, h) in handles.drain() {
            h.abort();
        }
    }
    {
        // Stop all read-probe loops — without this, they'd keep firing
        // read() against a dropped Ue5Session and emit "waiting" forever.
        let mut handles = state.read_probe_handles.lock();
        for (_, h) in handles.drain() {
            h.abort();
        }
    }

    let attached_opt = state.attached.write().take();
    let game_id = if let Some(att) = attached_opt {
        if let Some(h) = att.take_exit_watch() {
            h.abort();
        }
        let game_id = att.game_id.clone();
        info!(game = %game_id, reason = ?reason, "detaching");
        // Drop runs here when `att` goes out of scope below; the
        // `Ue5Session` Drop sends `Disconnect`, the DLL's `ConnState`
        // restores patches.
        drop(att);
        game_id
    } else {
        String::new()
    };

    if game_id.is_empty() {
        emit_attach(app, AttachStatePayload::Idle, "", reason);
    } else {
        emit_attach(app, AttachStatePayload::Idle, &game_id, reason);
    }
    state.watcher.lock().resume();
}

#[tauri::command(rename_all = "camelCase")]
pub fn read_feature(
    state: State<'_, AppState>,
    game_id: String,
    feature_id: String,
) -> AppResult<Value> {
    let attached_guard = state.attached.read();
    let attached = attached_guard.as_ref().ok_or(AppError::NotAttached)?;
    if attached.game_id != game_id {
        return Err(AppError::NotAttached);
    }
    let feature = resolve_feature(state.registry, &game_id, &feature_id)?;
    let addr = attached
        .feature_addr(&feature_id)
        .ok_or_else(|| AppError::UnknownFeature {
            game: game_id.clone(),
            feature: feature_id.clone(),
        })?;
    Ok(feature.read(attached.session.as_ref(), addr)?)
}

#[tauri::command(rename_all = "camelCase")]
pub fn read_features(
    state: State<'_, AppState>,
    app: AppHandle,
    game_id: String,
    feature_ids: Vec<String>,
) -> AppResult<Vec<(String, Value)>> {
    let attached_guard = state.attached.read();
    let attached = attached_guard.as_ref().ok_or(AppError::NotAttached)?;
    if attached.game_id != game_id {
        return Err(AppError::NotAttached);
    }
    let mut out = Vec::with_capacity(feature_ids.len());
    for fid in feature_ids {
        let Some(addr) = attached.feature_addr(&fid) else {
            continue;
        };
        let Some(feature) = state.registry.feature(&game_id, &fid) else {
            continue;
        };
        match feature.read(attached.session.as_ref(), addr) {
            Ok(v) => {
                out.push((fid.clone(), v));
                // Eagerly seed the runtime-state store as "active". Without
                // this, the FE wouldn't know the feature is healthy until
                // the probe loop's first tick (4s later) — the input field
                // looks ambiguous in the meantime. Emitting the same event
                // the probe loop uses keeps a single source of truth.
                let _ = app.emit(
                    "feature_freeze_state",
                    crate::types::FeatureFreezeStateEvent {
                        game_id: game_id.clone(),
                        feature_id: fid,
                        state: "active".to_string(),
                        hint: None,
                    },
                );
            }
            Err(e) => {
                let msg = e.to_string();
                // Transient reads (e.g. player currently in a vehicle, so
                // PawnPrivate doesn't point at a TtCharacter with
                // CharacterMovement) are expected and self-heal. Demote
                // those to INFO so the warn level stays reserved for real
                // bugs.
                if attach::is_transient_resolve_error(&msg) {
                    tracing::info!(
                        game = %game_id,
                        feature = %fid,
                        error = %msg,
                        "read failed (transient — will retry on next access)"
                    );
                    // Surface the "waiting" pill immediately rather than
                    // making the user wait for the probe loop's first
                    // tick. The probe will keep this state updated.
                    let _ = app.emit(
                        "feature_freeze_state",
                        crate::types::FeatureFreezeStateEvent {
                            game_id: game_id.clone(),
                            feature_id: fid,
                            state: "waiting".to_string(),
                            hint: Some(
                                "Currently unreachable — try entering a level or switching back to a character."
                                    .to_string(),
                            ),
                        },
                    );
                } else {
                    warn!(
                        game = %game_id,
                        feature = %fid,
                        error = %msg,
                        "read failed"
                    );
                }
            }
        }
    }
    Ok(out)
}

/// Compute a feature's optional human-readable status line (e.g.
/// "20 / 30 skills unlocked"). Returns `None` when the feature doesn't
/// override `status_text` (most don't). Called on-demand by the UI:
/// once on attach + after each successful write of a SetProgressTags
/// feature. Not on a poll loop — this can do N IPC round-trips for
/// bulk-tag features.
#[tauri::command(rename_all = "camelCase")]
pub fn feature_status_text(
    state: State<'_, AppState>,
    game_id: String,
    feature_id: String,
) -> AppResult<Option<String>> {
    let attached_guard = state.attached.read();
    let attached = attached_guard.as_ref().ok_or(AppError::NotAttached)?;
    if attached.game_id != game_id {
        return Err(AppError::NotAttached);
    }
    let feature = resolve_feature(state.registry, &game_id, &feature_id)?;
    let addr = attached
        .feature_addr(&feature_id)
        .ok_or_else(|| AppError::UnknownFeature {
            game: game_id.clone(),
            feature: feature_id.clone(),
        })?;
    feature
        .status_text(attached.session.as_ref(), addr)
        .map_err(AppError::Runtime)
}

#[tauri::command(rename_all = "camelCase")]
pub fn write_feature(
    state: State<'_, AppState>,
    game_id: String,
    feature_id: String,
    value: Value,
) -> AppResult<Value> {
    let attached_guard = state.attached.read();
    let attached = attached_guard.as_ref().ok_or(AppError::NotAttached)?;
    if attached.game_id != game_id {
        return Err(AppError::NotAttached);
    }
    let feature = resolve_feature(state.registry, &game_id, &feature_id)?;
    let addr = attached
        .feature_addr(&feature_id)
        .ok_or_else(|| AppError::UnknownFeature {
            game: game_id.clone(),
            feature: feature_id.clone(),
        })?;
    if let Err(e) = feature.write(attached.session.as_ref(), addr, value) {
        tracing::error!(
            game = %game_id,
            feature = %feature_id,
            addr = format!("0x{addr:X}"),
            error = %e,
            "write_feature failed"
        );
        return Err(e.into());
    }
    feature.read(attached.session.as_ref(), addr).map_err(|e| {
        tracing::error!(
            game = %game_id,
            feature = %feature_id,
            addr = format!("0x{addr:X}"),
            error = %e,
            "write_feature read-back failed"
        );
        e.into()
    })
}

#[tauri::command(rename_all = "camelCase")]
pub async fn retry_resolve(
    state: State<'_, AppState>,
    app: AppHandle,
    game_id: String,
    feature_id: String,
) -> AppResult<FeatureResolution> {
    // Snapshot the Arc<Ue5Session> + cache inputs under a short read lock;
    // release before doing the (potentially long) scan.
    let (session, module_base, detected_version) = {
        let attached_guard = state.attached.read();
        let attached = attached_guard.as_ref().ok_or(AppError::NotAttached)?;
        if attached.game_id != game_id {
            return Err(AppError::NotAttached);
        }
        (
            Arc::clone(&attached.session),
            attached.session.main_module_base(),
            attached.detected_version.clone(),
        )
    };

    let feature = resolve_feature(state.registry, &game_id, &feature_id)?;

    let resolved = tauri::async_runtime::spawn_blocking(move || feature.resolve(session.as_ref()))
        .await
        .map_err(|e| AppError::Other(format!("retry worker panicked: {e}")))?;

    let (ok, error_str, status_out) = match resolved {
        Ok(addr) => {
            info!(
                game = %game_id,
                feature = %feature_id,
                addr = format!("0x{addr:X}"),
                "retry resolve ok"
            );
            // Update the live resolutions map.
            {
                let mut attached_guard = state.attached.write();
                if let Some(att) = attached_guard.as_mut()
                    && att.game_id == game_id
                {
                    att.resolutions.insert(
                        feature_id.clone(),
                        attach::ResolvedFeature {
                            feature_id: feature_id.clone(),
                            address: Some(addr),
                            error: None,
                            status: attach::ResolutionStatus::Resolved,
                        },
                    );
                }
            }
            // Persist to profile cache so the next attach short-circuits.
            let profile_path = state.paths.profile_file(&game_id);
            if let Ok(mut profile) = profile::load(&profile_path, &game_id) {
                let now_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                profile.feature_address_cache.insert(
                    feature_id.clone(),
                    CachedAddress {
                        address: addr as u64,
                        game_version: detected_version,
                        module_base,
                        captured_unix_secs: now_secs,
                    },
                );
                if let Err(e) = profile::save(&profile_path, &profile) {
                    warn!(game = %game_id, error = %e, "profile save after retry failed");
                }
            }
            (true, None, attach::ResolutionStatus::Resolved)
        }
        Err(e) => {
            let msg = e.to_string();
            let status = if attach::is_transient_resolve_error(&msg) {
                attach::ResolutionStatus::Pending
            } else {
                attach::ResolutionStatus::Failed
            };
            info!(
                game = %game_id,
                feature = %feature_id,
                error = %msg,
                status = status.as_str(),
                "retry resolve outcome"
            );
            {
                let mut attached_guard = state.attached.write();
                if let Some(att) = attached_guard.as_mut()
                    && att.game_id == game_id
                {
                    att.resolutions.insert(
                        feature_id.clone(),
                        attach::ResolvedFeature {
                            feature_id: feature_id.clone(),
                            address: None,
                            error: Some(msg.clone()),
                            status,
                        },
                    );
                }
            }
            (false, Some(msg), status)
        }
    };

    let _ = app.emit(
        "feature_resolved",
        FeatureResolvedEvent {
            game_id: game_id.clone(),
            feature_id: feature_id.clone(),
            ok,
            error: error_str.clone(),
            status: Some(status_out.as_str().to_string()),
        },
    );

    Ok(FeatureResolution {
        feature_id,
        ok,
        error: error_str,
        status: status_out.as_str().to_string(),
    })
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_freeze(
    state: State<'_, AppState>,
    app: AppHandle,
    game_id: String,
    feature_id: String,
    frozen: bool,
) -> AppResult<()> {
    let key = (game_id.clone(), feature_id.clone());

    if !frozen {
        if let Some(h) = state.freeze_handles.lock().remove(&key) {
            h.abort();
        }
        // Restore-to-default: every freeze feature snapshots its primary +
        // companion bytes at attach-time. When the user toggles the freeze
        // off we write those snapshotted bytes back, so "off" reads as
        // "back to stock" rather than "stuck at whatever was last applied".
        // Best-effort — missing snapshot or write failure both no-op.
        let attached_guard = state.attached.read();
        if let Some(attached) = attached_guard.as_ref()
            && attached.game_id == game_id
        {
            let snapshot = attached.feature_snapshots.lock().get(&feature_id).cloned();
            if let Some(bytes) = snapshot
                && let Ok(feature) = resolve_feature(state.registry, &game_id, &feature_id)
                && let Some(addr) = attached.feature_addr(&feature_id)
                && let Err(e) = feature.restore(attached.session.as_ref(), addr, &bytes)
            {
                tracing::warn!(
                    game = %game_id,
                    feature = %feature_id,
                    error = %e,
                    "set_freeze(false): restore-to-default failed"
                );
            }
        }
        return Ok(());
    }

    let attached_guard = state.attached.read();
    let attached = attached_guard.as_ref().ok_or(AppError::NotAttached)?;
    if attached.game_id != game_id {
        return Err(AppError::NotAttached);
    }
    let feature = resolve_feature(state.registry, &game_id, &feature_id)?;
    let addr = attached
        .feature_addr(&feature_id)
        .ok_or_else(|| AppError::UnknownFeature {
            game: game_id.clone(),
            feature: feature_id.clone(),
        })?;
    // `freeze_value` from the TOML wins over "capture current". Used by
    // Infinite Health (always freeze `bCanBeDamaged` at false) and similar
    // lock-at-constant features where reading current would defeat the
    // lock (e.g. capturing `true` for bCanBeDamaged is a no-op freeze).
    let target = match feature.freeze_value() {
        Some(f) => Value::F64(f).coerce(feature.kind())?,
        None => feature.read(attached.session.as_ref(), addr)?,
    };
    let session = Arc::clone(&attached.session);
    drop(attached_guard);

    let interval_ms = feature.freeze_interval_ms();
    tracing::info!(
        game = %game_id,
        feature = %feature_id,
        addr = format!("0x{addr:X}"),
        interval_ms,
        "set_freeze: spawning freeze loop"
    );
    let h = freeze::spawn_freeze_task(app, session, feature, addr, target, game_id, interval_ms);
    state.freeze_handles.lock().insert(key, h);
    Ok(())
}

#[tauri::command(rename_all = "camelCase")]
pub fn set_code_patch(
    state: State<'_, AppState>,
    game_id: String,
    feature_id: String,
    applied: bool,
) -> AppResult<()> {
    let attached_guard = state.attached.read();
    let attached = attached_guard.as_ref().ok_or(AppError::NotAttached)?;
    if attached.game_id != game_id {
        return Err(AppError::NotAttached);
    }
    let feature = resolve_feature(state.registry, &game_id, &feature_id)?;
    let addr = attached
        .feature_addr(&feature_id)
        .ok_or_else(|| AppError::UnknownFeature {
            game: game_id.clone(),
            feature: feature_id.clone(),
        })?;
    // The DLL tracks the patch in its per-pipe `ConnState`. We don't keep a
    // host-side mirror of the set anymore — the DLL's auto-restore happens
    // server-side on `Drop`.
    feature.write(attached.session.as_ref(), addr, Value::Bool(applied))?;
    Ok(())
}

fn resolve_feature(
    registry: &openforge_runtime::Registry,
    game_id: &str,
    feature_id: &str,
) -> AppResult<&'static dyn Feature> {
    registry
        .feature(game_id, feature_id)
        .ok_or_else(|| AppError::UnknownFeature {
            game: game_id.to_string(),
            feature: feature_id.to_string(),
        })
}

fn emit_attach(app: &AppHandle, state: AttachStatePayload, game_id: &str, reason: Option<&str>) {
    let _ = app.emit(
        "attach_status",
        AttachStatusEvent {
            state,
            game_id: if game_id.is_empty() {
                None
            } else {
                Some(game_id.to_string())
            },
            reason: reason.map(str::to_string),
        },
    );
}
