//! The native side of the desktop app.

mod activity;
mod secrets;
mod shares;

use std::sync::Arc;
use std::time::Duration;
use tauri::{Emitter, Manager};
use wispers_access_sdk as sdk;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let desktop = tauri::async_runtime::block_on(Desktop::start(app.handle().clone()))?;
            app.manage(desktop);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            shares::shares,
            shares::check_share,
            shares::open_app,
            shares::clipboard_invite,
            shares::join,
            shares::leave,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// The main state struct.
pub struct Desktop {
    client: Arc<sdk::Client>,
    proxy: Arc<sdk::HostRoutedProxy>,
    activity: activity::ActivityStore,
}

/// The port the proxy asks for. Fixed so bookmarks keep working across
/// restarts.
const PROXY_PORT: u16 = 4242;

/// A pairing link is followed the moment it is minted, by a click in the UI.
const TOKEN_LIFETIME: Duration = Duration::from_secs(60);

impl Desktop {
    /// Opens the client on the app's data directory with the platform's
    /// credential store, and starts the proxy. Everything the app does needs
    /// both, so failing here fails the start.
    async fn start(app: tauri::AppHandle) -> anyhow::Result<Self> {
        let data_dir = app.path().app_data_dir()?;
        let secrets = secrets::PlatformSecretStore::open(&app.config().identifier);
        let client = sdk::Client::new(sdk::ClientConfig {
            data_dir: data_dir.join("sdk").to_string_lossy().into_owned(),
            secrets: secrets.map(|store| Arc::new(store) as Arc<dyn sdk::SecretStore>),
            observer: Some(Arc::new(ShareChangeRelay { app })),
        })?;
        let auth_mode = sdk::ProxyAuthMode::Pairing {
            token_lifetime: TOKEN_LIFETIME,
        };
        let proxy = match client
            .start_host_routed_proxy(PROXY_PORT, auth_mode.clone())
            .await
        {
            Ok(proxy) => proxy,
            Err(e) => {
                tracing::warn!(error = %e, "port {PROXY_PORT} is taken, using any free one");
                client.start_host_routed_proxy(0, auth_mode).await?
            }
        };
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
