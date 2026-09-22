//! Inter-process communication between server and cli tool.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::error;

#[cfg(windows)]
use tokio::net::{TcpListener, TcpStream};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};

#[cfg(unix)]
type ReadHalf = tokio::net::unix::OwnedReadHalf;
#[cfg(windows)]
type ReadHalf = tokio::net::tcp::OwnedReadHalf;
#[cfg(unix)]
type WriteHalf = tokio::net::unix::OwnedWriteHalf;
#[cfg(windows)]
type WriteHalf = tokio::net::tcp::OwnedWriteHalf;

#[cfg(unix)]
pub struct Server {
    listener: UnixListener,
}

#[cfg(unix)]
impl Server {
    pub async fn bind(share: &str) -> Result<Self> {
        let path = ipc_path(share);

        // Check for a stale socket.
        if path.exists() {
            match UnixStream::connect(&path).await {
                Ok(_) => {
                    anyhow::bail!("Server already running at {:?}", path);
                }
                Err(_) => {
                    // Stale socket, remove it
                    fs::remove_file(&path).context("failed to remove stale socket")?;
                }
            }
        }

        // Ensure the socket directory exists.
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Bind the socket.
        let listener = UnixListener::bind(&path).context("failed to bind socket")?;

        Ok(Self { listener })
    }

    pub async fn run(self, serving_handle: crate::serving::ServingHandle) {
        loop {
            let stream = match self.listener.accept().await {
                Ok((stream, _)) => stream,
                Err(e) => {
                    error!(error = %e, "Failed to accept IPC connection");
                    continue;
                }
            };
            let (reader, writer) = stream.into_split();
            let reader = BufReader::new(reader);
            let shutdown = handle_request(reader, writer, serving_handle.clone()).await;
            if shutdown {
                break;
            }
        }
    }
}

#[cfg(windows)]
pub struct Server {
    listener: TcpListener,
    /// Password that Windows clients must send before any request. Stored in
    /// the `.port` file alongside the port, readable only by the user.
    windows_ipc_password: String,
}

#[cfg(windows)]
impl Server {
    pub async fn bind(share: &str) -> Result<Self> {
        use rand::distr::SampleString;

        let path = ipc_path(share);

        // Check for a stale socket.
        if path.exists() {
            if let Ok(contents) = fs::read_to_string(&path)
                && let Some((port, _)) = parse_port_file(&contents)
                && TcpStream::connect(("127.0.0.1", port)).await.is_ok()
            {
                anyhow::bail!("daemon already running on port {}", port);
            }
            fs::remove_file(&path).context("failed to remove stale port file")?;
        }

        // Ensure the ports directory exists.
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Bind the local port.
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("failed to bind TCP listener")?;
        let port = listener.local_addr()?.port();

        // Write a random password for IPC auth.
        let password = rand::distr::Alphanumeric.sample_string(&mut rand::rng(), 32);
        fs::write(&path, format!("{}:{}", port, password)).context("failed to write port file")?;

        Ok(Self {
            listener,
            windows_ipc_password: password,
        })
    }

    pub async fn run(self, serving_handle: crate::serving::ServingHandle) {
        loop {
            let stream = match self.listener.accept().await {
                Ok((stream, _)) => stream,
                Err(e) => {
                    error!(error = %e, "Failed to accept IPC connection");
                    continue;
                }
            };
            let (reader, writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut password_line = String::new();
            if reader.read_line(&mut password_line).await.is_err() {
                continue;
            }
            if password_line.trim() != self.windows_ipc_password {
                continue; // Wrong password.
            }
            let shutdown = handle_request(reader, writer, serving_handle.clone()).await;
            if shutdown {
                break;
            }
        }
    }
}

/// Request from CLI to running server.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Status,
    GetInvite {
        node_name: String,
        user_id: String,
    },
    /// iroh only: mark a guest revoked and close its live connections.
    RevokeGuest {
        number: i64,
    },
    Reload,
    Shutdown,
}

/// Response from running server to CLI.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Response {
    Success { ok: bool, data: ResponseData },
    Error { ok: bool, error: String },
}

