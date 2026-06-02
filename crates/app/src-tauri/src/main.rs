#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::Ordering;

use openforge_app_lib::{
    commands, keybinds, keybinds::KeybindStore, log_setup, paths::AppPaths, settings,
    state::AppState,
};
use tauri::{Manager, WindowEvent};

fn main() {
    openforge_bundle::ensure_linked();
    // Force-link every engine backend so its `register_engine!` inventory item
    // survives in the final binary, then assert the registry actually resolved
    // both shipped engines. Without the explicit force-link the linker
    // dead-strips a host crate the app no longer references by symbol (this bit
    // Glacier after the Session-enum collapse stopped touching `glacier-host`),
    // and `backend_for(kind)` would return `None` only at attach time — a
    // silent runtime failure. Failing loudly at startup turns that into an
    // immediate, obvious crash instead.
    openforge_ue5_host::backend::ensure_linked();
    openforge_glacier_host::backend::ensure_linked();
    {
        use openforge_runtime::manifest::EngineKind;
        let registered = openforge_engine::registered_kinds();
        for kind in [EngineKind::Ue5, EngineKind::Glacier2] {
            assert!(
                registered.contains(&kind),
                "engine backend for {kind:?} is not registered — its host crate's \
                 register_engine! was dead-stripped from the binary (linkage bug)"
            );
        }
    }

    let paths = AppPaths::create().expect("failed to create app paths");
    let _log_guard = log_setup::init(paths.logs()).expect("logging init");
    let settings = settings::load(&paths.settings_file()).unwrap_or_default();
    let keybinds_path = paths.config_dir.join("keybinds.json");
    let keybinds = KeybindStore::load(keybinds_path);
    let state = AppState::new(paths, settings, keybinds);

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    keybinds::on_shortcut(app, shortcut, event.state());
                })
                .build(),
        )
        .manage(state)
        .setup(|app| {
            // Open DevTools automatically when the `devtools-enabled` feature
            // is on (dev convenience). Production release builds ship without
            // the feature, so the inspector is unreachable from the running
            // app — the FE-side hardening hook blocks F12 and friends too.
            #[cfg(feature = "devtools-enabled")]
            {
                if let Some(window) = app.get_webview_window("main") {
                    window.open_devtools();
                }
            }
            // Install every persisted global hotkey ONCE for the
            // process lifetime. Dispatch is attach-aware, so we don't
            // need to re-register at attach time — and re-registering
            // is what previously wedged the plugin's internal map.
            keybinds::register_at_startup(&app.handle().clone());
            Ok(())
        })
        .on_window_event(|window, event| {
            match event {
                WindowEvent::Focused(focused) => {
                    if let Some(state) = window.try_state::<AppState>() {
                        state.window_focused.store(*focused, Ordering::Relaxed);
                    }
                }
                WindowEvent::CloseRequested { .. } => {
                    // Run the same teardown the Detach CTA does, so the
                    // currently-running Lua polling task gets a clean cancel,
                    // a final `lua_script_status` event is emitted, and the
                    // pipe disconnects cleanly (DLL `ConnState::Drop` then
                    // tears down the per-connection `LuaRuntime`, joining
                    // its worker + delayed-timer + key-poll threads). Without
                    // this hook, the Drop chain still runs once the process
                    // exits — but the polling task gets killed mid-iteration
                    // and no Idle event fires. This makes shutdown observable.
                    if let Some(state) = window.try_state::<AppState>() {
                        commands::do_detach(
                            &state,
                            &window.app_handle().clone(),
                            Some("window_close"),
                        );
                    }
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_games,
            commands::list_features,
            commands::start_watcher,
            commands::stop_watcher,
            commands::get_process_state,
            commands::preflight,
            commands::attach,
            commands::detach,
            commands::read_feature,
            commands::read_features,
            commands::write_feature,
            commands::feature_status_text,
            commands::set_freeze,
            commands::retry_resolve,
            commands::set_code_patch,
            commands::feature_options,
            commands::invoke_feature_action,
            commands::is_elevated,
            commands::relaunch_as_admin,
            commands::get_settings,
            commands::set_settings,
            commands::get_profile,
            commands::save_profile,
            commands::open_log_folder,
            commands::list_keybinds,
            commands::set_keybind,
            commands::clear_keybind,
            commands::check_keybind_conflict,
            commands::list_lua_scripts,
            commands::read_lua_script,
            commands::save_user_lua_script,
            commands::delete_user_lua_script,
            commands::validate_lua_script,
            commands::refresh_community_lua_index,
            commands::install_community_lua_script,
            commands::run_lua_script,
            commands::stop_lua_script,
        ])
        .run(tauri::generate_context!())
        .expect("error while running OpenForge");
}
