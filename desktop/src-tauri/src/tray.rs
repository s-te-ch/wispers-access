//! The notification area icon, which holds the app while its window is
//! closed where there is no dock to do that.
//!
//! Linux desktops may have no tray at all, stock GNOME for one. There the
//! window is all there is: closing it quits.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

use crate::autostart;

const OPEN: &str = "open";
const QUIT: &str = "quit";

/// Whether there is a tray to put the icon in. Always on Windows; on Linux,
/// where the icon is a StatusNotifierItem, when something on the session bus
/// is there to show those.
#[cfg(target_os = "linux")]
pub fn available() -> bool {
    let watched = || -> zbus::Result<bool> {
        let bus = zbus::blocking::Connection::session()?;
        let dbus = zbus::blocking::fdo::DBusProxy::new(&bus)?;
        Ok(dbus.name_has_owner("org.kde.StatusNotifierWatcher".try_into()?)?)
    };
    watched().unwrap_or_else(|e| {
        tracing::warn!(error = %e, "could not look for a tray");
        false
    })
}

#[cfg(not(target_os = "linux"))]
pub fn available() -> bool {
    true
}

/// A click on the icon brings the window back (where the tray reports clicks;
/// Linux's don't, so there it is the menu's Open item). Its menu has the same, the
/// one setting, and the app's only way to quit.
pub fn add(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, OPEN, "Open Wispers Access", true, None::<&str>)?;
    let launch_at_login = autostart::menu_item(app.handle())?;
    let quit = MenuItem::with_id(app, QUIT, "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &launch_at_login,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    let mut tray = TrayIconBuilder::new()
        .tooltip("Wispers Access")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            OPEN => crate::show_window(app),
            QUIT => app.exit(0),
            _ => autostart::on_menu_event(app, event),
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::show_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}
