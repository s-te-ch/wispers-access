//! Autostart at login.
//!
//! This is on by default but can be toggled, in the app menu on macOS, in
//! the tray menu elsewhere, and in the window where there is no tray. An
//! autostart at login starts without the window, since the point is running
//! the proxy, not showing a window (without a tray the app starts minimised).

use tauri::menu::{CheckMenuItem, MenuEvent};
use tauri::{AppHandle, Manager, Wry};
use tauri_plugin_autostart::ManagerExt;

/// The flag the login launch passes.
const HIDDEN_FLAG: &str = "--hidden";

/// Configure and build the autostart plugin.
pub fn plugin() -> tauri::plugin::TauriPlugin<Wry> {
    tauri_plugin_autostart::Builder::new()
        .arg(HIDDEN_FLAG)
        .build()
}

/// Turn autostart on if this is the first run. The user can turn this off, and
/// later runs take care not to override the user's choice by looking for marker
/// file that tells them the default has already been applied. Dev builds skip
/// seting autostart entirely.
pub fn enable_on_first_run(app: &AppHandle) -> anyhow::Result<()> {
    if cfg!(debug_assertions) {
        return Ok(());
    }
    let marker = app.path().app_data_dir()?.join("launch-at-login-defaulted");
    if marker.exists() {
        return Ok(());
    }
    app.autolaunch().enable()?;
    std::fs::write(&marker, "")?;
    Ok(())
}

/// True if this run was started hidden (i.e. as a login item).
pub fn launched_hidden() -> bool {
    std::env::args().any(|arg| arg == HIDDEN_FLAG)
}

/// Build and return the "Launch at Login" item, checked to match the setting.
pub fn menu_item(app: &AppHandle) -> tauri::Result<CheckMenuItem<Wry>> {
    let enabled = app.autolaunch().is_enabled().unwrap_or(false);
    let item =
        CheckMenuItem::with_id(app, MENU_ID, "Launch at Login", true, enabled, None::<&str>)?;
    app.manage(LaunchAtLoginItem(item.clone()));
    Ok(item)
}

const MENU_ID: &str = "launch-at-login";

struct LaunchAtLoginItem(CheckMenuItem<Wry>);

/// Handle a click in a menu, ours or not. If ours, change autolaunch accordingly.
pub fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    if event.id.as_ref() != MENU_ID {
        return;
    }
    let Some(item) = app.try_state::<LaunchAtLoginItem>() else {
        return;
    };
    let wanted = item.0.is_checked().unwrap_or(false);
    let manager = app.autolaunch();
    let result = if wanted {
        manager.enable()
    } else {
        manager.disable()
    };
    if let Err(e) = result {
        tracing::warn!(error = %e, "could not change launch at login");
        let _ = item.0.set_checked(!wanted);
    }
}

/// Restart the app, showing the window (i.e. not hidden). The updater calls
/// this, because if the user restarts the app to update the app, they expect
/// the window to come back. By default, if the app was originally started
/// hidden, a restart would make it come back up hidden as well.
#[tauri::command]
pub fn restart(app: AppHandle) {
    let mut env = app.env();
    env.args_os.retain(|arg| arg != HIDDEN_FLAG);
    app.cleanup_before_exit();
    tauri::process::restart(&env)
}

/// Whether the app launches at login, for the window to show where no menu
/// has the setting.
#[tauri::command]
pub fn launch_at_login(app: AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
pub fn set_launch_at_login(app: AppHandle, enabled: bool) -> Result<(), String> {
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    result.map_err(|e| e.to_string())
}
