mod commands;
mod error;
mod installer;
mod models;
mod platform;
mod releases;
mod state;

use state::{AppState, InstallerPaths};
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let paths =
                InstallerPaths::new(app.path().app_data_dir()?, app.path().app_cache_dir()?);
            let state = AppState::new(paths)?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::resize_window,
            commands::list_releases,
            commands::scan_installations,
            commands::run_preflight,
            commands::install_build_tools,
            commands::start_install,
            commands::cancel_operation,
            commands::launch_installation,
            commands::reveal_installation,
            commands::restore_previous,
            commands::uninstall_managed,
            commands::clean_cache,
            commands::open_external,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Aseprite Installer");
}
