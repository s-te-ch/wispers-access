//! The native side of the desktop app.

mod activity;
mod autostart;
#[cfg(target_os = "macos")]
mod menu;
mod secrets;
mod shares;
#[cfg(not(target_os = "macos"))]
mod tray;

use std::sync::Arc;
use std::time::Duration;
use tauri::{Emitter, Manager};
use wispers_access_sdk as sdk;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();
    let app = tauri::Builder::default()
        // First, so a second launch exits before it starts anything.
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            show_window(app)
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(autostart::plugin())
        .setup(|app| {
            let handle = app.handle().clone();
            match Desktop::start(handle.clone()) {
                Ok(desktop) => {
                    app.manage(desktop);
                    if let Err(e) = autostart::enable_on_first_run(&handle) {
                        tracing::warn!(
                            error = format!("{e:#}"),
                            "could not turn launch at login on"
                        );
                    }
                    // On macOS, install the menu bar; elsewhere, the tray
                    // icon if there is a tray. Either holds the app while
                    // the window is closed; without, closing it quits.
                    #[cfg(target_os = "macos")]
                    let policy = menu::add(app).map(|()| WindowPolicy::HideOnClose);
                    #[cfg(not(target_os = "macos"))]
                    let policy = if tray::available() {
                        tray::add(app).map(|()| WindowPolicy::HideOnClose)
                    } else {
                        tracing::info!("no tray, closing the window quits");
                        Ok(WindowPolicy::QuitOnClose)
                    };
                    match policy {
                        Ok(policy) => {
                            app.manage(policy);
                            Ok(())
                        }
                        Err(e) => {
                            report_failed_start(&e.into());
                            std::process::exit(1)
                        }
                    }
                }
                Err(e) => {
                    report_failed_start(&e);
                    std::process::exit(1)
                }
            }
        })
        .on_window_event(hide_instead_of_closing)
        .invoke_handler(tauri::generate_handler![
            shares::shares,
            shares::check_share,
            shares::open_app,
            shares::clipboard_invite,
            shares::join,
            shares::leave,
            autostart::restart,
            window_policy,
            autostart::autostart_enabled,
            autostart::set_autostart_enabled,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");
    app.run(show_window_on);
}

/// Without a client there is nothing the app can do, and a window that never
/// opens tells the user nothing. The alert does, before the app exits.
/// `rfd` directly rather than Tauri's dialog plugin: this runs on the main
/// thread before the event loop, and the plugin's blocking dialog would
/// wait for that very thread to show it.
fn report_failed_start(error: &anyhow::Error) {
    tracing::error!(error = format!("{error:#}"), "could not start");
    rfd::MessageDialog::new()
        .set_level(rfd::MessageLevel::Error)
        .set_title("Wispers Access could not start")
        .set_description(format!("{error:#}"))
        .show();
}

/// What the window does when closed, which depends on whether anything else
/// (the macOS menu bar, a tray) holds the app's menu.
#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WindowPolicy {
    /// Closing hides it; the app keeps running.
    HideOnClose,
    /// Closing quits. The window is all there is, so a hidden launch shows it
    /// minimized, and the window carries the settings a menu would.
    QuitOnClose,
}

impl WindowPolicy {
    /// The policy setup chose, or hide on close if it hasn't yet.
    pub fn of(manager: &impl Manager<tauri::Wry>) -> Self {
        manager
            .try_state::<Self>()
            .map_or(Self::HideOnClose, |policy| *policy)
    }
}

/// The window's policy, for the window to know whether it carries the
/// settings a menu would.
#[tauri::command]
fn window_policy(app: tauri::AppHandle) -> WindowPolicy {
    WindowPolicy::of(&app)
}

/// The app outlives its window, so the proxy keeps serving the browser while
/// the window is away, wherever there is a menu bar or tray to hold it.
fn hide_instead_of_closing(window: &tauri::Window, event: &tauri::WindowEvent) {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        if WindowPolicy::of(window) == WindowPolicy::QuitOnClose {
            return;
        }
        api.prevent_close();
        if let Err(e) = window.hide() {
            tracing::warn!(error = %e, "could not hide the window");
        }
    }
}

