//! Tauri commands and data formats for shares.

use crate::Desktop;
use serde::Serialize;
use std::time::{Duration, SystemTime};
use tauri::State;
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_opener::OpenerExt;
use wispers_access_sdk as sdk;

/// The event that tells the UI to call [`shares`] again.
pub const SHARES_CHANGED: &str = "shares-changed";

/// A share as the UI shows it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareView {
    id: String,
    name: String,
    /// The `<share>` in the apps' addresses.
    label: String,
    /// The transport's full name, `iroh` or `wispers-connect`.
    transport: &'static str,
    apps: Vec<AppView>,
    /// `live`, `removed` or `revoked`.
    state: &'static str,
    joined_at_ms: u64,
    last_connected_ms: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppView {
    id: String,
    name: String,
    /// `web`, `jellyfin` or `immich`.
    kind: &'static str,
    /// Where the browser finds the app, `<app>.<share>.wa.localhost:<port>`.
    host: String,
}

/// List all joined shares, from the store. Doesn't wait for the network.
#[tauri::command]
pub fn shares(desktop: State<Desktop>) -> Result<Vec<ShareView>, String> {
    let shares = desktop.client.shares().map_err(|e| e.to_string())?;
    Ok(shares
        .iter()
        .map(|share| ShareView::of(share, &desktop))
        .collect())
}

impl ShareView {
    fn of(share: &sdk::Share, desktop: &Desktop) -> Self {
        let port = desktop.proxy.port();
        Self {
            id: share.id.to_string(),
            name: share.name.clone(),
            label: share.label.clone(),
            transport: share.transport.as_str(),
            apps: share
                .apps
                .iter()
                .map(|app| AppView {
                    id: app.id.clone(),
                    name: app.name.clone(),
                    kind: app.kind.as_str(),
                    host: format!("{}.{}.wa.localhost:{port}", app.id, share.label),
                })
                .collect(),
            state: describe_state(share.state),
            joined_at_ms: epoch_ms(share.joined_at),
            last_connected_ms: desktop.activity.last_connected(&share.id),
        }
    }
}

fn describe_state(state: sdk::ShareState) -> &'static str {
    match state {
        sdk::ShareState::Live => "live",
        sdk::ShareState::Removed => "removed",
        sdk::ShareState::Revoked => "revoked",
    }
}

fn epoch_ms(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Timeout for connectivity checks.A reachable host node answers in well under
/// a second. Only a blackholing connect runs into this.
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// The result of a connectivity check.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareStatus {
    /// `online`, `offline`, `unknown`, or the terminal `removed` / `revoked`.
    availability: &'static str,
    last_connected_ms: Option<u64>,
}

/// Asks the share's host node whether anything changed, which doubles as
/// the reachability check behind the status dot. A changed app list reaches
/// the UI through [`SHARES_CHANGED`] on the way.
#[tauri::command]
pub async fn check_share(
    desktop: State<'_, Desktop>,
    share_id: String,
) -> Result<ShareStatus, String> {
    let share = find(&desktop, &share_id)?;
    let availability =
        match tokio::time::timeout(CHECK_TIMEOUT, desktop.client.refresh(share.id.clone())).await {
            Ok(Ok(Some(changed))) if changed.state != sdk::ShareState::Live => {
                describe_state(changed.state)
            }
            Ok(Ok(_)) => {
                desktop.activity.mark_connected(&share.id);
                "online"
            }
            Ok(Err(sdk::SdkError::HostNode(_))) => "offline",
            Ok(Err(e)) => {
                tracing::warn!(share = %share.label, error = %e, "could not check the share");
                "unknown"
            }
            Err(_) => "unknown",
        };
    Ok(ShareStatus {
        availability,
        last_connected_ms: desktop.activity.last_connected(&share.id),
    })
}

/// Opens an app in the system browser, pairing the browser on the way.
#[tauri::command]
pub fn open_app(
    app: tauri::AppHandle,
    desktop: State<Desktop>,
    share_id: String,
    app_id: String,
) -> Result<(), String> {
    let share = find(&desktop, &share_id)?;
    let url = desktop
        .proxy
        .browse_url(share.id, app_id)
        .map_err(|e| e.to_string())?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

/// The clipboard's text if it is an invite code, for prefilling the join
/// form.
#[tauri::command]
pub fn clipboard_invite(app: tauri::AppHandle) -> Option<String> {
    let text = app.clipboard().read_text().ok()?;
    let code = text.trim();
    sdk::validate_invite(code.to_owned())
        .ok()
        .map(|_| code.to_owned())
}

/// Joins the share an invite code is for. The error is the message the UI
/// shows.
#[tauri::command]
pub async fn join(desktop: State<'_, Desktop>, invite_code: String) -> Result<ShareView, String> {
    let share = desktop
        .client
        .join(invite_code.trim().to_owned())
        .await
        .map_err(|e| e.to_string())?;
    desktop.activity.mark_connected(&share.id);
    Ok(ShareView::of(&share, &desktop))
}

/// Removes a share from this device.
#[tauri::command]
pub async fn leave(desktop: State<'_, Desktop>, share_id: String) -> Result<(), String> {
    let share = find(&desktop, &share_id)?;
    desktop
        .client
        .leave(share.id.clone())
        .await
        .map_err(|e| e.to_string())?;
    desktop.activity.remove(&share.id);
    Ok(())
}

/// The share an ID from the UI names, or the message for one that is gone.
fn find(desktop: &Desktop, share_id: &str) -> Result<sdk::Share, String> {
    desktop
        .client
        .share(share_id.to_owned())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no share {share_id}"))
}
