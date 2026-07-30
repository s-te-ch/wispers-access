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
pub type IpcStream = UnixStream;
#[cfg(windows)]
pub type IpcStream = TcpStream;
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
            let shutdown = handle_request(stream, serving_handle.clone()).await;
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
        use rand::Rng;

        let path = ipc_path(share);

        // Check for a stale socket.
        if path.exists() {
            if let Ok(contents) = fs::read_to_string(&path)
                && let Some((port, _)) = parse_port_file(&contents)
                && TcpStream::connect(("127.0.0.1", port)).await.is_ok()
            {
                anyhow::bail!("daemon already running on port {}", port);
            }
            fs::remove_file(&path)
                .await
                .context("failed to remove stale port file")?;
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
        let password: String = rand::rng()
            .sample_iter(rand::distr::Alphanumeric)
            .take(32)
            .map(char::from)
            .collect();
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
            let mut buf_stream = BufReader::new(stream);
            let mut password_line = String::new();
            if let Err(_) = buf_stream.read_line(&mut password_line).await {
                continue;
            }
            if password_line.trim() != self.windows_ipc_password {
                continue; // Wrong password.
            }
            let shutdown = handle_request(stream, serving_handle.clone()).await;
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
    GetInvite { node_name: String, user_id: String },
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
    Empty,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StatusData {
    pub connected_to_hub: bool,
    pub upstream: String,
    // TODO: This is optional for backewards compat. Remove with the next version.
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub started_at: Option<String>, // RFC 3339
    #[serde(default)]
    pub connected_since: Option<String>, // RFC 3339
    /// This server's own node number in the connectivity group.
    #[serde(default)]
    pub node_number: Option<i32>,
    /// Guests with a live P2P connection to this server right now.
    #[serde(default)]
    pub connected_peers: Option<Vec<PeerData>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PeerData {
    pub node_number: i32,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub connected_since: Option<String>, // RFC 3339
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InviteData {
    pub registration_token: String,
    pub activation_code: String,
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

async fn handle_request(stream: IpcStream, handle: crate::serving::ServingHandle) -> bool {
    let (reader, writer) = stream.into_split();
    let reader = BufReader::new(reader);
    let mut shutdown = false;
    let response = match parse_request(reader).await {
        Ok(Request::Status) => handle_status(&handle).await,
        Ok(Request::GetInvite { node_name, user_id }) => {
            handle_invite(&handle, &node_name, &user_id).await
        }
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
    let peers = handle
        .connected_peers()
        .await
        .into_iter()
        .map(|p| PeerData {
            node_number: p.node_number,
            user_id: p.user_id,
            connected_since: Some(fmt_rfc3339(p.connected_since)),
        })
        .collect();
    Response::success(ResponseData::Status(StatusData {
        connected_to_hub: handle.connected_to_hub().await,
        upstream: handle.upstream().to_string(),
        pid: Some(std::process::id()),
        started_at: Some(fmt_rfc3339(handle.started_at())),
        connected_since: handle.connected_since().await.map(fmt_rfc3339),
        node_number: handle.own_node_number(),
        connected_peers: Some(peers),
    }))
}

fn fmt_rfc3339(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

async fn handle_invite(
    handle: &crate::serving::ServingHandle,
    node_name: &str,
    user_id: &str,
) -> Response {
    let token = match handle.get_registration_token(node_name, user_id).await {
        Ok(t) => t,
        Err(e) => return Response::error(invite_error_message(&e)),
    };
    let code = match handle.get_activation_code().await {
        Ok(c) => c,
        Err(e) => {
            let e = format!("error generating activation code: {}", e);
            return Response::error(e);
        }
    };
    Response::success(ResponseData::Invite(InviteData {
        registration_token: token,
        activation_code: code,
    }))
}

/// Explain a quota rejection after an `invite` if it happens.
fn invite_error_message(e: &anyhow::Error) -> String {
    match e.downcast_ref::<crate::wcbe::QuotaExceeded>() {
        Some(q) if q.quota == "nodes_per_group" => format!(
            "the share is full: {} of {} node quota used (members and pending \
             invites). Revoke nodes with `waserver revoke` or wait for a pending
             invite to expire. `waserver status <share>` shows the available quota.",
            q.current, q.limit
        ),
        _ => format!("error generating registration token: {}", e),
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
            .await
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
