mod commands;
mod error;
#[cfg(target_os = "macos")]
mod installer;
#[cfg(any(target_os = "linux", target_os = "windows"))]
#[path = "installer_portable.rs"]
mod installer;
mod models;
mod platform;
#[cfg(any(target_os = "linux", target_os = "windows", test))]
mod portable_transaction;
mod releases;
mod state;
mod upstream;

use state::{AppState, InstallerPaths};
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let paths =
                InstallerPaths::new(app.path().app_data_dir()?, app.path().app_cache_dir()?);
            let state = AppState::new(paths)?;
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            match std::fs::symlink_metadata(&state.paths.transaction_file) {
                Ok(_) => {
                    // Recover under the same interprocess lock used by live operations
                    // before discovery or launch commands can observe a half transaction.
                    match state.begin_operation() {
                        Ok(_) => state.finish_operation(),
                        Err(error) => {
                            eprintln!("Aseprite Installer opened in recovery-safe mode: {error}");
                            state.set_recovery_error(Some(error));
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    state.set_recovery_error(Some(crate::error::InstallerError::with_detail(
                        "recoveryInspect",
                        "The interrupted-installation journal could not be inspected safely.",
                        error.to_string(),
                    )));
                }
            }
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::resize_window,
            commands::get_platform_info,
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
            commands::get_recovery_status,
            commands::retry_recovery,
            commands::open_external,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Aseprite Installer");
}
