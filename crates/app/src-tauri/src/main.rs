#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::Ordering;

use openforge_app_lib::{commands, log_setup, paths::AppPaths, settings, state::AppState};
use tauri::{Manager, WindowEvent};

fn main() {
    openforge_bundle::ensure_linked();

    let paths = AppPaths::create().expect("failed to create app paths");
    let _log_guard = log_setup::init(paths.logs()).expect("logging init");
    let settings = settings::load(&paths.settings_file()).unwrap_or_default();
    let state = AppState::new(paths, settings);

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .manage(state)
        .setup(|app| {
            // Open DevTools automatically in debug builds so we can inspect
            // the FE without right-click menus. Release builds skip this and
            // the user can still toggle via F12 if needed.
            #[cfg(debug_assertions)]
            {
                if let Some(window) = app.get_webview_window("main") {
                    window.open_devtools();
                }
            }
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running OpenForge");
}
