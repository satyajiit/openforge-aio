#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::Ordering;

use openforge_app_lib::{
    commands, keybinds, keybinds::KeybindStore, log_setup, paths::AppPaths, settings,
    state::AppState,
};
use tauri::{Manager, WindowEvent};

fn main() {
    openforge_bundle::ensure_linked();

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
            if let WindowEvent::Focused(focused) = event
                && let Some(state) = window.try_state::<AppState>()
            {
                state.window_focused.store(*focused, Ordering::Relaxed);
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running OpenForge");
}