/// Data payload for successful responses.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ResponseData {
    Status(StatusData),
    Invite(InviteData),
    Reload(ReloadData),
    Empty,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StatusData {
    /// Reachable by guests: connected to the hub (Wispers Connect), or
    /// online with a home relay (iroh).
    pub reachable: bool,
    /// The apps as currently served, in config order.
    pub apps: Vec<AppData>,
    /// Hash of the served app list. Differs from the file's when a
    /// `reload` is pending.
    pub config_hash: u64,
    pub pid: u32,
    pub started_at: String, // RFC 3339
    /// Since when the daemon has been reachable; `None` while it is not.
    #[serde(default)]
    pub connected_since: Option<String>, // RFC 3339
    /// Guests with a live P2P connection to this host node right now.
    #[serde(default)]
    pub connected_guests: Option<Vec<GuestData>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GuestData {
    /// The node number (Wispers Connect) or guest number (iroh).
    pub node_number: i32,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub connected_since: Option<String>, // RFC 3339
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AppData {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub kind: crate::config::AppKind,
    pub upstream: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InviteData {
    /// The invite code, ready to show.
    pub code: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ReloadData {
    pub changed: bool,
    pub apps: Vec<AppData>,
}

impl Response {
    pub fn success(data: ResponseData) -> Self {
        Response::Success { ok: true, data }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Response::Error {
            ok: false,
            error: msg.into(),
        }
    }
}

async fn handle_request(
    reader: BufReader<ReadHalf>,
    writer: WriteHalf,
    handle: crate::serving::ServingHandle,
) -> bool {
    let mut shutdown = false;
    let response = match parse_request(reader).await {
        Ok(Request::Status) => handle_status(&handle).await,
        Ok(Request::GetInvite { node_name, user_id }) => {
            handle_invite(&handle, &node_name, &user_id).await
        }
        Ok(Request::RevokeGuest { number }) => match handle.revoke_guest(number).await {
            Ok(_) => Response::success(ResponseData::Empty),
            Err(e) => Response::error(format!("{:#}", e)),
        },
        Ok(Request::Reload) => handle_reload(&handle).await,
        Ok(Request::Shutdown) => {
            shutdown = true;
            handle_shutdown(&handle).await
        }
        Err(s) => Response::error(s),
    };
    send_response(writer, response).await;
    shutdown
}

async fn parse_request(mut reader: BufReader<ReadHalf>) -> std::result::Result<Request, String> {
    let mut line = String::new();
    match reader.read_line(&mut line).await {
        Err(e) => Err(format!("cannot read request: {}", e)),
        Ok(_) => match serde_json::from_str::<Request>(&line) {
            Err(e) => Err(format!("invalid request: {}", e)),
            Ok(r) => Ok(r),
        },
    }
}

async fn handle_status(handle: &crate::serving::ServingHandle) -> Response {
    let connected = handle
        .connected_guests()
        .await
        .into_iter()
        .map(|p| GuestData {
            node_number: p.number,
            user_id: Some(p.user_id),
            connected_since: Some(fmt_rfc3339(p.connected_since)),
        })
        .collect();
    let config = handle.config().await;
    Response::success(ResponseData::Status(StatusData {
        reachable: handle.reachable().await,
        apps: app_data(&config),
        config_hash: config.config_hash(),
        pid: std::process::id(),
        started_at: fmt_rfc3339(handle.started_at()),
        connected_since: handle.reachable_since().await.map(fmt_rfc3339),
        connected_guests: Some(connected),
    }))
}

fn app_data(config: &crate::config::ShareConfig) -> Vec<AppData> {
    config
        .apps
        .iter()
        .map(|s| AppData {
            id: s.id.clone(),
            name: s.name.clone(),
            kind: s.kind,
            upstream: s.upstream.clone(),
        })
        .collect()
}

fn fmt_rfc3339(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

async fn handle_invite(
    handle: &crate::serving::ServingHandle,
    node_name: &str,
    user_id: &str,
) -> Response {
    match handle.invite(node_name, user_id).await {
        Ok(invite) => Response::success(ResponseData::Invite(InviteData {
            code: invite.to_code(),
        })),
        Err(e) => Response::error(invite_error_message(&e)),
    }
}

/// Explain a quota rejection after an `invite` if it happens.
fn invite_error_message(e: &anyhow::Error) -> String {
    match e.downcast_ref::<crate::wcbe::QuotaExceeded>() {
        Some(q) if q.quota == "nodes_per_group" => format!(
            "the share is full: {} of {} node quota used (this host, guests and pending \
             invites). Revoke nodes with `waserver revoke` or wait for a pending
             invite to expire. `waserver status <share>` shows the available quota.",
            q.current, q.limit
        ),
        _ => format!("error generating registration token: {}", e),
    }
}

async fn handle_reload(handle: &crate::serving::ServingHandle) -> Response {
    match handle.reload().await {
        Ok(outcome) => Response::success(ResponseData::Reload(ReloadData {
            changed: outcome.changed,
            apps: app_data(&outcome.config),
        })),
        Err(e) => Response::error(format!("{:#}", e)),
    }
}

async fn handle_shutdown(handle: &crate::serving::ServingHandle) -> Response {
    let _ = handle.shutdown().await;
    Response::success(ResponseData::Empty)
}

async fn send_response(mut writer: WriteHalf, resp: Response) {
    let json = serde_json::to_string(&resp).unwrap_or_else(|e| {
        serde_json::to_string(&Response::error(format!("serialization error: {}", e))).unwrap()
    });
    if let Err(e) = writer.write_all(json.as_bytes()).await {
        error!(error = %e, "Failed to write response");
        return;
    }
    if let Err(e) = writer.write_all(b"\n").await {
        error!(error = %e, "Failed to write newline");
        return;
    }
    if let Err(e) = writer.flush().await {
        error!(error = %e, "Failed to flush");
    }
}

/// Client for connecting to the daemon.
pub struct Client {
    reader: BufReader<ReadHalf>,
    writer: WriteHalf,
}

impl Client {
    #[cfg(unix)]
    pub async fn connect(share: &str) -> Result<Self> {
        let path = ipc_path(share);
        let stream = UnixStream::connect(&path).await.with_context(|| {
            format!("failed to connect to server at {:?} (is it running?)", path)
        })?;
        let (reader, writer) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(reader),
            writer,
        })
    }

    #[cfg(windows)]
    pub async fn connect(share: &str) -> Result<Self> {
        let path = ipc_path(share);
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("daemon not running (no port file {:?})", path))?;
        let (port, password) = parse_port_file(&contents).context("invalid daemon port file")?;
        let stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .with_context(|| format!("daemon not running (port {})", port))?;
        let (reader, mut writer) = stream.into_split();
        // Send IPC password
        writer.write_all(password.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
        Ok(Self {
            reader: BufReader::new(reader),
            writer,
        })
    }

    pub async fn request(&mut self, req: &Request) -> Result<Response> {
        let req_json = serde_json::to_string(req)?;
        self.writer.write_all(req_json.as_bytes()).await?;
        self.writer.write_all(b"\n").await?;
        self.writer.flush().await?;

        let mut line = String::new();
        self.reader.read_line(&mut line).await?;

        let response: Response = serde_json::from_str(&line)?;
        Ok(response)
    }
}

#[cfg(unix)]
fn ipc_path(share: &str) -> PathBuf {
    let base = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    let dir = base.join(".waserver").join("sockets");
    dir.join(format!("{}.sock", share))
}

#[cfg(windows)]
fn ipc_path(share: &str) -> PathBuf {
    let base = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    let dir = base.join(".waserver").join("ports");
    return dir.join(format!("{}.port", share));
}

#[cfg(windows)]
fn parse_port_file(contents: &str) -> Option<(u16, &str)> {
    let contents = contents.trim();
    let colon = contents.find(':')?;
    let port: u16 = contents[..colon].parse().ok()?;
    let password = &contents[colon + 1..];
    Some((port, password))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_quota_error_names_the_way_out() {
        let quota = anyhow::Error::new(crate::wcbe::QuotaExceeded {
            quota: "nodes_per_group".to_owned(),
            limit: 12,
            current: 12,
        });
        let msg = invite_error_message(&quota);
        assert!(msg.contains("12 of 12 node quota used"), "{msg}");
        assert!(msg.contains("waserver revoke"), "{msg}");

        // Other errors keep the raw passthrough.
        let other = anyhow::anyhow!("server returned 500: boom");
        let msg = invite_error_message(&other);
        assert_eq!(
            msg,
            "error generating registration token: server returned 500: boom"
        );
    }
}