/// Reveal the app window. The window is configured invisible and shown once the
/// event loop runs (which is not at all for an autostart at login, unless
/// closing the window quits: then it shows minimized, since closing it is how
/// to quit).
///
/// Reopen is the dock icon clicked while the window is hidden. That event only
/// exists on macOS.
fn show_window_on(app: &tauri::AppHandle, event: tauri::RunEvent) {
    match event {
        tauri::RunEvent::Ready if !autostart::launched_hidden() => show_window(app),
        tauri::RunEvent::Ready if WindowPolicy::of(app) == WindowPolicy::QuitOnClose => {
            show_window_minimized(app)
        }
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => show_window(app),
        _ => {}
    }
}

/// Brings the window back from hidden or minimized, in front.
fn show_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Shows the window minimized. Minimized before it shows, which X11 honours
/// so the window never appears; Wayland only minimizes shown windows, so
/// again after, which may flash it briefly.
fn show_window_minimized(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.minimize();
        let _ = window.show();
        let _ = window.minimize();
    }
}

/// The main state struct.
pub struct Desktop {
    client: Arc<sdk::Client>,
    proxy: Arc<sdk::HostRoutedProxy>,
    activity: activity::ActivityStore,
}

/// The port the proxy asks for. Fixed so bookmarks keep working across
/// restarts. No known desktop software uses it, but if you know the Fibonacci
/// sequence it's still quite memorable :)
const PROXY_PORT: u16 = 11235;

/// A pairing link is followed the moment it is minted, by a click in the UI.
const TOKEN_LIFETIME: Duration = Duration::from_secs(60);

impl Desktop {
    /// Opens the client on the app's data directory with the platform's
    /// credential store, and starts the proxy. Everything the app does needs
    /// both, so failing here fails the start.
    ///
    /// Synchronous on purpose: the client owns a tokio runtime, and dropping
    /// one inside another runtime's `block_on` is a panic, which a failed
    /// open would do.
    fn start(app: tauri::AppHandle) -> anyhow::Result<Self> {
        let data_dir = app.path().app_data_dir()?;
        let secrets = secrets::PlatformSecretStore::open(&app.config().identifier, &data_dir)?;
        let client = sdk::Client::new(sdk::ClientConfig {
            data_dir: data_dir.join("sdk").to_string_lossy().into_owned(),
            secrets: secrets.map(|store| Arc::new(store) as Arc<dyn sdk::SecretStore>),
            observer: Some(Arc::new(ShareChangeRelay { app })),
        })?;
        let auth_mode = sdk::ProxyAuthMode::Pairing {
            token_lifetime: TOKEN_LIFETIME,
        };
        let proxy = tauri::async_runtime::block_on(async {
            match client
                .start_host_routed_proxy(PROXY_PORT, auth_mode.clone())
                .await
            {
                Ok(proxy) => Ok(proxy),
                Err(e) => {
                    tracing::warn!(error = %e, "port {PROXY_PORT} is taken, using any free one");
                    client.start_host_routed_proxy(0, auth_mode).await
                }
            }
        })?;
        tracing::info!("serving shares on wa.localhost:{}", proxy.port());
        Ok(Self {
            client,
            proxy,
            activity: activity::ActivityStore::open(data_dir.join("share-activity.json")),
        })
    }
}

/// The SDK's observer. A changed share, whatever changed, has the UI reload
/// the list.
struct ShareChangeRelay {
    app: tauri::AppHandle,
}

impl sdk::Observer for ShareChangeRelay {
    fn on_share_changed(&self, _: sdk::Share) {
        if let Err(e) = self.app.emit(shares::SHARES_CHANGED, ()) {
            tracing::warn!(error = %e, "could not tell the UI about a changed share");
        }
    }
}

/// The SDK's own lines at `info`, its dependencies only when they complain.
/// `RUST_LOG` overrides.
fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("warn,wispers_access_sdk=info,wispers_access_desktop_lib=info")
    });
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
}
