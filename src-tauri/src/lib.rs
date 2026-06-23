mod commands;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
    .setup(|app| {
      if cfg!(debug_assertions) {
        app.handle().plugin(
          tauri_plugin_log::Builder::default()
            .level(log::LevelFilter::Info)
            .build(),
        )?;
      }
      Ok(())
    })
    .invoke_handler(tauri::generate_handler![
        commands::fido_device_info,
        commands::fido_pin_retries,
        commands::iroh_host_start,
        commands::iroh_host_status,
        commands::iroh_client_connect,
        commands::iroh_client_send_input,
    ])
    .manage(commands::AppState::default())
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
