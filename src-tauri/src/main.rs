// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;

use commands::AppState;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_log::Builder::default().build())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::fido_device_info,
            commands::fido_pin_retries,
            commands::titan_derive_identity,
            commands::is_daemon_mode,
            commands::set_webcodecs_available,
            commands::is_webcodecs_available,
            commands::get_encoder_config,
            commands::set_encoder_config,
            commands::detect_available_encoders,
            commands::host_registration_status,
            commands::titan_register_host,
            commands::host_unregister,
            commands::iroh_host_start,
            commands::iroh_host_stop,
            commands::iroh_host_status,
            commands::iroh_client_connect,
            commands::iroh_client_disconnect,
            commands::iroh_client_send_input,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
