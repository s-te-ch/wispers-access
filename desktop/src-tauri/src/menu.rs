//! The macOS menu bar.

use tauri::menu::{Menu, MenuItemKind, PredefinedMenuItem};

use crate::autostart;

pub fn add(app: &tauri::App) -> tauri::Result<()> {
    let handle = app.handle();
    let menu = Menu::default(handle)?;
    if let Some(MenuItemKind::Submenu(app_menu)) = menu.items()?.first() {
        app_menu.insert(&autostart::menu_item(handle)?, 2)?;
        app_menu.insert(&PredefinedMenuItem::separator(handle)?, 3)?;
    }
    app.set_menu(menu)?;
    app.on_menu_event(autostart::on_menu_event);
    Ok(())
}
